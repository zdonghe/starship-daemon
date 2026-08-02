use std::ffi::c_void;
use std::mem;
use std::time::Instant;

use starship_daemon::daemon::DaemonState;
use starship_daemon::ffi::{self, HANDLE, DWORD, LPCVOID, LPVOID};
use starship_daemon::watch::MAX_WATCHED_REPOS;
use starship_daemon::{HEADER_LEN, MAX_FRAME_LEN, MAX_TOTAL_LEN, PROTO_VERSION};

const PIPE_ACCESS_DUPLEX: DWORD = 3;
const FILE_FLAG_OVERLAPPED: DWORD = 0x40000000;
const PIPE_TYPE_MESSAGE: DWORD = 4;
const PIPE_WAIT: DWORD = 0;
const ERROR_PIPE_CONNECTED: DWORD = 535;
const ERROR_IO_PENDING: DWORD = 997;
const PIPE_READMODE_BYTE: DWORD = 0;
// 9 instances: evict the least-recently-active on a 9th connect so a queued
// client always finds a free one. Idle-reap timer rejected: a silent client
// holds its slot only until 9-way contention, then reconnects on next prompt.
const MAX_SESSIONS: usize = 9;
const MAX_REQS_PER_WAKE: u32 = 8;
// Max frame = 5 + MAX_TOTAL_LEN = MAX_FRAME_LEN (lib.rs), the one source of
// truth for buf and the pipe in/out buffers; total_len is capped at the header
// stage so want never outgrows buf. Body caps (32768+256+4096) plus fixed
// fields ~37 KiB; the headroom is forward-compat slack.

// Wait set is the only thing sharing MAXIMUM_WAIT_OBJECTS (64); guard against
// silent WAIT_FAILED busy-spin if either cap is raised.
const _: () = assert!(MAX_SESSIONS + MAX_WATCHED_REPOS <= 64);

#[derive(Clone, Copy)]
enum ReadStage {
    Header, // 5 bytes: [u8 version][u32 LE total_len]
    Body,   // total_len bytes of body
}

// The one outstanding overlapped op. Never Idle at rest: Connect (parked in
// ConnectNamedPipe), ReadPending/WritePending (kernel owns the OVERLAPPED), or
// ReadBuffered (sync-completed read deferred across a firehose self-wake).
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
    // Frame receive state: got = received prefix = buf write offset; reads are
    // only ever issued for want - got, so got never overshoots want and
    // buf[0..got] is always the received prefix.
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

    // Re-arm for the next header. Sets ONLY stage/want/got - never op (see
    // disconnect_session's cancel-wait).
    fn reset_read_state(&mut self) {
        self.stage = ReadStage::Header;
        self.want = HEADER_LEN;
        self.got = 0;
    }
}

enum ReadIssue {
    Buffered(usize), // n bytes already in buf; caller folds into got
    Pending,         // op is in flight; event will re-signal on completion
    Failed,          // read broken; caller should disconnect
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
        let ret = ffi::ReadFile(s.pipe, s.buf.as_mut_ptr().add(offset) as LPVOID, count as DWORD, &mut read, ol);
        if ret != 0 {
            if read == 0 { return ReadIssue::Failed; }
            return ReadIssue::Buffered(read as usize);
        }
        if ffi::GetLastError() == ERROR_IO_PENDING {
            s.op = SessionOp::ReadPending;
            return ReadIssue::Pending;
        }
        ReadIssue::Failed
    }
}

// Begin a fresh op. Order is load-bearing: zero the OVERLAPPED before binding
// the event, then ResetEvent before issuing, so a stale completion can't be
// attributed to this op.
fn begin_op(s: &mut Session) -> *mut c_void {
    unsafe {
        s.ol = mem::zeroed();
        s.ol.h_event = s.event;
        ffi::ResetEvent(s.event);
        &mut s.ol as *mut _ as *mut c_void
    }
}

// Poll the op: Some(n) done (n bytes; 0 = broken), None = in flight. `expected`
// gates a short write (bytes != len) into Some(0) so a half-written frame
// disconnects instead of resuming.
fn complete_op(s: &mut Session, expected: Option<usize>) -> Option<u32> {
    unsafe {
        let mut bytes: DWORD = 0;
        let ok = ffi::GetOverlappedResult(s.pipe, &mut s.ol as *mut _ as *mut c_void, &mut bytes, 0);
        if ok != 0 {
            if let Some(len) = expected {
                if bytes != len as DWORD { return Some(0); }
            }
            return Some(bytes);
        }
        if ffi::GetLastError() == ffi::ERROR_IO_INCOMPLETE { return None; }
        Some(0)
    }
}

fn complete_read(s: &mut Session) -> Option<u32> { complete_op(s, None) }

fn complete_write(s: &mut Session) -> Option<u32> { complete_op(s, Some(s.write_buf.len())) }

fn issue_write(s: &mut Session) -> WriteIssue {
    unsafe {
        let ol = begin_op(s);
        let mut w: DWORD = 0;
        let ret = ffi::WriteFile(s.pipe, s.write_buf.as_ptr() as LPCVOID, s.write_buf.len() as DWORD, &mut w, ol);
        if ret != 0 { return WriteIssue::Completed; }
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
            // Client raced the rearm and connected first; signal to reap it.
            ffi::SetEvent(s.event);
        }
    }
}

fn disconnect_session(s: &mut Session) {
    unsafe { ffi::DisconnectNamedPipe(s.pipe); }
    if matches!(s.op, SessionOp::ReadPending | SessionOp::WritePending) {
        // DisconnectNamedPipe cancels the pending op; wait out the cancel
        // before rearming so a stale completion can't signal after ResetEvent.
        unsafe {
            let mut bytes: DWORD = 0;
            let _ = ffi::GetOverlappedResult(s.pipe, &mut s.ol as *mut _ as *mut c_void, &mut bytes, 1);
        }
    }
    s.connected = false;
    s.op = SessionOp::Idle;
    s.write_buf.clear();
    s.reset_read_state();
    rearm_connect(s);
}

// Manual-reset event is still latched (WFMO never clears it) but the op is
// still pending: clear the latch or the daemon spins; the real completion
// re-signals. Same reasoning as begin_op.
fn park(s: &mut Session) {
    unsafe { ffi::ResetEvent(s.event); }
}

// Reap the completed op into session state. Returns false to stop servicing.
fn consume_completed_op(s: &mut Session) -> bool {
    match s.op {
        SessionOp::Connect => {
            // Signal-based: no GetOverlappedResult (a connect reports 0 bytes,
            // ERROR_PIPE_CONNECTED reads as pending). First read comes next.
            s.connected = true;
            s.op = SessionOp::Idle;
            true
        }
        SessionOp::ReadPending => match complete_read(s) {
            Some(0) => { disconnect_session(s); false }
            Some(n) => { s.op = SessionOp::Idle; s.got += n as usize; true }
            None => { park(s); false }
        },
        SessionOp::ReadBuffered(n) => { s.op = SessionOp::Idle; s.got += n; true }
        SessionOp::WritePending => match complete_write(s) {
            Some(0) => { disconnect_session(s); false }
            Some(_) => { s.op = SessionOp::Idle; s.write_buf.clear(); true }
            None => { park(s); false }
        },
        // No op in flight: unreachable, but reset to listening rather than
        // wedge a silent session (client would hang; its Read has no timeout).
        SessionOp::Idle => { disconnect_session(s); false }
    }
}

enum NextRead { Pipelined, SelfWake }

// Next header read after a response. Pipelined folds a buffered result and
// keeps driving; SelfWake (firehose cap) defers the fold and self-wakes so
// this session yields this iteration.
fn issue_next_read(s: &mut Session, mode: NextRead) -> bool {
    match issue_read_at(s, 0, HEADER_LEN) {
        ReadIssue::Buffered(n) => {
            if matches!(mode, NextRead::SelfWake) {
                s.op = SessionOp::ReadBuffered(n);
                unsafe { ffi::SetEvent(s.event); }
                false
            } else {
                s.got += n;
                true
            }
        }
        ReadIssue::Pending => false,
        ReadIssue::Failed => { disconnect_session(s); false }
    }
}

fn advance_stages(s: &mut Session, state: &mut DaemonState, served: &mut u32) -> bool {
    loop {
        if s.got < s.want {
            match issue_read_at(s, s.got, s.want - s.got) {
                ReadIssue::Buffered(n) => { s.got += n; }
                ReadIssue::Pending => return false,
                ReadIssue::Failed => { disconnect_session(s); return false; }
            }
            continue;
        }
        if !advance_one_stage(s, state, served) { return false; }
    }
}

fn advance_one_stage(s: &mut Session, state: &mut DaemonState, served: &mut u32) -> bool {
    match s.stage {
        ReadStage::Header => {
            // Malformed frame: drop the connection; client falls back to its
            // plain prompt.
            if s.buf[0] != PROTO_VERSION {
                disconnect_session(s);
                return false;
            }
            let total_len = u32::from_le_bytes(s.buf[1..HEADER_LEN].try_into().unwrap()) as usize;
            if total_len > MAX_TOTAL_LEN {
                disconnect_session(s);
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
        Err(_) => { disconnect_session(s); return false; }
    };
    stage_response(s, &prompt);
    *served += 1;
    s.reset_read_state();
    // Async write: never block the daemon on a full out-buffer; a Pending
    // write resumes when the client drains.
    match issue_write(s) {
        WriteIssue::Completed => {}
        WriteIssue::Pending => return false,
        WriteIssue::Failed => { disconnect_session(s); return false; }
    }
    if *served >= MAX_REQS_PER_WAKE {
        issue_next_read(s, NextRead::SelfWake)
    } else {
        issue_next_read(s, NextRead::Pipelined)
    }
}

fn service_session(s: &mut Session, state: &mut DaemonState) {
    // At rest a session always has one op outstanding; Idle is transient.
    debug_assert!(!matches!(s.op, SessionOp::Idle));
    let mut served = 0u32;
    // Consume once, then drive stages; advance_stages always parks or
    // disconnects, so one pass per wake.
    if !consume_completed_op(s) { return; }
    advance_stages(s, state, &mut served);
}

fn stage_response(s: &mut Session, output: &str) {
    // [u32 LE len][prompt utf8]; len = prompt.len().
    let mut wb = Vec::with_capacity(4 + output.len());
    wb.extend_from_slice(&(output.len() as u32).to_le_bytes());
    wb.extend_from_slice(output.as_bytes());
    s.write_buf = wb;
}

fn create_sessions(pipe_name: &[u16]) -> Vec<Session> {
    let mut sessions = Vec::with_capacity(MAX_SESSIONS);
    for _ in 0..MAX_SESSIONS {
        let pipe = unsafe { ffi::CreateNamedPipeW(pipe_name.as_ptr(), PIPE_ACCESS_DUPLEX|FILE_FLAG_OVERLAPPED, PIPE_TYPE_MESSAGE|PIPE_WAIT, MAX_SESSIONS as DWORD, MAX_FRAME_LEN as DWORD, MAX_FRAME_LEN as DWORD, 0, std::ptr::null()) };
        if pipe == ffi::INVALID_HANDLE_VALUE { std::process::exit(1); }
        let mode = PIPE_READMODE_BYTE;
        unsafe { ffi::SetNamedPipeHandleState(pipe, &mode as *const _ as *mut _, std::ptr::null_mut(), std::ptr::null_mut()); }
        let event = unsafe { ffi::CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() { std::process::exit(1); }
        sessions.push(Session::new(pipe, event));
    }
    sessions
}

// Service every signaled session (not WFMO's lowest-index result) so a
// self-woken firehose session can't re-select itself and starve the rest.
// Then rotate: the eviction check runs on any session wake, but eviction can
// only fire when the connected count hits MAX_SESSIONS, reachable only on a
// wake that reaped a connect, so a queued client always finds a free instance.
fn service_signaled_sessions(sessions: &mut [Session], state: &mut DaemonState) {
    for idx in 0..sessions.len() {
        let ev = sessions[idx].event;
        if unsafe { ffi::WaitForSingleObject(ev, 0) } != ffi::WAIT_OBJECT_0 { continue; }
        let s = &mut sessions[idx];
        s.last_activity = Instant::now();
        service_session(s, state);
    }
    // 9th connected session is transient; evict the least-recently-active so a
    // free instance stays available for a queued client.
    if sessions.iter().filter(|s| s.connected).count() >= MAX_SESSIONS {
        if let Some(i) = sessions.iter().enumerate()
            .filter(|(_, s)| s.connected)
            .min_by_key(|(_, s)| s.last_activity)
            .map(|(i, _)| i)
        {
            disconnect_session(&mut sessions[i]);
        }
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
        for s in &mut sessions { rearm_connect(s); }
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
            for s in &self.sessions { self.handles.push(s.event); }
            for i in 0..self.state.watcher.num_entries() { self.handles.push(self.state.watcher.change_event(i)); }
            // Pure event-driven: connect/write/watcher all wake via events, no tick.
            let total = self.handles.len() as DWORD;
            let rc = unsafe { ffi::WaitForMultipleObjects(total, self.handles.as_ptr(), 0, ffi::INFINITE) };
            if rc == ffi::WAIT_FAILED {
                // Unreachable (const-asserted budget); fail loudly, don't spin.
                eprintln!("starship-daemon: WaitForMultipleObjects failed (GetLastError={})", unsafe { ffi::GetLastError() });
                std::thread::sleep(std::time::Duration::from_millis(10));
                self.handles.clear();
                continue;
            }
            self.state.watcher.process_signaled();
            if rc >= ffi::WAIT_OBJECT_0 && rc < ffi::WAIT_OBJECT_0 + total {
                service_signaled_sessions(&mut self.sessions, &mut self.state);
            }
            self.handles.clear();
        }
    }
}
