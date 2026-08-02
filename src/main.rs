use std::mem;
use std::num::NonZeroUsize;
use std::time::Instant;

use std::path::{Path, PathBuf};
use std::ffi::c_void;

use lru::LruCache;
use starship_daemon::cache::{self, CacheKey, CachedValue, RenderContext};
use starship_daemon::ffi::{self, HANDLE, DWORD, LPVOID, LPCVOID};
use starship_daemon::watch::{WatcherState, MAX_WATCHED_REPOS, STATS_ENABLED, drain_stats};
use starship_daemon::{ParsedRequest, parse_request, PROTO_VERSION, HEADER_LEN, MAX_TOTAL_LEN};

const PIPE_ACCESS_DUPLEX: DWORD = 3;
const FILE_FLAG_OVERLAPPED: DWORD = 0x40000000;
const PIPE_TYPE_MESSAGE: DWORD = 4;
const PIPE_WAIT: DWORD = 0;
const ERROR_PIPE_CONNECTED: DWORD = 535;
const ERROR_IO_PENDING: DWORD = 997;
const PIPE_READMODE_BYTE: DWORD = 0;
// 9 pipe instances: at most 8 serve an active client at once. The moment a 9th
// connects we evict the least-recently-active session, so one instance is always
// free for a queued client. 9 sessions + 16 watcher handles = 25 <= 64.
//
// Trade-off vs. a timer-based idle reap: a connected-but-silent client now holds
// its slot (and its ~36 KiB buffers) until 9-way contention, and evicting a
// session breaks that connection (the client reconnects on its next prompt).
// Both are bounded by the 9-instance cap and self-heal under pressure.
const MAX_SESSIONS: usize = 9;
const MAX_REQS_PER_WAKE: u32 = 8;
// Max frame: [u8 version][u32 LE total_len][body] = 5 + 65531 = 65536. The body
// caps (cwd 32768, keymap 256, config 4096) plus fixed fields total ~37 KiB; the
// headroom is forward-compat slack. Must equal MAX_FRAME_LEN in lib.rs.
const BUF_SIZE: usize = 65536;

// The wait set shares MAXIMUM_WAIT_OBJECTS (64); the session events and watcher
// change events are the only handles in it. Compile-time guard against silent
// WAIT_FAILED busy-spinning if either cap is ever raised.
const _: () = assert!(MAX_SESSIONS + MAX_WATCHED_REPOS <= 64);

#[derive(Clone, Copy)]
enum ReadStage {
    Header, // 5 bytes: [u8 version][u32 LE total_len]
    Body,   // total_len bytes of body
}

struct Session {
    pipe: HANDLE,
    ol: ffi::OVERLAPPED,
    event: HANDLE,
    buf: [u8; BUF_SIZE],
    active: bool,
    sync_bytes: u32,
    read_in_flight: bool,
    write_in_flight: bool,
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
            buf: [0u8; BUF_SIZE],
            active: false,
            sync_bytes: 0,
            read_in_flight: false,
            write_in_flight: false,
            write_buf: Vec::new(),
            stage: ReadStage::Header,
            want: HEADER_LEN,
            got: 0,
            last_activity: Instant::now(),
        }
    }
}

enum IssueOutcome { Sync, Pending, Error }

fn issue_read_at(s: &mut Session, offset: usize, count: usize) -> IssueOutcome {
    unsafe {
        s.ol = mem::zeroed();
        s.ol.h_event = s.event;
        ffi::ResetEvent(s.event);
        let mut read: DWORD = 0;
        let ret = ffi::ReadFile(s.pipe, s.buf.as_mut_ptr().add(offset) as LPVOID, count as DWORD, &mut read, &mut s.ol as *mut _ as *mut c_void);
        if ret != 0 {
            if read == 0 { return IssueOutcome::Error; }
            s.sync_bytes = read;
            s.read_in_flight = false;
            return IssueOutcome::Sync;
        }
        let err = ffi::GetLastError();
        if err == ERROR_IO_PENDING {
            s.read_in_flight = true;
            return IssueOutcome::Pending;
        }
        IssueOutcome::Error
    }
}

fn complete_read(s: &mut Session) -> Option<u32> {
    unsafe {
        let mut bytes: DWORD = 0;
        let ok = ffi::GetOverlappedResult(s.pipe, &mut s.ol as *mut _ as *mut c_void, &mut bytes, 0);
        if ok != 0 { return Some(bytes); }
        if ffi::GetLastError() == ffi::ERROR_IO_INCOMPLETE { return None; }
        Some(0)
    }
}

enum WriteIssue { Done, Pending, Error }

fn issue_write(s: &mut Session) -> WriteIssue {
    unsafe {
        s.ol = mem::zeroed();
        s.ol.h_event = s.event;
        ffi::ResetEvent(s.event);
        let mut w: DWORD = 0;
        let ret = ffi::WriteFile(s.pipe, s.write_buf.as_ptr() as LPCVOID, s.write_buf.len() as DWORD, &mut w, &mut s.ol as *mut _ as *mut c_void);
        if ret != 0 { return WriteIssue::Done; }
        let err = ffi::GetLastError();
        if err == ERROR_IO_PENDING {
            s.write_in_flight = true;
            return WriteIssue::Pending;
        }
        WriteIssue::Error
    }
}

fn complete_write(s: &mut Session) -> Option<u32> {
    unsafe {
        let mut bytes: DWORD = 0;
        let ok = ffi::GetOverlappedResult(s.pipe, &mut s.ol as *mut _ as *mut c_void, &mut bytes, 0);
        if ok != 0 && bytes == s.write_buf.len() as DWORD { return Some(bytes); }
        if ok != 0 { return Some(0); }
        if ffi::GetLastError() == ffi::ERROR_IO_INCOMPLETE { return None; }
        Some(0)
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

fn disconnect_session(s: &mut Session) {
    unsafe { ffi::DisconnectNamedPipe(s.pipe); }
    if s.read_in_flight || s.write_in_flight {
        // An op is pending; DisconnectNamedPipe cancels it.
        // Wait for the cancellation to fully complete before reusing the
        // OVERLAPPED and resetting the event in rearm_connect, so a stale
        // completion cannot signal the event after ResetEvent.
        unsafe {
            let mut bytes: DWORD = 0;
            let _ = ffi::GetOverlappedResult(s.pipe, &mut s.ol as *mut _ as *mut c_void, &mut bytes, 1);
        }
    }
    s.active = false;
    s.read_in_flight = false;
    s.write_in_flight = false;
    s.write_buf.clear();
    s.sync_bytes = 0;
    s.stage = ReadStage::Header;
    s.want = HEADER_LEN;
    s.got = 0;
    rearm_connect(s);
}

fn service_session(
    s: &mut Session,
    config_path: &mut PathBuf,
    cached_config: &mut toml::Table,
    last_cfg_mtime: &mut u64,
    lru: &mut LruCache<CacheKey, CachedValue>,
    watcher: &mut WatcherState,
) {
    let mut served = 0u32;
    loop {
        // Consume whatever operation completed (write, sync read, or pending read).
        if s.write_in_flight {
            match complete_write(s) {
                Some(n) => {
                    s.write_in_flight = false;
                    s.write_buf.clear();
                    if n == 0 { disconnect_session(s); return; }
                }
                None => { unsafe { ffi::ResetEvent(s.event); } return; } // still pending
            }
        } else if s.sync_bytes > 0 {
            s.got += s.sync_bytes as usize;
            s.sync_bytes = 0;
        } else if s.read_in_flight {
            match complete_read(s) {
                Some(n) => {
                    s.read_in_flight = false;
                    if n == 0 { disconnect_session(s); return; }
                    s.got += n as usize;
                }
                None => { unsafe { ffi::ResetEvent(s.event); } return; } // still pending
            }
        } else {
            return; // nothing in flight
        }

        // Advance stages while a full stage is buffered; issue reads for the rest.
        loop {
            if s.got < s.want {
                match issue_read_at(s, s.got, s.want - s.got) {
                    IssueOutcome::Sync => break, // sync_bytes set; outer loop consumes
                    IssueOutcome::Pending => return,
                    IssueOutcome::Error => { disconnect_session(s); return; }
                }
            }
            // got >= want: current stage complete.
            match s.stage {
                ReadStage::Header => {
                    // Version or length violations are deterministic: drop the
                    // connection. The client falls back to its plain prompt.
                    if s.buf[0] != PROTO_VERSION {
                        disconnect_session(s);
                        return;
                    }
                    let total_len = u32::from_le_bytes(s.buf[1..HEADER_LEN].try_into().unwrap()) as usize;
                    if total_len > MAX_TOTAL_LEN {
                        disconnect_session(s);
                        return;
                    }
                    s.stage = ReadStage::Body;
                    s.want = HEADER_LEN + total_len;
                }
                ReadStage::Body => {
                    if !process_request(s, s.want, config_path, cached_config, last_cfg_mtime, lru, watcher) {
                        disconnect_session(s);
                        return;
                    }
                    served += 1;
                    s.stage = ReadStage::Header;
                    s.want = HEADER_LEN;
                    s.got = 0;
                    // Issue the staged response asynchronously; never block the
                    // daemon on a full out-buffer. A Pending write returns to
                    // the event loop and resumes when the client drains.
                    match issue_write(s) {
                        WriteIssue::Done => {}
                        WriteIssue::Pending => return,
                        WriteIssue::Error => { disconnect_session(s); return; }
                    }
                    if served >= MAX_REQS_PER_WAKE {
                        // Firehose cap: self-wake so other sessions and the
                        // watcher get serviced this loop iteration.
                        match issue_read_at(s, 0, HEADER_LEN) {
                            IssueOutcome::Sync => { unsafe { ffi::SetEvent(s.event); } return; }
                            IssueOutcome::Pending => return,
                            IssueOutcome::Error => { disconnect_session(s); return; }
                        }
                    }
                    match issue_read_at(s, 0, HEADER_LEN) {
                        IssueOutcome::Sync => break, // next request already buffered
                        IssueOutcome::Pending => return,
                        IssueOutcome::Error => { disconnect_session(s); return; }
                    }
                }
            }
        }
    }
}

fn process_request(s: &mut Session, total: usize, config_path: &mut PathBuf, cached_config: &mut toml::Table, last_cfg_mtime: &mut u64, lru: &mut LruCache<CacheKey, CachedValue>, watcher: &mut WatcherState) -> bool {
    let ParsedRequest { cwd, props } = match parse_request(&s.buf[..total]) { Some(r) => r, None => { return false; } };
    // Lenient empty cwd: feed "." so rendering never sees a zero-length path.
    let cwd = if cwd.as_os_str().is_empty() { PathBuf::from(".") } else { cwd };
    let git_dir = starship_daemon::find_git_dir(&cwd);
    let status_code = props.status_code;
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

    let tw = props.terminal_width;

    if props.disable_cache {
        let ctx = RenderContext { cwd: cwd.clone(), terminal_width: tw, status_code, keymap };
        let output = cache::render_prompt_with_config(&ctx, git_dir.as_deref(), cached_config);
        stage_response(s, &output);
        return true;
    }

    let repo_root = git_dir.as_ref().and_then(|g| g.parent());
    let watcher_version = if let Some(r) = repo_root {
        watcher.ensure(r);
        watcher.flush();
        watcher.version(r)
    } else {
        0
    };
    let ck = cache::compute_cache_key(&cwd, &keymap, tw, cur_cfg_mtime, watcher_version);
    let ctx = RenderContext { cwd: cwd.clone(), terminal_width: tw, status_code, keymap };

    let output = cache::render_cached(&ctx, git_dir.as_deref(), cached_config, &ck, lru);
    stage_response(s, &output);
    true
}

fn stage_response(s: &mut Session, output: &str) {
    // [u32 LE len][prompt utf8]; len = prompt.len().
    let mut wb = Vec::with_capacity(4 + output.len());
    wb.extend_from_slice(&(output.len() as u32).to_le_bytes());
    wb.extend_from_slice(output.as_bytes());
    s.write_buf = wb;
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

    if std::env::var("STARSHIP_WATCH_STATS").is_ok() {
        STATS_ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
        std::thread::spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            eprintln!("[watcher-stats]\n{}", drain_stats());
        });
    }

    {
        let warm_ctx = RenderContext {
            cwd: PathBuf::from("."),
            terminal_width: 120,
            status_code: 0,
            keymap: "vi".to_string(),
        };
        let warm_key = cache::compute_cache_key(
            Path::new("."), "vi", 120, 0, 0,
        );
        let _ = cache::render_cached(&warm_ctx, None, &cached_config, &warm_key, &mut lru);
    }

    let mut watcher = WatcherState::new();
    let mut handles = Vec::with_capacity(MAX_SESSIONS + MAX_WATCHED_REPOS);

    loop {
        for s in &sessions { handles.push(s.event); }
        for i in 0..watcher.num_entries() { handles.push(watcher.change_event(i)); }
        // Fully event-driven: no periodic tick. A connect, a client write, or a
        // watcher completion all wake the daemon via events.
        let total = handles.len() as DWORD;
        let rc = unsafe { ffi::WaitForMultipleObjects(total, handles.as_ptr(), 0, ffi::INFINITE) };
        if rc == ffi::WAIT_FAILED {
            // Should be unreachable (budget enforced by the const assert above),
            // but fail loudly instead of busy-spinning the loop.
            eprintln!("starship-daemon: WaitForMultipleObjects failed (GetLastError={})", unsafe { ffi::GetLastError() });
            std::thread::sleep(std::time::Duration::from_millis(10));
            handles.clear();
            continue;
        }
        watcher.process_signaled();
        if rc >= ffi::WAIT_OBJECT_0 && rc < ffi::WAIT_OBJECT_0 + total {
            // Service every signaled session, not just the lowest-index one.
            // Otherwise a firehose session that self-wakes (SetEvent at the
            // MAX_REQS_PER_WAKE cap) keeps re-selecting itself via WFMO and
            // starves all higher-index sessions indefinitely.
            for idx in 0..sessions.len() {
                let ev = sessions[idx].event;
                if unsafe { ffi::WaitForSingleObject(ev, 0) } != ffi::WAIT_OBJECT_0 { continue; }
                let s = &mut sessions[idx];
                s.last_activity = Instant::now();
                if s.active {
                    service_session(s, &mut config_path, &mut cached_config, &mut last_cfg_mtime, &mut lru, &mut watcher);
                } else {
                    s.active = true;
                    match issue_read_at(s, 0, HEADER_LEN) {
                        IssueOutcome::Sync => service_session(s, &mut config_path, &mut cached_config, &mut last_cfg_mtime, &mut lru, &mut watcher),
                        IssueOutcome::Pending => {}
                        IssueOutcome::Error => disconnect_session(s),
                    }
                }
            }
            // Eager slot rotation: a 9th active session is transient. Evict the
            // least-recently-active session so a free instance always remains for
            // a queued client. Connect-driven (this very iteration serviced the
            // connect), never a timer.
            if sessions.iter().filter(|s| s.active).count() >= MAX_SESSIONS {
                if let Some(i) = sessions.iter().enumerate()
                    .filter(|(_, s)| s.active)
                    .min_by_key(|(_, s)| s.last_activity)
                    .map(|(i, _)| i)
                {
                    disconnect_session(&mut sessions[i]);
                }
            }
        }
        handles.clear();
    }
}
