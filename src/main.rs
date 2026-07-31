use std::mem;
use std::num::NonZeroUsize;
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

macro_rules! pipe_error { ($p:expr) => { unsafe { ffi::DisconnectNamedPipe($p); return Err(()); } } }

fn read_exact(pipe: HANDLE, buf: &mut [u8]) -> bool {
    unsafe {
        let mut total: DWORD = 0;
        let mut left: DWORD = 0;
        if ffi::PeekNamedPipe(pipe, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut total, &mut left) != 0 {
            if left > buf.len() as DWORD { return false; }
        }
        let mut r: DWORD = 0;
        ffi::ReadFile(pipe, buf.as_mut_ptr() as LPVOID, buf.len() as DWORD, &mut r, std::ptr::null_mut()) != 0
            && r == buf.len() as DWORD
    }
}

fn write_all(pipe: HANDLE, buf: &[u8]) -> bool {
    unsafe { let mut w: DWORD = 0; ffi::WriteFile(pipe, buf.as_ptr() as LPCVOID, buf.len() as DWORD, &mut w, std::ptr::null_mut()) != 0 }
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
    let wide = ffi::to_wide(starship_daemon::PIPE_NAME);
    let pipe = unsafe { ffi::CreateNamedPipeW(wide.as_ptr(), PIPE_ACCESS_DUPLEX|FILE_FLAG_OVERLAPPED, PIPE_TYPE_MESSAGE|PIPE_WAIT, 1, 65536, 65536, 0, std::ptr::null()) };
    if pipe == ffi::INVALID_HANDLE_VALUE { std::process::exit(1); }
    let connect_event = unsafe { ffi::CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    let mut connect_ol: ffi::OVERLAPPED = unsafe { mem::zeroed() };

    rearm_connect(pipe, &mut connect_ol, connect_event);
    println!("starship-daemon started on {}", starship_daemon::PIPE_NAME);

    let _ = cache::render_cached(
        &RenderContext { cwd: PathBuf::from("."), terminal_width: 120, status_code: 0, keymap: "vi".to_string() },
        None, &cached_config,
        &cache::compute_cache_key(Path::new("."), 0, "vi", 120, 0, 0),
        &mut lru,
    );

    let mut watcher = WatcherState::new();
    let mut handles = Vec::new();

    loop {
        handles.push(connect_event);
        handles.extend(watcher.change_events());
        let total = handles.len() as DWORD;
        let rc = unsafe { ffi::WaitForMultipleObjects(total, handles.as_ptr(), 0, u32::MAX) };
        if rc >= ffi::WAIT_OBJECT_0 && rc < ffi::WAIT_OBJECT_0 + total {
            watcher.process_signaled();
            if rc == ffi::WAIT_OBJECT_0 {
                let _ = handle_client(pipe, &mut config_path, &mut cached_config, &mut last_cfg_mtime, &mut lru, &mut watcher);
                rearm_connect(pipe, &mut connect_ol, connect_event);
            }
        }
        handles.clear();
    }
}

fn rearm_connect(pipe: HANDLE, ol: &mut ffi::OVERLAPPED, event: HANDLE) {
    unsafe {
        *ol = mem::zeroed();
        ol.h_event = event;
        ffi::ResetEvent(event);
        let ret = ffi::ConnectNamedPipe(pipe, ol as *mut _ as *mut c_void);
        if ret != 0 || ffi::GetLastError() == ERROR_PIPE_CONNECTED {
            ffi::SetEvent(event);
        }
    }
}

fn send_response(pipe: HANDLE, output: &str) {
    let b = output.as_bytes();
    let len_bytes = (b.len() as u32).to_le_bytes();
    write_all(pipe, &len_bytes);
    write_all(pipe, b);
    unsafe { ffi::FlushFileBuffers(pipe); ffi::DisconnectNamedPipe(pipe); }
}

fn handle_client(pipe: HANDLE, config_path: &mut PathBuf, cached_config: &mut toml::Table, last_cfg_mtime: &mut u64, lru: &mut LruCache<CacheKey, CachedValue>, watcher: &mut WatcherState) -> Result<(), ()> {
    let mut buf = [0u8; 4 + 32768 + 4 + 4096];
    if !read_exact(pipe, &mut buf[..4]) { pipe_error!(pipe); }
    let cwd_len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
    if cwd_len > 32768 { pipe_error!(pipe); }
    if !read_exact(pipe, &mut buf[4..4 + cwd_len]) { pipe_error!(pipe); }
    let props_start = 4 + cwd_len;
    if !read_exact(pipe, &mut buf[props_start..props_start + 4]) { pipe_error!(pipe); }
    let props_len = u32::from_le_bytes(buf[props_start..props_start + 4].try_into().unwrap()) as usize;
    if props_len > 4096 { pipe_error!(pipe); }
    let props_body_start = props_start + 4;
    if !read_exact(pipe, &mut buf[props_body_start..props_body_start + props_len]) { pipe_error!(pipe); }
    let total = props_body_start + props_len;
    let ParsedRequest { cwd, props } = match parse_request(&buf[..total]) {
        Some(r) => r,
        None => { pipe_error!(pipe); }
    };
    let git_dir = starship_daemon::find_git_dir(&cwd);
    let status_code = props.status_code.unwrap_or(0);
    let keymap = props.keymap.unwrap_or_else(|| "vi".to_string());

    if let Some(ref req) = props.starship_config {
        let p = PathBuf::from(req);
        if p != *config_path {
            if let Some(new_cfg) = cache::load_config(&p) {
                *config_path = new_cfg;
                unsafe { std::env::set_var("STARSHIP_CONFIG", req); }
            }
        }
    }

    let cur_cfg_mtime = {
        let mtime = cache::get_mtime_ns(config_path);
        if mtime != *last_cfg_mtime {
            *last_cfg_mtime = mtime;
            *cached_config = cache::read_config(config_path);
            lru.clear();
            cache::clear_repo_cache();
        }
        mtime
    };

    let tw = props.terminal_width.unwrap_or(120);

    if props.disable_cache.unwrap_or(false) {
        let ctx = RenderContext { cwd, terminal_width: tw, status_code, keymap };
        let output = cache::render_prompt_with_config(&ctx, git_dir.as_deref(), cached_config);
        send_response(pipe, &output);
        return Ok(());
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
    send_response(pipe, &output);
    Ok(())
}
