use std::ffi::c_void;

pub type HANDLE = *mut c_void;
pub type DWORD = u32;
pub type BOOL = i32;
pub type LPCWSTR = *const u16;
pub type LPVOID = *mut c_void;
pub type LPCVOID = *const c_void;
pub type LPDWORD = *mut u32;

pub const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;

pub const WAIT_OBJECT_0: DWORD = 0;
pub const WAIT_TIMEOUT: DWORD = 0x00000102;
pub const WAIT_FAILED: DWORD = 0xFFFFFFFF;
pub const INFINITE: DWORD = 0xFFFFFFFF;

pub const ERROR_NOT_FOUND: DWORD = 1168;
pub const ERROR_IO_INCOMPLETE: DWORD = 996;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OVERLAPPED {
    pub internal: usize,
    pub internal_high: usize,
    pub offset: DWORD,
    pub offset_high: DWORD,
    pub h_event: HANDLE,
}

pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn to_wide_path(p: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

unsafe extern "system" {
    pub fn CreateEventW(attr: *const c_void, manual: BOOL, init: BOOL, name: LPCWSTR) -> HANDLE;
    pub fn CloseHandle(h: HANDLE) -> BOOL;
    pub fn ResetEvent(h: HANDLE) -> BOOL;
    pub fn SetEvent(h: HANDLE) -> BOOL;
    pub fn CreateFileW(name: LPCWSTR, access: DWORD, share: DWORD, sec: *const c_void, disp: DWORD, flags: DWORD, tmpl: HANDLE) -> HANDLE;
    pub fn ReadDirectoryChangesW(dir: HANDLE, buf: LPVOID, len: DWORD, subtree: BOOL, filter: DWORD, bytes: LPDWORD, overlapped: *mut c_void, comp: *const c_void) -> BOOL;
    pub fn GetOverlappedResult(h: HANDLE, overlapped: *mut c_void, bytes: LPDWORD, wait: BOOL) -> BOOL;
    pub fn CreateNamedPipeW(name: LPCWSTR, open_mode: DWORD, pipe_mode: DWORD, max_inst: DWORD, out_buf: DWORD, in_buf: DWORD, timeout: DWORD, sec: *const c_void) -> HANDLE;
    pub fn ConnectNamedPipe(h: HANDLE, overlapped: *mut c_void) -> BOOL;
    pub fn DisconnectNamedPipe(h: HANDLE) -> BOOL;
    pub fn ReadFile(h: HANDLE, buf: LPVOID, len: DWORD, read: LPDWORD, overlapped: *mut c_void) -> BOOL;
    pub fn WriteFile(h: HANDLE, buf: LPCVOID, len: DWORD, written: LPDWORD, overlapped: *mut c_void) -> BOOL;
    pub fn WaitForMultipleObjects(count: DWORD, handles: *const HANDLE, wait_all: BOOL, ms: DWORD) -> DWORD;
    pub fn WaitForSingleObject(handle: HANDLE, ms: DWORD) -> DWORD;
    pub fn GetLastError() -> DWORD;
    pub fn FlushFileBuffers(h: HANDLE) -> BOOL;
    pub fn PeekNamedPipe(h: HANDLE, buf: LPVOID, buf_size: DWORD, bytes_read: LPDWORD, total_avail: LPDWORD, bytes_left: LPDWORD) -> BOOL;
    pub fn CancelIoEx(h: HANDLE, overlapped: *mut c_void) -> BOOL;
    pub fn SetNamedPipeHandleState(h: HANDLE, mode: LPDWORD, max_collect: LPDWORD, timeout: LPDWORD) -> BOOL;
}
