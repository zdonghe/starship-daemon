use std::collections::HashMap;
use std::ffi::c_void;
use std::mem;
use std::path::{Path, PathBuf};

use starship_daemon::prompt::{self, ModuleConfig, RenderContext};

// -- Win32 FFI for named pipe + config file watching -------------------

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
const FILE_LIST_DIRECTORY: DWORD = 1;
const FILE_SHARE_READ: DWORD = 1;
const FILE_SHARE_WRITE: DWORD = 2;
const FILE_SHARE_DELETE: DWORD = 4;
const OPEN_EXISTING: DWORD = 3;
const FILE_FLAG_BACKUP_SEMANTICS: DWORD = 0x02000000;
const FILE_NOTIFY_CHANGE_FILE_NAME: DWORD = 1;
const FILE_NOTIFY_CHANGE_LAST_WRITE: DWORD = 0x10;
const WAIT_OBJECT_0: DWORD = 0;
const WAIT_TIMEOUT: DWORD = 0x00000102;
const ERROR_PIPE_CONNECTED: DWORD = 535;
const CHANGE_BUF_SIZE: u32 = 65536;

#[repr(C)]
struct OVERLAPPED {
    internal: usize, internal_high: usize, offset: DWORD, offset_high: DWORD, h_event: HANDLE,
}

extern "system" {
    fn CreateNamedPipeW(name: LPCWSTR, open_mode: DWORD, pipe_mode: DWORD, max_inst: DWORD, out_buf: DWORD, in_buf: DWORD, timeout: DWORD, sec: *const c_void) -> HANDLE;
    fn ConnectNamedPipe(h: HANDLE, overlapped: *mut c_void) -> BOOL;
    fn DisconnectNamedPipe(h: HANDLE) -> BOOL;
    fn ReadFile(h: HANDLE, buf: LPVOID, len: DWORD, read: LPDWORD, overlapped: *mut c_void) -> BOOL;
    fn WriteFile(h: HANDLE, buf: LPCVOID, len: DWORD, written: LPDWORD, overlapped: *mut c_void) -> BOOL;
    fn CloseHandle(h: HANDLE) -> BOOL;
    fn CreateFileW(name: LPCWSTR, access: DWORD, share: DWORD, sec: *const c_void, disp: DWORD, flags: DWORD, tmpl: HANDLE) -> HANDLE;
    fn ReadDirectoryChangesW(dir: HANDLE, buf: LPVOID, len: DWORD, subtree: BOOL, filter: DWORD, bytes: LPDWORD, overlapped: *mut c_void, comp: *const c_void) -> BOOL;
    fn CreateEventW(attr: *const c_void, manual: BOOL, init: BOOL, name: LPCWSTR) -> HANDLE;
    fn WaitForMultipleObjects(count: DWORD, handles: *const HANDLE, wait_all: BOOL, ms: DWORD) -> DWORD;
    fn GetOverlappedResult(h: HANDLE, overlapped: *mut c_void, bytes: LPDWORD, wait: BOOL) -> BOOL;
    fn ResetEvent(h: HANDLE) -> BOOL;
    fn SetEvent(h: HANDLE) -> BOOL;
    fn GetLastError() -> DWORD;
    fn GetProcessHeap() -> HANDLE;
    fn HeapAlloc(heap: HANDLE, flags: DWORD, size: usize) -> LPVOID;
    fn HeapFree(heap: HANDLE, flags: DWORD, mem: LPVOID) -> BOOL;
    fn WaitForSingleObject(h: HANDLE, ms: DWORD) -> DWORD;
}

fn to_wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }
fn alloc_buf() -> *mut u8 { unsafe { HeapAlloc(GetProcessHeap(), 0, CHANGE_BUF_SIZE as usize) as *mut u8 } }
fn free_buf(p: *mut u8) { unsafe { HeapFree(GetProcessHeap(), 0, p as LPVOID); } }
fn read_exact(pipe: HANDLE, buf: &mut [u8]) -> bool {
    unsafe { let mut r: DWORD = 0; ReadFile(pipe, buf.as_mut_ptr() as LPVOID, buf.len() as DWORD, &mut r, std::ptr::null_mut()) != 0 && r == buf.len() as DWORD }
}
fn write_all(pipe: HANDLE, buf: &[u8]) -> bool {
    unsafe { let mut w: DWORD = 0; WriteFile(pipe, buf.as_ptr() as LPCVOID, buf.len() as DWORD, &mut w, std::ptr::null_mut()) != 0 }
}

// -- Client request properties (manual JSON parser) -------------------

struct ClientProps {
    status_code: Option<i32>,
    keymap: Option<String>,
    terminal_width: Option<usize>,
    starship_config: Option<String>,
}

impl ClientProps {
    fn parse_json(data: &[u8]) -> Option<Self> {
        let s = std::str::from_utf8(data).ok()?;
        let s = s.trim().trim_start_matches('{').trim_end_matches('}');
        let mut status_code = None;
        let mut keymap = None;
        let mut terminal_width = None;
        let mut starship_config = None;
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
            else {
                let vs = i; while i < bytes.len() && bytes[i] != b',' && bytes[i] != b'}' && bytes[i] != b' ' { i += 1; }
                let val = std::str::from_utf8(&bytes[vs..i]).ok()?;
                match key { "status_code" => status_code = val.parse::<i32>().ok(), "terminal_width" => terminal_width = val.parse::<usize>().ok(), _ => {} }
            }
        }
        Some(ClientProps { status_code, keymap, terminal_width, starship_config })
    }
}

// -- Prompt cache --------------------------------------------------------

#[derive(Hash, Eq, PartialEq, Clone)]
struct CacheKey {
    cwd: PathBuf,
    status_code: i32,
    keymap: String,
    time_bucket: u64,
}

// -- Config file watching ------------------------------------------------

struct ConfigWatch {
    dir_handle: HANDLE, change_buf: *mut u8, overlapped: OVERLAPPED, change_event: HANDLE, config_path: PathBuf,
}

impl ConfigWatch {
    fn new(config_path: &Path) -> Option<Self> {
        let dir = config_path.parent()?;
        let wide = to_wide(&dir.to_string_lossy());
        unsafe {
            let dh = CreateFileW(wide.as_ptr(), FILE_LIST_DIRECTORY, FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_OVERLAPPED|FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut());
            if dh == INVALID_HANDLE_VALUE { return None; }
            let ev = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
            if ev.is_null() { CloseHandle(dh); return None; }
            let buf = alloc_buf();
            if buf.is_null() { CloseHandle(dh); CloseHandle(ev); return None; }
            let mut cw = ConfigWatch { dir_handle: dh, change_buf: buf, overlapped: mem::zeroed(), change_event: ev, config_path: config_path.to_path_buf() };
            cw.start(); Some(cw)
        }
    }
    fn start(&mut self) {
        unsafe {
            ResetEvent(self.change_event); self.overlapped = mem::zeroed(); self.overlapped.h_event = self.change_event;
            let mut b: DWORD = 0;
            ReadDirectoryChangesW(self.dir_handle, self.change_buf as LPVOID, CHANGE_BUF_SIZE, 0, FILE_NOTIFY_CHANGE_FILE_NAME|FILE_NOTIFY_CHANGE_LAST_WRITE, &mut b, &mut self.overlapped as *mut _ as *mut c_void, std::ptr::null());
        }
    }
    fn check_event(&mut self) -> bool {
        if self.change_event.is_null() { return false; }
        if unsafe { WaitForSingleObject(self.change_event, 0) } != WAIT_OBJECT_0 { return false; }
        unsafe { let mut b: DWORD = 0; GetOverlappedResult(self.dir_handle, &mut self.overlapped as *mut _ as *mut c_void, &mut b, 0); }
        let cname = self.config_path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        // Check if our config file changed
        let raw = unsafe { std::slice::from_raw_parts(self.change_buf, CHANGE_BUF_SIZE as usize) };
        let changed = raw.windows(cname.len()).any(|w| w == cname.as_bytes());
        self.start();
        changed
    }
}
impl Drop for ConfigWatch {
    fn drop(&mut self) {
        unsafe {
            if self.dir_handle != INVALID_HANDLE_VALUE { CloseHandle(self.dir_handle); }
            if self.dir_handle != INVALID_HANDLE_VALUE { CloseHandle(self.dir_handle); }
            if !self.change_buf.is_null() { free_buf(self.change_buf); }
            CloseHandle(self.change_event);
        }
    }
}

fn main() {
    let config_path = prompt::default_config_path();
    let mut module_config = match prompt::load_config(&config_path) {
        Some(cfg) => cfg,
        None => { eprintln!("Could not load config"); std::process::exit(1); }
    };
    let mut config_watch = ConfigWatch::new(&config_path);
    let mut prompt_cache: HashMap<CacheKey, String> = HashMap::new();
    let wide = to_wide(starship_daemon::PIPE_NAME);
    let pipe = unsafe { CreateNamedPipeW(wide.as_ptr(), PIPE_ACCESS_DUPLEX|FILE_FLAG_OVERLAPPED, PIPE_TYPE_MESSAGE|PIPE_WAIT, 1, 65536, 65536, 0, std::ptr::null()) };
    if pipe == INVALID_HANDLE_VALUE { std::process::exit(0); }
    let connect_event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    let mut connect_ol: OVERLAPPED = unsafe { mem::zeroed() };

    // Issue initial connect
    unsafe { *&mut connect_ol = mem::zeroed(); connect_ol.h_event = connect_event; ResetEvent(connect_event); }
    let ret = unsafe { ConnectNamedPipe(pipe, &mut connect_ol as *mut _ as *mut c_void) };
    if ret == 0 { let err = unsafe { GetLastError() }; if err == ERROR_PIPE_CONNECTED { unsafe { SetEvent(connect_event); } } }
    else { unsafe { SetEvent(connect_event); } }
    println!("starship-daemon started on {}", starship_daemon::PIPE_NAME);
    loop {
        let config_evt = config_watch.as_ref().map_or(std::ptr::null_mut(), |cw| cw.change_event);
        let handles = [connect_event, config_evt];
        let rc = unsafe { WaitForMultipleObjects(handles.len() as DWORD, handles.as_ptr(), 0, 100) };
        if rc == WAIT_TIMEOUT { continue; }
        let idx = (rc - WAIT_OBJECT_0) as usize;
        if idx == 0 {
            let _ = handle_client(pipe, &mut module_config, &mut prompt_cache);
            unsafe { *&mut connect_ol = mem::zeroed(); connect_ol.h_event = connect_event; ResetEvent(connect_event); }
            let ret = unsafe { ConnectNamedPipe(pipe, &mut connect_ol as *mut _ as *mut c_void) };
            if ret == 0 { let err = unsafe { GetLastError() }; if err == ERROR_PIPE_CONNECTED { unsafe { SetEvent(connect_event); } } }
            else { unsafe { SetEvent(connect_event); } }
        } else if idx == 1 {
            if let Some(ref mut cw) = config_watch {
                if cw.check_event() {
                    if let Some(new_cfg) = prompt::load_config(&config_path) { module_config = new_cfg; prompt_cache.clear(); }
                }
            }
        }
    }
}

fn handle_client(pipe: HANDLE, module_config: &mut ModuleConfig, prompt_cache: &mut HashMap<CacheKey, String>) -> Result<(), ()> {
    let mut hdr = [0u8; 4];
    if !read_exact(pipe, &mut hdr) || u32::from_le_bytes(hdr) as usize > 32768 { unsafe { DisconnectNamedPipe(pipe); } return Err(()); }
    let cwd_len = u32::from_le_bytes(hdr) as usize;
    let mut cwd_bytes = vec![0u8; cwd_len];
    if !read_exact(pipe, &mut cwd_bytes) { unsafe { DisconnectNamedPipe(pipe); } return Err(()); }
    if !read_exact(pipe, &mut hdr) || u32::from_le_bytes(hdr) as usize > 4096 { unsafe { DisconnectNamedPipe(pipe); } return Err(()); }
    let props_len = u32::from_le_bytes(hdr) as usize;
    let mut props_bytes = vec![0u8; props_len];
    if !read_exact(pipe, &mut props_bytes) { unsafe { DisconnectNamedPipe(pipe); } return Err(()); }
    let cwd = PathBuf::from(String::from_utf8_lossy(&cwd_bytes).as_ref());
    let props: ClientProps = match ClientProps::parse_json(&props_bytes) { Some(p) => p, None => { unsafe { DisconnectNamedPipe(pipe); } return Err(()); } };
    let status_code = props.status_code.unwrap_or(0);
    let keymap = props.keymap.unwrap_or_else(|| "vi".to_string());
    if let Some(ref req) = props.starship_config {
        let p = PathBuf::from(req);
        if p != module_config.config_path {
            if let Some(new_cfg) = prompt::load_config(&p) { *module_config = new_cfg; prompt_cache.clear(); std::env::set_var("STARSHIP_CONFIG", req); }
        }
    }
    let tb = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 300).unwrap_or(0);
    let ck = CacheKey { cwd: cwd.clone(), status_code, keymap: keymap.clone(), time_bucket: tb };
    if let Some(cached) = prompt_cache.get(&ck) {
        let b = cached.as_bytes(); let l = (b.len() as u32).to_le_bytes();
        write_all(pipe, &l); write_all(pipe, b);
        unsafe { let mut d = [0u8; 4]; let mut r: DWORD = 0; ReadFile(pipe, d.as_mut_ptr() as LPVOID, 4, &mut r, std::ptr::null_mut()); DisconnectNamedPipe(pipe); }
        return Ok(());
    }
    // Pre-warm OS/Defender cache: read .git files so gix doesn't wait for Defender
    let git_dir = cwd.join(".git");
    let _ = std::fs::read(git_dir.join("HEAD"));
    let _ = std::fs::read(git_dir.join("index"));

    let ctx = RenderContext { cwd: cwd.clone(), terminal_width: props.terminal_width.unwrap_or(120), status_code, keymap };
    let output = prompt::render_prompt(&ctx);
    prompt_cache.insert(ck, output.clone());
    let b = output.as_bytes(); let l = (b.len() as u32).to_le_bytes();
    write_all(pipe, &l); write_all(pipe, b);
    unsafe { let mut d = [0u8; 4]; let mut r: DWORD = 0; ReadFile(pipe, d.as_mut_ptr() as LPVOID, 4, &mut r, std::ptr::null_mut()); DisconnectNamedPipe(pipe); }
    Ok(())
}
