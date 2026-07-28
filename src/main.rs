use std::collections::HashMap;
use std::ffi::c_void;
use std::mem;
use std::path::PathBuf;
use starship_daemon::prompt::{self, RenderContext};

type HANDLE = *mut c_void;
type DWORD = u32;
type BOOL = i32;
type LPCWSTR = *const u16;
type LPVOID = *mut c_void;
type LPCVOID = *const c_void;
type LPDWORD = *mut u32;

const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;
const PIPE_ACCESS_DUPLEX: DWORD = 3;
const FILE_FLAG_OVERLAPPED: DWORD = 0x40000000;
const PIPE_TYPE_MESSAGE: DWORD = 4;
const PIPE_WAIT: DWORD = 0;
const WAIT_OBJECT_0: DWORD = 0;
const ERROR_PIPE_CONNECTED: DWORD = 535;
const CACHE_TTL_MINUTES: u64 = 2;
const CACHE_MAX_ENTRIES: usize = 10000;

#[repr(C)]
struct OVERLAPPED {
    internal: usize, internal_high: usize, offset: DWORD, offset_high: DWORD, h_event: HANDLE,
}

unsafe extern "system" {
    fn CreateNamedPipeW(name: LPCWSTR, open_mode: DWORD, pipe_mode: DWORD, max_inst: DWORD, out_buf: DWORD, in_buf: DWORD, timeout: DWORD, sec: *const c_void) -> HANDLE;
    fn ConnectNamedPipe(h: HANDLE, overlapped: *mut c_void) -> BOOL;
    fn DisconnectNamedPipe(h: HANDLE) -> BOOL;
    fn ReadFile(h: HANDLE, buf: LPVOID, len: DWORD, read: LPDWORD, overlapped: *mut c_void) -> BOOL;
    fn WriteFile(h: HANDLE, buf: LPCVOID, len: DWORD, written: LPDWORD, overlapped: *mut c_void) -> BOOL;
    fn CreateEventW(attr: *const c_void, manual: BOOL, init: BOOL, name: LPCWSTR) -> HANDLE;
    fn WaitForMultipleObjects(count: DWORD, handles: *const HANDLE, wait_all: BOOL, ms: DWORD) -> DWORD;
    fn ResetEvent(h: HANDLE) -> BOOL;
    fn SetEvent(h: HANDLE) -> BOOL;
    fn FlushFileBuffers(h: HANDLE) -> BOOL;
    fn GetLastError() -> DWORD;
}

macro_rules! bail { ($p:expr) => { unsafe { DisconnectNamedPipe($p); return Err(()); } } }

fn to_wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }
fn read_exact(pipe: HANDLE, buf: &mut [u8]) -> bool {
    // Named pipe in message-mode: ReadFile returns exactly one message when data available.
    // read_exact is needed because the 3 client writes (cwd, props_len, props_data) arrive
    // as separate messages. A single large ReadFile would block until the full amount arrives.
    unsafe { let mut r: DWORD = 0; ReadFile(pipe, buf.as_mut_ptr() as LPVOID, buf.len() as DWORD, &mut r, std::ptr::null_mut()) != 0 && r == buf.len() as DWORD }
}

fn write_all(pipe: HANDLE, buf: &[u8]) -> bool {
    unsafe { let mut w: DWORD = 0; WriteFile(pipe, buf.as_ptr() as LPCVOID, buf.len() as DWORD, &mut w, std::ptr::null_mut()) != 0 }
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
    let config_path = prompt::default_config_path();
    let mut config_path = match prompt::load_config(&config_path) {
        Some(p) => p,
        None => { eprintln!("Could not load config"); std::process::exit(1); }
    };
    let mut prompt_cache: HashMap<prompt::CacheKey, String> = HashMap::new();
    let mut cached_config = prompt::read_config(&config_path);
    let mut last_cfg_mtime = prompt::get_mtime_ns(&config_path);
    let wide = to_wide(starship_daemon::PIPE_NAME);
    let pipe = unsafe { CreateNamedPipeW(wide.as_ptr(), PIPE_ACCESS_DUPLEX|FILE_FLAG_OVERLAPPED, PIPE_TYPE_MESSAGE|PIPE_WAIT, 1, 65536, 65536, 0, std::ptr::null()) };
    if pipe == INVALID_HANDLE_VALUE { std::process::exit(0); }
    let connect_event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    let mut connect_ol: OVERLAPPED = unsafe { mem::zeroed() };

    rearm_connect(pipe, &mut connect_ol, connect_event);
    println!("starship-daemon started on {}", starship_daemon::PIPE_NAME);

    {
        let warm_ctx = RenderContext {
            cwd: PathBuf::from("."),
            terminal_width: 120,
            status_code: 0,
            keymap: "vi".to_string(),
        };
        let _ = prompt::render_prompt_with_config(&warm_ctx, None, &cached_config);
    }

    loop {
        let handles = [connect_event];
        let rc = unsafe { WaitForMultipleObjects(1, handles.as_ptr(), 0, 0xFFFFFFFF) };
        if (rc - WAIT_OBJECT_0) == 0 {
            let _ = handle_client(pipe, &mut config_path, &mut cached_config, &mut last_cfg_mtime, &mut prompt_cache);
            rearm_connect(pipe, &mut connect_ol, connect_event);
        }
    }
}


fn current_minute() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 60).unwrap_or(0)
}
fn evict_stale(cache: &mut HashMap<prompt::CacheKey, String>) {
    let cutoff = current_minute().saturating_sub(CACHE_TTL_MINUTES);
    cache.retain(|k, _| k.time_bucket > cutoff);
}

fn rearm_connect(pipe: HANDLE, ol: &mut OVERLAPPED, event: HANDLE) {
    unsafe { *ol = mem::zeroed(); ol.h_event = event; ResetEvent(event); }
    let ret = unsafe { ConnectNamedPipe(pipe, ol as *mut _ as *mut c_void) };
    if ret == 0 { let err = unsafe { GetLastError() }; if err == ERROR_PIPE_CONNECTED { unsafe { SetEvent(event); } } }
    else { unsafe { SetEvent(event); } }
}

fn send_response(pipe: HANDLE, output: &str) {
    let b = output.as_bytes(); let l = (b.len() as u32).to_le_bytes();
    let mut buf = [0u8; 4 + 65536];
    buf[..4].copy_from_slice(&l);
    buf[4..4+b.len()].copy_from_slice(b);
    write_all(pipe, &buf[..4+b.len()]);
    unsafe { FlushFileBuffers(pipe); DisconnectNamedPipe(pipe); }
}

fn handle_client(pipe: HANDLE, config_path: &mut PathBuf, cached_config: &mut toml::Table, last_cfg_mtime: &mut u64, prompt_cache: &mut HashMap<prompt::CacheKey, String>) -> Result<(), ()> {
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
            if let Some(new_cfg) = prompt::load_config(&p) {
                *config_path = new_cfg;
                prompt_cache.clear();
                unsafe { std::env::set_var("STARSHIP_CONFIG", req); }
                *cached_config = prompt::read_config(config_path);
                *last_cfg_mtime = prompt::get_mtime_ns(config_path);
            }
        }
    }

    let cur_cfg_mtime = prompt::get_mtime_ns(config_path);
    if cur_cfg_mtime != *last_cfg_mtime {
        *last_cfg_mtime = cur_cfg_mtime;
        *cached_config = prompt::read_config(config_path);
        prompt_cache.clear();
    }

    let tw = props.terminal_width.unwrap_or(120);

    if props.disable_cache.unwrap_or(false) {
        let ctx = RenderContext { cwd: cwd.clone(), terminal_width: tw, status_code, keymap };
        let output = prompt::render_prompt_with_config(&ctx, git_dir.as_deref(), cached_config);
        send_response(pipe, &output);
        return Ok(());
    }

    let tb = current_minute();
    let ck = prompt::compute_cache_key(&cwd, status_code, &keymap, tw, tb, config_path, git_dir.as_deref());
    let ctx = RenderContext { cwd: cwd.clone(), terminal_width: tw, status_code, keymap };

    if let Some(cached) = prompt_cache.get(&ck) {
        send_response(pipe, cached);
        return Ok(());
    }

    let output = prompt::render_prompt_with_config(&ctx, git_dir.as_deref(), cached_config);
    prompt_cache.insert(ck, output.clone());
    if prompt_cache.len() >= CACHE_MAX_ENTRIES { evict_stale(prompt_cache); }
    send_response(pipe, &output);
    Ok(())
}
