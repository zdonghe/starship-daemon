use std::ffi::c_void;
use std::mem;
use std::path::PathBuf;
use std::time::Instant;
use std::time::Duration;

type HANDLE = *mut c_void;
type DWORD = u32;
type BOOL = i32;
type LPCWSTR = *const u16;
type LPVOID = *mut c_void;
type LPCVOID = *const c_void;
type LPDWORD = *mut u32;

const PIPE_ACCESS_DUPLEX: DWORD = 3;
const PIPE_TYPE_MESSAGE: DWORD = 4;
const PIPE_READMODE_MESSAGE: DWORD = 2;
const PIPE_WAIT: DWORD = 0;
const NMPWAIT_USE_DEFAULT_WAIT: DWORD = 0;
const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;

unsafe extern "system" {
    fn CreateFileW(name: LPCWSTR, access: DWORD, share: DWORD, sec: *const c_void, disp: DWORD, flags: DWORD, tmpl: HANDLE) -> HANDLE;
    fn ReadFile(h: HANDLE, buf: LPVOID, len: DWORD, read: LPDWORD, overlapped: *mut c_void) -> BOOL;
    fn WriteFile(h: HANDLE, buf: LPCVOID, len: DWORD, written: LPDWORD, overlapped: *mut c_void) -> BOOL;
    fn WaitNamedPipeW(name: LPCWSTR, timeout: DWORD) -> BOOL;
    fn CloseHandle(h: HANDLE) -> BOOL;
    fn GetLastError() -> DWORD;
}

fn to_wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }

fn connect_pipe(name: &str) -> HANDLE {
    let wide = to_wide(name);
    let GENERIC_READ = 0x80000000u32;
    let GENERIC_WRITE = 0x40000000u32;
    let OPEN_EXISTING = 3u32;
    let FILE_FLAG_OVERLAPPED = 0x40000000u32;
    loop {
        unsafe {
            let ok = WaitNamedPipeW(wide.as_ptr(), NMPWAIT_USE_DEFAULT_WAIT);
            if ok == 0 { std::thread::sleep(Duration::from_millis(10)); continue; }
            let h = CreateFileW(wide.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_OVERLAPPED, std::ptr::null_mut());
            if h != INVALID_HANDLE_VALUE { return h; }
            let err = GetLastError();
            if err == 2 { std::thread::sleep(Duration::from_millis(10)); continue; }
            panic!("CreateFile failed: {}", err);
        }
    }
}

fn send_req(pipe: HANDLE, cwd: &str, props: &str) -> String {
    let cwd_b = cwd.as_bytes();
    let props_b = props.as_bytes();
    unsafe {
        let len1 = (cwd_b.len() as u32).to_le_bytes();
        let len2 = (props_b.len() as u32).to_le_bytes();
        let mut written: DWORD = 0;
        WriteFile(pipe, len1.as_ptr() as LPCVOID, 4, &mut written, std::ptr::null_mut());
        WriteFile(pipe, cwd_b.as_ptr() as LPCVOID, cwd_b.len() as DWORD, &mut written, std::ptr::null_mut());
        WriteFile(pipe, len2.as_ptr() as LPCVOID, 4, &mut written, std::ptr::null_mut());
        WriteFile(pipe, props_b.as_ptr() as LPCVOID, props_b.len() as DWORD, &mut written, std::ptr::null_mut());
        let mut hdr = [0u8; 4];
        let mut read: DWORD = 0;
        ReadFile(pipe, hdr.as_mut_ptr() as LPVOID, 4, &mut read, std::ptr::null_mut());
        let rlen = u32::from_le_bytes(hdr) as usize;
        let mut buf = vec![0u8; rlen];
        ReadFile(pipe, buf.as_mut_ptr() as LPVOID, rlen as DWORD, &mut read, std::ptr::null_mut());
        String::from_utf8_lossy(&buf[..read as usize]).to_string()
    }
}

fn main() {
    let pipe_name = r"\\.\pipe\starship-daemon";
    let cwd = std::env::current_dir().unwrap();
    let cwd_s = cwd.to_str().unwrap();
    let props = r#"{"status_code":0,"keymap":"viins","terminal_width":120}"#;

    // Wait for daemon to be running
    println!("Connecting to daemon at {} ...", pipe_name);
    let pipe = connect_pipe(pipe_name);
    println!("Connected.\n");

    // Cold (first) request
    let start = Instant::now();
    let output = send_req(pipe, cwd_s, props);
    let cold = start.elapsed();
    println!("cold request (first): {:>8.1} ms  ({} bytes)", cold.as_secs_f64() * 1000.0, output.len());

    // Warm (cached) requests
    let mut times = Vec::new();
    for i in 0..20 {
        // reconnect for each request (daemon disconnects after serving)
        let pipe2 = connect_pipe(pipe_name);
        let start = Instant::now();
        let _ = send_req(pipe2, cwd_s, props);
        let d = start.elapsed();
        unsafe { CloseHandle(pipe2); }
        times.push(d);
        println!("  request {:>2}: {:>8.1} ms", i + 1, d.as_secs_f64() * 1000.0);
    }

    let avg = times.iter().sum::<Duration>() / times.len() as u32;
    let min = times.iter().min().unwrap();
    let max = times.iter().max().unwrap();
    println!("\nwarm (cached) --- min: {:>8.1} ms  avg: {:>8.1} ms  max: {:>8.1} ms",
        min.as_secs_f64() * 1000.0, avg.as_secs_f64() * 1000.0, max.as_secs_f64() * 1000.0);
    println!("cold first request: {:>8.1} ms", cold.as_secs_f64() * 1000.0);

    unsafe { CloseHandle(pipe); }
}
