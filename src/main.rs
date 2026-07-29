use std::mem;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::ffi::c_void;

use lru::LruCache;
use starship_daemon::cache::{self, CacheKey, CachedValue, RenderContext};
use starship_daemon::ffi::{self, HANDLE, DWORD, LPVOID, LPCVOID};
use starship_daemon::watch::WatcherState;

const PIPE_ACCESS_DUPLEX: DWORD = 3;
const FILE_FLAG_OVERLAPPED: DWORD = 0x40000000;
const PIPE_TYPE_MESSAGE: DWORD = 4;
const PIPE_WAIT: DWORD = 0;
const ERROR_PIPE_CONNECTED: DWORD = 535;

macro_rules! bail { ($p:expr) => { unsafe { ffi::DisconnectNamedPipe($p); return Err(()); } } }

fn read_exact(pipe: HANDLE, buf: &mut [u8]) -> bool {
    unsafe {
        let mut avail: DWORD = 0;
        if ffi::PeekNamedPipe(pipe, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, &mut avail) != 0 {
            if avail > buf.len() as DWORD { return false; }
        }
        let mut r: DWORD = 0;
        ffi::ReadFile(pipe, buf.as_mut_ptr() as LPVOID, buf.len() as DWORD, &mut r, std::ptr::null_mut()) != 0
            && r == buf.len() as DWORD
    }
}

fn write_all(pipe: HANDLE, buf: &[u8]) -> bool {
    unsafe { let mut w: DWORD = 0; ffi::WriteFile(pipe, buf.as_ptr() as LPCVOID, buf.len() as DWORD, &mut w, std::ptr::null_mut()) != 0 }
}

struct ClientProps {
    status_code: Option<i32>,
    keymap: Option<String>,
    terminal_width: Option<usize>,
    starship_config: Option<String>,
    disable_cache: Option<bool>,
}

impl ClientProps {
    fn parse_json(data: &[u8]) -> Option<Self> {
        let s = std::str::from_utf8(data).ok()?;
        let s = s.trim().trim_start_matches('{').trim_end_matches('}');
        let mut status_code = None;
        let mut keymap = None;
        let mut terminal_width = None;
        let mut starship_config = None;
        let mut disable_cache = None;
        let mut i = 0;
        let bytes = s.as_bytes();
        while i < bytes.len() {
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b',' || bytes[i] == b'\t' || bytes[i] == b'\n') { i += 1; }
            if i >= bytes.len() { break; }
            if bytes[i] != b'"' { break; }
            i += 1;
            let ks = i; while i < bytes.len() && bytes[i] != b'"' { i += 1; }
            if i >= bytes.len() { break; }
            let key = std::str::from_utf8(&bytes[ks..i]).ok()?; i += 1;
            while i < bytes.len() && bytes[i] != b':' { i += 1; }
            if i >= bytes.len() { break; }
            i += 1;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') { i += 1; }
            if i >= bytes.len() { break; }
            if bytes[i] == b'"' {
                i += 1; let vs = i;
                while i < bytes.len() && bytes[i] != b'"' { i += 1; }
                let val = std::str::from_utf8(&bytes[vs..i]).ok()?.to_string(); i += 1;
                match key { "keymap" => keymap = Some(val), "starship_config" => starship_config = Some(val), _ => {} }
            } else if i + 3 < bytes.len() && &bytes[i..i+4] == b"null" { i += 4; }
            else if i + 3 < bytes.len() && &bytes[i..i+4] == b"true" { i += 4; match key { "disable_cache" => disable_cache = Some(true), _ => {} } }
            else if i + 4 < bytes.len() && &bytes[i..i+5] == b"false" { i += 5; match key { "disable_cache" => disable_cache = Some(false), _ => {} } }
            else {
                let vs = i; while i < bytes.len() && bytes[i] != b',' && bytes[i] != b'}' && bytes[i] != b' ' { i += 1; }
                let val = std::str::from_utf8(&bytes[vs..i]).ok()?;
                match key { "status_code" => status_code = val.parse::<i32>().ok(), "terminal_width" => terminal_width = val.parse::<usize>().ok(), _ => {} }
            }
        }
        Some(ClientProps { status_code, keymap, terminal_width, starship_config, disable_cache })
    }
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

    {
        let warm_ctx = RenderContext {
            cwd: PathBuf::from("."),
            terminal_width: 120,
            status_code: 0,
            keymap: "vi".to_string(),
        };
        let warm_key = cache::compute_cache_key(
            Path::new("."), 0, "vi", 120, &config_path, 0,
        );
        let _ = cache::render_cached(&warm_ctx, None, &cached_config, &warm_key, &mut lru);
    }

    let mut watcher = WatcherState::new();

    loop {
        let mut handles: Vec<HANDLE> = vec![connect_event];
        for w in &watcher.entries { handles.push(w.change_event); }
        let timeout: DWORD = if watcher.entries.is_empty() { u32::MAX } else { 100 };
        let total = handles.len() as DWORD;
        let rc = unsafe { ffi::WaitForMultipleObjects(total, handles.as_ptr(), 0, timeout) };
        if rc >= ffi::WAIT_OBJECT_0 && rc < ffi::WAIT_OBJECT_0 + total {
            let idx = rc - ffi::WAIT_OBJECT_0;
            if idx == 0 {
                let _ = handle_client(pipe, &mut config_path, &mut cached_config, &mut last_cfg_mtime, &mut lru, &mut watcher);
                rearm_connect(pipe, &mut connect_ol, connect_event);
            } else {
                watcher.handle_event((idx - 1) as usize);
            }
        } else if rc == ffi::WAIT_TIMEOUT {
            watcher.process_dirty();
        }
    }
}

fn rearm_connect(pipe: HANDLE, ol: &mut ffi::OVERLAPPED, event: HANDLE) {
    unsafe { *ol = mem::zeroed(); ol.h_event = event; ffi::ResetEvent(event); }
    let ret = unsafe { ffi::ConnectNamedPipe(pipe, ol as *mut _ as *mut c_void) };
    if ret == 0 { let err = unsafe { ffi::GetLastError() }; if err == ERROR_PIPE_CONNECTED { unsafe { ffi::SetEvent(event); } } }
    else { unsafe { ffi::SetEvent(event); } }
}

fn send_response(pipe: HANDLE, output: &str) {
    let b = output.as_bytes();
    let len_bytes = (b.len() as u32).to_le_bytes();
    write_all(pipe, &len_bytes);
    write_all(pipe, b);
    unsafe { ffi::FlushFileBuffers(pipe); ffi::DisconnectNamedPipe(pipe); }
}

fn handle_client(pipe: HANDLE, config_path: &mut PathBuf, cached_config: &mut toml::Table, last_cfg_mtime: &mut u64, lru: &mut LruCache<CacheKey, CachedValue>, watcher: &mut WatcherState) -> Result<(), ()> {
    let mut buf = [0u8; 8 + 32768];
    if !read_exact(pipe, &mut buf[..4]) { bail!(pipe); }
    let cwd_len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
    if cwd_len > 32768 { bail!(pipe); }
    if !read_exact(pipe, &mut buf[..cwd_len]) { bail!(pipe); }
    let cwd = PathBuf::from(String::from_utf8_lossy(&buf[..cwd_len]).as_ref());

    if !read_exact(pipe, &mut buf[..4]) { bail!(pipe); }
    let props_len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
    if props_len > 4096 { bail!(pipe); }
    if !read_exact(pipe, &mut buf[..props_len]) { bail!(pipe); }
    let props_bytes = &buf[..props_len];

    let git_dir = starship_daemon::find_git_dir(&cwd);
    let props: ClientProps = match ClientProps::parse_json(&props_bytes) { Some(p) => p, None => { bail!(pipe); } };
    let status_code = props.status_code.unwrap_or(0);
    let keymap = props.keymap.unwrap_or_else(|| "vi".to_string());

    if let Some(ref req) = props.starship_config {
        let p = PathBuf::from(req);
        if p != *config_path {
            if let Some(new_cfg) = cache::load_config(&p) {
                *config_path = new_cfg;
                lru.clear();
                cache::clear_repo_cache();
                unsafe { std::env::set_var("STARSHIP_CONFIG", req); }
                *cached_config = cache::read_config(config_path);
                *last_cfg_mtime = cache::get_mtime_ns(config_path);
            }
        }
    }

    let cur_cfg_mtime = cache::get_mtime_ns(config_path);
    if cur_cfg_mtime != *last_cfg_mtime {
        *last_cfg_mtime = cur_cfg_mtime;
        *cached_config = cache::read_config(config_path);
        lru.clear();
        cache::clear_repo_cache();
    }

    let tw = props.terminal_width.unwrap_or(120);

    if props.disable_cache.unwrap_or(false) {
        let ctx = RenderContext { cwd: cwd.clone(), terminal_width: tw, status_code, keymap };
        let output = cache::render_prompt_with_config(&ctx, git_dir.as_deref(), cached_config);
        send_response(pipe, &output);
        return Ok(());
    }

    let repo_root = git_dir.as_ref().and_then(|g| g.parent());
    if let Some(r) = repo_root { watcher.ensure(r); }
    watcher.poll();
    watcher.process_dirty();
    let watcher_gen = repo_root.map(|r| watcher.generation(r)).unwrap_or(0);
    let ck = cache::compute_cache_key(&cwd, status_code, &keymap, tw, config_path.as_path(), watcher_gen);
    let ctx = RenderContext { cwd: cwd.clone(), terminal_width: tw, status_code, keymap };

    let output = cache::render_cached(&ctx, git_dir.as_deref(), cached_config, &ck, lru);
    send_response(pipe, &output);
    Ok(())
}
