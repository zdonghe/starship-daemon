use std::mem;
use std::num::NonZeroUsize;
use std::time::Instant;

use std::path::{Path, PathBuf};
use std::ffi::c_void;

use lru::LruCache;
use starship_daemon::cache::{self, CacheKey, CachedValue, RenderContext};
use starship_daemon::ffi::{self, HANDLE, DWORD, LPVOID, LPCVOID};
use starship_daemon::watch::WatcherState;
use starship_daemon::{ParsedRequest, parse_request};

const PIPE_ACCESS_DUPLEX: DWORD = 3;
const FILE_FLAG_OVERLAPPED: DWORD = 0x40000000;
const PIPE_TYPE_MESSAGE: DWORD = 4;
const PIPE_WAIT: DWORD = 0;
const ERROR_PIPE_CONNECTED: DWORD = 535;
const ERROR_IO_PENDING: DWORD = 997;
const PIPE_READMODE_BYTE: DWORD = 0;
const MAX_SESSIONS: usize = 8;
const MAX_IDLE_MS: u128 = 5000;
const BUF_SIZE: usize = 4 + 32768 + 4 + 4096;

fn read_exact(pipe: HANDLE, event: HANDLE, buf: &mut [u8]) -> bool {
    unsafe {
        let mut ol: ffi::OVERLAPPED = mem::zeroed();
        ol.h_event = event;
        ffi::ResetEvent(event);
        let mut r: DWORD = 0;
        let ret = ffi::ReadFile(pipe, buf.as_mut_ptr() as LPVOID, buf.len() as DWORD, &mut r, &mut ol as *mut _ as *mut c_void);
        if ret != 0 { return r == buf.len() as DWORD; }
        if ffi::GetLastError() == ERROR_IO_PENDING {
            let mut bytes: DWORD = 0;
            return ffi::GetOverlappedResult(pipe, &mut ol as *mut _ as *mut c_void, &mut bytes, 1) != 0
                && bytes == buf.len() as DWORD;
        }
        false
    }
}

fn write_all(pipe: HANDLE, buf: &[u8]) -> bool {
    unsafe {
        let mut ol: ffi::OVERLAPPED = mem::zeroed();
        let mut w: DWORD = 0;
        let ret = ffi::WriteFile(pipe, buf.as_ptr() as LPCVOID, buf.len() as DWORD, &mut w, &mut ol as *mut _ as *mut c_void);
        if ret != 0 { return w == buf.len() as DWORD; }
        if ffi::GetLastError() == ERROR_IO_PENDING {
            let mut bytes: DWORD = 0;
            return ffi::GetOverlappedResult(pipe, &mut ol as *mut _ as *mut c_void, &mut bytes, 1) != 0
                && bytes == buf.len() as DWORD;
        }
        false
    }
}

struct Session {
    pipe: HANDLE,
    ol: ffi::OVERLAPPED,
    event: HANDLE,
    buf: [u8; BUF_SIZE],
    active: bool,
    sync_bytes: u32,
    last_activity: Instant,
}

impl Session {
    fn new(pipe: HANDLE, event: HANDLE) -> Self {
        Session {
            pipe,
            ol: unsafe { mem::zeroed() },
            event,
            buf: [0u8; BUF_SIZE],
            active: false,
            sync_bytes: 0,
            last_activity: Instant::now(),
        }
    }
}

#[derive(Debug)]
enum ReadResult { Data(u32), Closed }

fn issue_read(s: &mut Session) -> bool {
    unsafe {
        s.ol = mem::zeroed();
        s.ol.h_event = s.event;
        ffi::ResetEvent(s.event);
        let mut read: DWORD = 0;
        let ret = ffi::ReadFile(s.pipe, s.buf.as_mut_ptr() as LPVOID, 4, &mut read, &mut s.ol as *mut _ as *mut c_void);
        if ret != 0 {
            s.sync_bytes = read;
            return true;
        }
        let err = ffi::GetLastError();
        if err == ERROR_IO_PENDING {
            s.sync_bytes = 0;
            return false;
        }
        ffi::SetEvent(s.event);
        s.sync_bytes = 0;
        false
    }
}

fn complete_read(s: &mut Session) -> ReadResult {
    unsafe {
        let mut bytes: DWORD = 0;
        let ok = ffi::GetOverlappedResult(s.pipe, &mut s.ol as *mut _ as *mut c_void, &mut bytes, 1);
        if ok != 0 {
            return ReadResult::Data(bytes);
        }
        ReadResult::Closed
    }
}

fn rearm_connect(s: &mut Session) {
    unsafe {
        s.ol = mem::zeroed();
        s.ol.h_event = s.event;
        ffi::ResetEvent(s.event);
        let ret = ffi::ConnectNamedPipe(s.pipe, &mut s.ol as *mut _ as *mut c_void);
        if ret != 0 || ffi::GetLastError() == ERROR_PIPE_CONNECTED {
            ffi::SetEvent(s.event);
        }
    }
}

fn send_response(pipe: HANDLE, output: &str) -> bool {
    let b = output.as_bytes();
    let len_bytes = (b.len() as u32).to_le_bytes();
    write_all(pipe, &len_bytes) && write_all(pipe, b)
}

fn disconnect_session(s: &mut Session) {
    let ok = unsafe { ffi::DisconnectNamedPipe(s.pipe) };
    if ok != 0 && s.active && s.sync_bytes == 0 {
        // An async read may be pending; DisconnectNamedPipe cancels it.
        // Wait for the cancellation to fully complete before reusing the
        // OVERLAPPED and resetting the event in rearm_connect, so a stale
        // completion cannot signal the event after ResetEvent.
        unsafe {
            let mut bytes: DWORD = 0;
            let _ = ffi::GetOverlappedResult(s.pipe, &mut s.ol as *mut _ as *mut c_void, &mut bytes, 1);
        }
    }
    s.active = false;
    rearm_connect(s);
}

fn drain_session(
    s: &mut Session,
    config_path: &mut PathBuf,
    cached_config: &mut toml::Table,
    last_cfg_mtime: &mut u64,
    lru: &mut LruCache<CacheKey, CachedValue>,
    watcher: &mut WatcherState,
) {
    loop {
        let n = if s.sync_bytes > 0 {
            let n = s.sync_bytes;
            s.sync_bytes = 0;
            n
        } else {
            match complete_read(s) {
                ReadResult::Data(n) => n,
                ReadResult::Closed => { disconnect_session(s); break; }
            }
        };
        if n < 4 { disconnect_session(s); break; }
        if !process_request(s, config_path, cached_config, last_cfg_mtime, lru, watcher) {
            disconnect_session(s);
            break;
        }
        if !issue_read(s) { break; }
    }
}

fn process_request(s: &mut Session, config_path: &mut PathBuf, cached_config: &mut toml::Table, last_cfg_mtime: &mut u64, lru: &mut LruCache<CacheKey, CachedValue>, watcher: &mut WatcherState) -> bool {
    let cwd_len = u32::from_le_bytes(s.buf[..4].try_into().unwrap()) as usize;
    if cwd_len > 32768 || cwd_len == 0 { return false; }
    if !read_exact(s.pipe, s.event, &mut s.buf[4..4 + cwd_len]) { return false; }
    let props_start = 4 + cwd_len;
    if !read_exact(s.pipe, s.event, &mut s.buf[props_start..props_start + 4]) { return false; }
    let props_len = u32::from_le_bytes(s.buf[props_start..props_start + 4].try_into().unwrap()) as usize;
    if props_len > 4096 || props_len == 0 { return false; }
    if !read_exact(s.pipe, s.event, &mut s.buf[props_start + 4..props_start + 4 + props_len]) { return false; }
    let total = props_start + 4 + props_len;

    let ParsedRequest { cwd, props } = match parse_request(&s.buf[..total]) { Some(r) => r, None => { return false; } };
    let git_dir = starship_daemon::find_git_dir(&cwd);
    let status_code = props.status_code.unwrap_or(0);
    let keymap = props.keymap.unwrap_or_else(|| "vi".to_string());

    let mut cfg_changed = false;
    if let Some(ref req) = props.starship_config {
        let p = Path::new(req);
        if p != config_path.as_path() {
            if let Some(new_cfg) = cache::load_config(&p) {
                *config_path = new_cfg;
                lru.clear();
                cache::clear_repo_cache();
                unsafe { std::env::set_var("STARSHIP_CONFIG", req); }
                *cached_config = cache::read_config(config_path);
                *last_cfg_mtime = cache::get_mtime_ns(config_path);
                cfg_changed = true;
            }
        }
    }

    let cur_cfg_mtime = if !cfg_changed {
        let mtime = cache::get_mtime_ns(config_path);
        if mtime != *last_cfg_mtime {
            *last_cfg_mtime = mtime;
            *cached_config = cache::read_config(config_path);
            lru.clear();
            cache::clear_repo_cache();
        }
        mtime
    } else {
        *last_cfg_mtime
    };

    let tw = props.terminal_width.unwrap_or(120);

    if props.disable_cache.unwrap_or(false) {
        let ctx = RenderContext { cwd: cwd.clone(), terminal_width: tw, status_code, keymap };
        let output = cache::render_prompt_with_config(&ctx, git_dir.as_deref(), cached_config);
        return send_response(s.pipe, &output);
    }

    let repo_root = git_dir.as_ref().and_then(|g| g.parent());
    let watcher_version = if let Some(r) = repo_root {
        watcher.ensure(r);
        watcher.flush();
        watcher.version(r)
    } else {
        0
    };
    let ck = cache::compute_cache_key(&cwd, status_code, &keymap, tw, cur_cfg_mtime, watcher_version);
    let ctx = RenderContext { cwd: cwd.clone(), terminal_width: tw, status_code, keymap };

    let output = cache::render_cached(&ctx, git_dir.as_deref(), cached_config, &ck, lru);
    send_response(s.pipe, &output)
}

fn main() {
    let config_path = cache::default_config_path();
    let mut config_path = match cache::load_config(&config_path) {
        Some(p) => p,
        None => { eprintln!("Could not load config"); std::process::exit(1); }
    };
    let mut lru: LruCache<CacheKey, CachedValue> = LruCache::new(NonZeroUsize::new(256).unwrap());
    let mut cached_config = cache::read_config(&config_path);
    let mut last_cfg_mtime = cache::get_mtime_ns(&config_path);
    let pipe_name = starship_daemon::pipe_name();
    let wide = ffi::to_wide(&pipe_name);

    let mut sessions = Vec::with_capacity(MAX_SESSIONS);
    for _ in 0..MAX_SESSIONS {
        let pipe = unsafe { ffi::CreateNamedPipeW(wide.as_ptr(), PIPE_ACCESS_DUPLEX|FILE_FLAG_OVERLAPPED, PIPE_TYPE_MESSAGE|PIPE_WAIT, MAX_SESSIONS as DWORD, 65536, 65536, 0, std::ptr::null()) };
        if pipe == ffi::INVALID_HANDLE_VALUE { std::process::exit(1); }
        let mode = PIPE_READMODE_BYTE;
        unsafe { ffi::SetNamedPipeHandleState(pipe, &mode as *const _ as *mut _, std::ptr::null_mut(), std::ptr::null_mut()); }
        let event = unsafe { ffi::CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() { std::process::exit(1); }
        sessions.push(Session::new(pipe, event));
    }

    for s in &mut sessions { rearm_connect(s); }
    println!("starship-daemon started on {}", pipe_name);

    {
        let warm_ctx = RenderContext {
            cwd: PathBuf::from("."),
            terminal_width: 120,
            status_code: 0,
            keymap: "vi".to_string(),
        };
        let warm_key = cache::compute_cache_key(
            Path::new("."), 0, "vi", 120, 0, 0,
        );
        let _ = cache::render_cached(&warm_ctx, None, &cached_config, &warm_key, &mut lru);
    }

    let mut watcher = WatcherState::new();
    let mut handles = Vec::with_capacity(MAX_SESSIONS + watcher.num_entries());

    loop {
        let now = Instant::now();
        for s in &mut sessions {
            if s.active && now.duration_since(s.last_activity).as_millis() > MAX_IDLE_MS {
                disconnect_session(s);
            }
        }
        for s in &sessions { handles.push(s.event); }
        handles.extend(watcher.change_events());
        let timeout: DWORD = 1000;
        let total = handles.len() as DWORD;
        let rc = unsafe { ffi::WaitForMultipleObjects(total, handles.as_ptr(), 0, timeout) };
        if rc >= ffi::WAIT_OBJECT_0 && rc < ffi::WAIT_OBJECT_0 + total {
            let idx = (rc - ffi::WAIT_OBJECT_0) as usize;
            if idx < sessions.len() {
                let s = &mut sessions[idx];
                s.last_activity = Instant::now();
                if s.active {
                    drain_session(s, &mut config_path, &mut cached_config, &mut last_cfg_mtime, &mut lru, &mut watcher);
                } else {
                    s.active = true;
                    if issue_read(s) {
                        drain_session(s, &mut config_path, &mut cached_config, &mut last_cfg_mtime, &mut lru, &mut watcher);
                    }
                }
            } else {
                let w_idx = idx - sessions.len();
                watcher.handle_event(w_idx);
            }
        }
        handles.clear();
    }
}
