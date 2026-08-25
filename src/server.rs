use std::ffi::c_void;
use std::mem;
#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use starship_daemon::daemon::DaemonState;
use starship_daemon::ffi::{self, DWORD, HANDLE, LPCVOID, LPVOID};
use starship_daemon::watch::MAX_WATCHED_REPOS;
use starship_daemon::{HEADER_LEN, MAX_FRAME_LEN, MAX_TOTAL_LEN};

const PIPE_ACCESS_DUPLEX: DWORD = 3;
const FILE_FLAG_OVERLAPPED: DWORD = 0x40000000;
const PIPE_TYPE_MESSAGE: DWORD = 4;
const PIPE_WAIT: DWORD = 0;
const ERROR_PIPE_CONNECTED: DWORD = 535;
const ERROR_IO_PENDING: DWORD = 997;
const PIPE_READMODE_BYTE: DWORD = 0;
const MAX_SESSIONS: usize = 9;
const MAX_REQS_PER_WAKE: u32 = 8;
const _: () = assert!(MAX_SESSIONS + MAX_WATCHED_REPOS <= 64);

#[derive(Clone, Copy)]
enum ReadStage {
    Header,
    Body,
}

enum SessionOp {
    Connect,
    ReadPending,
    ReadBuffered(usize),
    WritePending,
    Idle,
}

struct Session {
    pipe: HANDLE,
    ol: ffi::OVERLAPPED,
    event: HANDLE,
    buf: [u8; MAX_FRAME_LEN],
    connected: bool,
    op: SessionOp,
    write_buf: Vec<u8>,
    stage: ReadStage,
    want: usize,
    got: usize,
    last_activity: Instant,
}

impl Session {
    fn new(pipe: HANDLE, event: HANDLE) -> Self {
        Session {
            pipe,
            ol: unsafe { mem::zeroed() },
            event,
            buf: [0u8; MAX_FRAME_LEN],
            connected: false,
            op: SessionOp::Connect,
            write_buf: Vec::new(),
            stage: ReadStage::Header,
            want: HEADER_LEN,
            got: 0,
            last_activity: Instant::now(),
        }
    }

    fn reset_read_state(&mut self) {
        self.stage = ReadStage::Header;
        self.want = HEADER_LEN;
        self.got = 0;
    }
}

enum ReadIssue {
    Buffered(usize),
    Pending,
    Failed,
}

enum WriteIssue {
    Completed,
    Pending,
    Failed,
}

fn issue_read_at(s: &mut Session, offset: usize, count: usize) -> ReadIssue {
    unsafe {
        debug_assert!(offset + count <= s.buf.len());
        let ol = begin_op(s);
        let mut read: DWORD = 0;
        let ret = ffi::ReadFile(
            s.pipe,
            s.buf.as_mut_ptr().add(offset) as LPVOID,
            count as DWORD,
            &mut read,
            ol,
        );
        if ret != 0 {
            if read == 0 {
                return ReadIssue::Failed;
            }
            return ReadIssue::Buffered(read as usize);
        }
        if ffi::GetLastError() == ERROR_IO_PENDING {
            s.op = SessionOp::ReadPending;
            return ReadIssue::Pending;
        }
        ReadIssue::Failed
    }
}

fn begin_op(s: &mut Session) -> *mut c_void {
    unsafe {
        s.ol = mem::zeroed();
        s.ol.h_event = s.event;
        ffi::ResetEvent(s.event);
        &mut s.ol as *mut _ as *mut c_void
    }
}

fn complete_op(s: &mut Session, expected: Option<usize>) -> Option<u32> {
    unsafe {
        let mut bytes: DWORD = 0;
        let ok =
            ffi::GetOverlappedResult(s.pipe, &mut s.ol as *mut _ as *mut c_void, &mut bytes, 0);
        if ok != 0 {
            if let Some(len) = expected
                && bytes != len as DWORD
            {
                return Some(0);
            }
            return Some(bytes);
        }
        if ffi::GetLastError() == ffi::ERROR_IO_INCOMPLETE {
            return None;
        }
        Some(0)
    }
}

fn complete_read(s: &mut Session) -> Option<u32> {
    complete_op(s, None)
}

fn complete_write(s: &mut Session) -> Option<u32> {
    complete_op(s, Some(s.write_buf.len()))
}

fn issue_write(s: &mut Session) -> WriteIssue {
    unsafe {
        let ol = begin_op(s);
        let mut w: DWORD = 0;
        let ret = ffi::WriteFile(
            s.pipe,
            s.write_buf.as_ptr() as LPCVOID,
            s.write_buf.len() as DWORD,
            &mut w,
            ol,
        );
        if ret != 0 {
            return WriteIssue::Completed;
        }
        if ffi::GetLastError() == ERROR_IO_PENDING {
            s.op = SessionOp::WritePending;
            return WriteIssue::Pending;
        }
        WriteIssue::Failed
    }
}

fn rearm_connect(s: &mut Session) {
    s.op = SessionOp::Connect;
    unsafe {
        let ol = begin_op(s);
        let ret = ffi::ConnectNamedPipe(s.pipe, ol);
        if ret != 0 || ffi::GetLastError() == ERROR_PIPE_CONNECTED {
            ffi::SetEvent(s.event);
        }
    }
}

fn disconnect_session(s: &mut Session, reason: DisconnectReason) {
    #[cfg(not(debug_assertions))]
    let _ = reason;
    #[cfg(debug_assertions)]
    if s.connected {
        let n = CONNECTED.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
        dbg(format_args!("disconnect reason={:?} count={}", reason, n));
    }
    unsafe {
        ffi::DisconnectNamedPipe(s.pipe);
    }
    if matches!(s.op, SessionOp::ReadPending | SessionOp::WritePending) {
        unsafe {
            let mut bytes: DWORD = 0;
            let _ =
                ffi::GetOverlappedResult(s.pipe, &mut s.ol as *mut _ as *mut c_void, &mut bytes, 1);
        }
    }
    s.connected = false;
    s.op = SessionOp::Idle;
    s.write_buf.clear();
    s.reset_read_state();
    rearm_connect(s);
}

fn park(s: &mut Session) {
    unsafe {
        ffi::ResetEvent(s.event);
    }
}

fn consume_completed_op(s: &mut Session) -> bool {
    match s.op {
        SessionOp::Connect => {
            s.connected = true;
            s.op = SessionOp::Idle;
            #[cfg(debug_assertions)]
            {
                let n = CONNECTED.fetch_add(1, Ordering::Relaxed) + 1;
                dbg(format_args!("connect count={}", n));
            }
            true
        }
        SessionOp::ReadPending => match complete_read(s) {
            Some(0) => {
                disconnect_session(s, DisconnectReason::ClientClosed);
                false
            }
            Some(n) => {
                s.op = SessionOp::Idle;
                s.got += n as usize;
                true
            }
            None => {
                park(s);
                false
            }
        },
        SessionOp::ReadBuffered(n) => {
            s.op = SessionOp::Idle;
            s.got += n;
            true
        }
        SessionOp::WritePending => match complete_write(s) {
            Some(0) => {
                disconnect_session(s, DisconnectReason::ClientClosed);
                false
            }
            Some(_) => {
                s.op = SessionOp::Idle;
                s.write_buf.clear();
                true
            }
            None => {
                park(s);
                false
            }
        },
        SessionOp::Idle => {
            disconnect_session(s, DisconnectReason::IdleDefensive);
            false
        }
    }
}

enum NextRead {
    Pipelined,
    SelfWake,
}

fn issue_next_read(s: &mut Session, mode: NextRead) -> bool {
    match issue_read_at(s, 0, HEADER_LEN) {
        ReadIssue::Buffered(n) => {
            if matches!(mode, NextRead::SelfWake) {
                s.op = SessionOp::ReadBuffered(n);
                unsafe {
                    ffi::SetEvent(s.event);
                }
                false
            } else {
                s.got += n;
                true
            }
        }
        ReadIssue::Pending => false,
        ReadIssue::Failed => {
            disconnect_session(s, DisconnectReason::ClientClosed);
            false
        }
    }
}

fn advance_stages(s: &mut Session, state: &mut DaemonState, served: &mut u32) -> bool {
    loop {
        if s.got < s.want {
            match issue_read_at(s, s.got, s.want - s.got) {
                ReadIssue::Buffered(n) => {
                    s.got += n;
                }
                ReadIssue::Pending => return false,
                ReadIssue::Failed => {
                    disconnect_session(s, DisconnectReason::ClientClosed);
                    return false;
                }
            }
            continue;
        }
        if !advance_one_stage(s, state, served) {
            return false;
        }
    }
}

fn advance_one_stage(s: &mut Session, state: &mut DaemonState, served: &mut u32) -> bool {
    match s.stage {
        ReadStage::Header => {
            if !starship_daemon::valid_request_type(s.buf[0]) {
                disconnect_session(s, DisconnectReason::Malformed);
                return false;
            }
            let total_len = u32::from_le_bytes(s.buf[1..HEADER_LEN].try_into().unwrap()) as usize;
            if total_len > MAX_TOTAL_LEN {
                disconnect_session(s, DisconnectReason::Malformed);
                return false;
            }
            s.stage = ReadStage::Body;
            s.want = HEADER_LEN + total_len;
            true
        }
        ReadStage::Body => handle_complete_request(s, state, served),
    }
}

fn handle_complete_request(s: &mut Session, state: &mut DaemonState, served: &mut u32) -> bool {
    let prompt = match state.handle(&s.buf[..s.want]) {
        Ok(p) => p,
        Err(_) => {
            disconnect_session(s, DisconnectReason::HandleError);
            return false;
        }
    };
    stage_response(s, &prompt);
    *served += 1;
    s.reset_read_state();
    match issue_write(s) {
        WriteIssue::Completed => {}
        WriteIssue::Pending => return false,
        WriteIssue::Failed => {
            disconnect_session(s, DisconnectReason::WriteFailed);
            return false;
        }
    }
    if *served >= MAX_REQS_PER_WAKE {
        issue_next_read(s, NextRead::SelfWake)
    } else {
        issue_next_read(s, NextRead::Pipelined)
    }
}

fn service_session(s: &mut Session, state: &mut DaemonState) {
    debug_assert!(!matches!(s.op, SessionOp::Idle));
    let mut served = 0u32;
    if !consume_completed_op(s) {
        return;
    }
    advance_stages(s, state, &mut served);
}

fn stage_response(s: &mut Session, output: &str) {
    let mut wb = Vec::with_capacity(4 + output.len());
    wb.extend_from_slice(&(output.len() as u32).to_le_bytes());
    wb.extend_from_slice(output.as_bytes());
    s.write_buf = wb;
}

fn create_sessions(pipe_name: &[u16]) -> Vec<Session> {
    let mut sessions = Vec::with_capacity(MAX_SESSIONS);
    for _ in 0..MAX_SESSIONS {
        let pipe = unsafe {
            ffi::CreateNamedPipeW(
                pipe_name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_MESSAGE | PIPE_WAIT,
                MAX_SESSIONS as DWORD,
                MAX_FRAME_LEN as DWORD,
                MAX_FRAME_LEN as DWORD,
                0,
                std::ptr::null(),
            )
        };
        if pipe == ffi::INVALID_HANDLE_VALUE {
            std::process::exit(1);
        }
        let mode = PIPE_READMODE_BYTE;
        unsafe {
            ffi::SetNamedPipeHandleState(
                pipe,
                &mode as *const _ as *mut _,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
        let event = unsafe { ffi::CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            std::process::exit(1);
        }
        sessions.push(Session::new(pipe, event));
    }
    sessions
}

fn service_signaled_sessions(sessions: &mut [Session], state: &mut DaemonState) {
    for s in sessions.iter_mut() {
        let ev = s.event;
        if unsafe { ffi::WaitForSingleObject(ev, 0) } != ffi::WAIT_OBJECT_0 {
            continue;
        }
        s.last_activity = Instant::now();
        service_session(s, state);
    }
    if sessions.iter().filter(|s| s.connected).count() >= MAX_SESSIONS
        && let Some(i) = sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| s.connected)
            .min_by_key(|(_, s)| s.last_activity)
            .map(|(i, _)| i)
    {
        #[cfg(debug_assertions)]
        dbg(format_args!("cache full ({}), evicting LRU", MAX_SESSIONS));
        disconnect_session(&mut sessions[i], DisconnectReason::LruEvict);
    }
}

pub struct Server {
    sessions: Vec<Session>,
    state: DaemonState,
    pipe_name: String,
    handles: Vec<HANDLE>,
}

impl Server {
    pub fn new(state: DaemonState, pipe_name: String) -> Self {
        let wide = ffi::to_wide(&pipe_name);
        let mut sessions = create_sessions(&wide);
        for s in &mut sessions {
            rearm_connect(s);
        }
        Server {
            sessions,
            state,
            pipe_name,
            handles: Vec::with_capacity(MAX_SESSIONS + MAX_WATCHED_REPOS),
        }
    }

    pub fn run(&mut self) {
        println!("starship-daemon started on {}", self.pipe_name);
        self.state.warm_up();
        loop {
            for s in &self.sessions {
                self.handles.push(s.event);
            }
            for i in 0..self.state.watcher.num_entries() {
                self.handles.push(self.state.watcher.change_event(i));
            }
            let total = self.handles.len() as DWORD;
            let rc = unsafe {
                ffi::WaitForMultipleObjects(total, self.handles.as_ptr(), 0, ffi::INFINITE)
            };
            if rc == ffi::WAIT_FAILED {
                eprintln!(
                    "starship-daemon: WaitForMultipleObjects failed (GetLastError={})",
                    unsafe { ffi::GetLastError() }
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
                self.handles.clear();
                continue;
            }
            self.state.watcher.process_signaled();
            if rc < ffi::WAIT_OBJECT_0 + total {
                service_signaled_sessions(&mut self.sessions, &mut self.state);
            }
            self.handles.clear();
        }
    }
}

// ---- debug-only (all cfg(debug_assertions)) ----
#[cfg(debug_assertions)]
static CONNECTED: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug)]
enum DisconnectReason {
    LruEvict,
    ClientClosed,
    Malformed,
    HandleError,
    WriteFailed,
    IdleDefensive,
}

#[cfg(debug_assertions)]
fn dbg(args: std::fmt::Arguments) {
    eprintln!("[dbg] {}", args);
}
