use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use starship_daemon::ffi;

const PIPE_PATH: &str = r"\\.\pipe\starship-daemon";

fn pipe_path() -> String {
    std::env::var("STARSHIP_DAEMON_PIPE")
        .map(|n| { if n.starts_with(r"\\.\pipe\") { n } else { format!(r"\\.\pipe\{n}") } })
        .unwrap_or_else(|_| PIPE_PATH.to_string())
}

fn unique_pipe_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let name = format!("starship-daemon-test-{}", std::process::id());
        unsafe { std::env::set_var("STARSHIP_DAEMON_PIPE", &name); }
        name
    })
}

static DAEMON_LOCK: Mutex<()> = Mutex::new(());

struct PipeClient {
    handle: ffi::HANDLE,
}

impl PipeClient {
    fn connect(timeout_ms: u32) -> Option<Self> {
        let deadline = Instant::now()
            + std::time::Duration::from_millis(timeout_ms as u64);
        loop {
            unsafe {
                let wide = ffi::to_wide(&pipe_path());
                let handle = ffi::CreateFileW(
                    wide.as_ptr(),
                    0xC0000000,
                    3,
                    std::ptr::null(),
                    3,
                    0x80,
                    std::ptr::null_mut(),
                );
                if handle != ffi::INVALID_HANDLE_VALUE {
                    return Some(PipeClient { handle });
                }
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    fn send_request(&self, cwd: &str, props: &str) -> bool {
        unsafe {
            let cwd_bytes = cwd.as_bytes();
            let props_bytes = props.as_bytes();
            let mut written: ffi::DWORD = 0;
            let cwd_len = (cwd_bytes.len() as u32).to_le_bytes();
            if ffi::WriteFile(self.handle, cwd_len.as_ptr() as *const c_void, 4, &mut written, std::ptr::null_mut()) == 0 { return false; }
            if ffi::WriteFile(self.handle, cwd_bytes.as_ptr() as *const c_void, cwd_bytes.len() as u32, &mut written, std::ptr::null_mut()) == 0 { return false; }
            let props_len = (props_bytes.len() as u32).to_le_bytes();
            if ffi::WriteFile(self.handle, props_len.as_ptr() as *const c_void, 4, &mut written, std::ptr::null_mut()) == 0 { return false; }
            if ffi::WriteFile(self.handle, props_bytes.as_ptr() as *const c_void, props_bytes.len() as u32, &mut written, std::ptr::null_mut()) == 0 { return false; }
            ffi::FlushFileBuffers(self.handle) != 0
        }
    }

    fn read_response(&self) -> Option<String> {
        self.read_response_timeout(5000)
    }

    fn read_response_timeout(&self, ms: u64) -> Option<String> {
        let deadline = Instant::now() + Duration::from_millis(ms);
        loop {
            unsafe {
                let mut avail: ffi::DWORD = 0;
                if ffi::PeekNamedPipe(self.handle, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut()) != 0 && avail >= 4 {
                    break;
                }
            }
            if Instant::now() > deadline { return None; }
            std::thread::sleep(Duration::from_millis(5));
        }
        unsafe {
            let mut len_buf = [0u8; 4];
            let mut read: ffi::DWORD = 0;
            if ffi::ReadFile(self.handle, len_buf.as_mut_ptr() as *mut c_void, 4, &mut read, std::ptr::null_mut()) == 0 || read != 4 {
                return None;
            }
            let resp_len = u32::from_le_bytes(len_buf) as usize;
            if resp_len > 65536 { return None; }
            let mut resp_buf = vec![0u8; resp_len];
            let mut total_read = 0;
            while total_read < resp_len {
                let chunk = (resp_len - total_read) as u32;
                if ffi::ReadFile(self.handle, resp_buf[total_read..].as_mut_ptr() as *mut c_void, chunk, &mut read, std::ptr::null_mut()) == 0 {
                    return None;
                }
                total_read += read as usize;
            }
            String::from_utf8(resp_buf).ok()
        }
    }

    fn write_raw(&self, bytes: &[u8]) -> bool {
        unsafe {
            let mut written: ffi::DWORD = 0;
            ffi::WriteFile(self.handle, bytes.as_ptr() as *const c_void, bytes.len() as u32, &mut written, std::ptr::null_mut()) != 0
        }
    }
}

impl Drop for PipeClient {
    fn drop(&mut self) {
        unsafe { ffi::CloseHandle(self.handle); }
    }
}

struct DaemonProcess {
    process: std::process::Child,
    _config_dir: tempfile::TempDir,
}

impl DaemonProcess {
    fn with_config(config_toml: &str) -> Self {
        let config_dir = tempfile::TempDir::new().unwrap();
        let config_path = config_dir.path().join("starship.toml");
        std::fs::write(&config_path, config_toml.as_bytes()).unwrap();

        unique_pipe_name();

        let daemon_exe = std::env::var("CARGO_BIN_EXE_starship-daemon")
            .unwrap_or_else(|_| {
                let mut p = std::env::current_exe().unwrap();
                p.pop();
                p.pop();
                p.push("starship-daemon.exe");
                p.display().to_string()
            });

        let mut process = std::process::Command::new(&daemon_exe)
            .env("STARSHIP_CONFIG", &config_path)
            .spawn()
            .expect("failed to spawn daemon");

        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(c) = PipeClient::connect(100) {
                drop(c);
                break;
            }
            if Instant::now() > deadline {
                let _ = process.kill();
                let _ = process.wait();
                panic!("daemon did not become ready within 10s");
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        DaemonProcess { process, _config_dir: config_dir }
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn with_daemon_config<F>(config_toml: &str, f: F)
where
    F: FnOnce() + std::panic::UnwindSafe,
{
    let lock = DAEMON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let daemon = DaemonProcess::with_config(config_toml);
    let result = std::panic::catch_unwind(f);
    drop(daemon);
    drop(lock);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

fn with_daemon<F>(f: F)
where
    F: FnOnce() + std::panic::UnwindSafe,
{
    with_daemon_config(
        "format = \"$character\"\nadd_newline = false\n[character]\nformat = \">\"\n",
        f,
    );
}

fn check_response(resp: &str) {
    assert!(!resp.is_empty(), "response should be non-empty");
    assert!(resp.contains('$') || resp.contains('❮') || resp.contains(">"),
        "response should contain a prompt character, got: {resp:?}");
}

#[test]
fn ipc_content_served() {
    with_daemon(|| {
        let c = PipeClient::connect(1000).expect("connect");
        assert!(c.send_request(".", "{}"));
        let resp = c.read_response().expect("read response");
        check_response(&resp);
    });
}

#[test]
fn ipc_reconnect() {
    with_daemon(|| {
        let resp1;
        {
            let c = PipeClient::connect(1000).expect("first connect");
            assert!(c.send_request(".", "{}"));
            resp1 = c.read_response().expect("first response");
            check_response(&resp1);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        {
            let c = PipeClient::connect(1000).expect("reconnect");
            assert!(c.send_request(".", "{}"));
            let resp2 = c.read_response().expect("second response");
            check_response(&resp2);
            assert_eq!(resp1, resp2, "same cwd and props should give same prompt");
        }
    });
}

#[test]
fn ipc_multiple_connections() {
    with_daemon(|| {
        for i in 0..5 {
            let c = PipeClient::connect(1000)
                .unwrap_or_else(|| panic!("connect attempt {i}"));
            assert!(c.send_request(".", r#"{"status_code":0}"#));
            let resp = c.read_response()
                .unwrap_or_else(|| panic!("response {i}"));
            check_response(&resp);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
}

#[test]
fn ipc_mid_request_disconnect() {
    with_daemon(|| {
        let raw = ManuallyDrop::new(PipeClient::connect(1000).expect("connect with retry"));
        unsafe {
            let partial = b"AAAA";
            let mut written: ffi::DWORD = 0;
            ffi::WriteFile(raw.handle, partial.as_ptr() as *const c_void, 4, &mut written, std::ptr::null_mut());
            ffi::CloseHandle(raw.handle);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));

        let c = PipeClient::connect(1000).expect("reconnect after partial write");
        assert!(c.send_request(".", "{}"));
        let resp = c.read_response().expect("response after partial write");
        check_response(&resp);
    });
}

#[test]
fn ipc_custom_status_code() {
    with_daemon_config(
        "format = \"$status\"\nadd_newline = false\n\
         [status]\ndisabled = false\nsuccess_symbol = \"OK\"\nsymbol = \"FAIL\"\n",
        || {
            let c = PipeClient::connect(1000).expect("connect");
            assert!(c.send_request(".", r#"{"status_code":0}"#));
            let resp0 = c.read_response().expect("status 0 response");
            assert!(resp0.contains("OK"),
                "status 0 prompt should contain OK symbol, got: {resp0:?}");

            let c = PipeClient::connect(1000).expect("reconnect");
            assert!(c.send_request(".", r#"{"status_code":1}"#));
            let resp1 = c.read_response().expect("status 1 response");
            assert_ne!(resp0, resp1, "different status codes must give different prompts");
            assert!(resp1.contains("FAIL"),
                "status 1 prompt should contain the FAIL symbol, got: {resp1:?}");
        },
    );
}

#[test]
fn ipc_disable_cache() {
    with_daemon(|| {
        let c = PipeClient::connect(1000).expect("connect");
        assert!(c.send_request(".", r#"{"disable_cache":true}"#));
        let resp = c.read_response().expect("response");
        check_response(&resp);
    });
}

#[test]
fn ipc_sync_drop_race() {
    with_daemon(|| {
        let a = PipeClient::connect(1000).expect("A connect");
        std::thread::sleep(Duration::from_millis(300));

        unsafe {
            let mut written: ffi::DWORD = 0;
            let cwd_len = 1u32.to_le_bytes();
            ffi::WriteFile(a.handle, cwd_len.as_ptr() as *const c_void, 4, &mut written, std::ptr::null_mut());
            ffi::FlushFileBuffers(a.handle);
        }
        std::thread::sleep(Duration::from_millis(200));

        let b = PipeClient::connect(1000).expect("B connect");
        unsafe {
            let mut written: ffi::DWORD = 0;
            let cwd_bytes: &[u8] = b".";
            let props_bytes = b"{}";
            let buf = [1u8, 0u8, 0u8, 0u8];
            let props_len = (props_bytes.len() as u32).to_le_bytes();
            ffi::WriteFile(b.handle, buf.as_ptr() as *const c_void, 4, &mut written, std::ptr::null_mut());
            ffi::WriteFile(b.handle, cwd_bytes.as_ptr() as *const c_void, cwd_bytes.len() as u32, &mut written, std::ptr::null_mut());
            ffi::WriteFile(b.handle, props_len.as_ptr() as *const c_void, 4, &mut written, std::ptr::null_mut());
            ffi::WriteFile(b.handle, props_bytes.as_ptr() as *const c_void, props_bytes.len() as u32, &mut written, std::ptr::null_mut());
        }
        std::thread::sleep(Duration::from_millis(200));

        unsafe {
            let mut written: ffi::DWORD = 0;
            let rest = [b'.', 2u8, 0u8, 0u8, 0u8, b'{', b'}'];
            ffi::WriteFile(a.handle, rest.as_ptr() as *const c_void, rest.len() as u32, &mut written, std::ptr::null_mut());
            ffi::FlushFileBuffers(a.handle);
        }

        let start = Instant::now();
        let resp = b.read_response();
        let elapsed = start.elapsed();
        assert!(resp.is_some(), "B request dropped by sync-drop race (no response in 5s), elapsed={elapsed:?}");
        assert!(elapsed < Duration::from_secs(2), "B response stalled {elapsed:?} - sync-drop race");
    });
}

mod common;
use std::path::Path;

#[test]
fn ipc_git_push_stale_ahead() {
    let bare_dir = tempfile::TempDir::new().unwrap();
    let bare_path = bare_dir.path().join("remote.git");
    std::fs::create_dir_all(&bare_path).unwrap();
    common::git(&bare_path, &["init", "--bare"]);

    let work_dir = tempfile::TempDir::new().unwrap();
    let repo_path = work_dir.path().join("repo");
    std::fs::create_dir_all(&repo_path).unwrap();
    common::git(&repo_path, &["init"]);
    common::git(&repo_path, &["branch", "-M", "main"]);
    common::git(&repo_path, &["config", "user.email", "test@test"]);
    common::git(&repo_path, &["config", "user.name", "test"]);
    common::git(&repo_path, &["remote", "add", "origin", bare_path.to_str().unwrap()]);

    std::fs::write(repo_path.join("init.txt"), "init").unwrap();
    common::git(&repo_path, &["add", "init.txt"]);
    common::git(&repo_path, &["commit", "-m", "init"]);
    common::git(&repo_path, &["push", "-u", "origin", "main"]);
    common::settle();

    std::fs::write(repo_path.join("ahead.txt"), "ahead").unwrap();
    common::git(&repo_path, &["add", "ahead.txt"]);
    common::git(&repo_path, &["commit", "-m", "ahead"]);

    let repo_str = repo_path.to_str().unwrap().to_string();

    with_daemon_config(
        "format = \"$git_status\"\nadd_newline = false\n[git_status]\nformat = \"$ahead_behind\"\n",
        || {
            let c = PipeClient::connect(1000).expect("connect");
            assert!(c.send_request(&repo_str, "{}"));
            let before_push = c.read_response().expect("before push");
            assert!(before_push.contains('⇡'),
                "prompt should show ahead indicator before push, got: {before_push:?}");

            common::git(Path::new(&repo_str), &["push"]);

            let deadline = Instant::now() + Duration::from_secs(5);
            let mut after_push = None;
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
                if let Some(c2) = PipeClient::connect(500) {
                    if c2.send_request(&repo_str, "{}") {
                        if let Some(resp) = c2.read_response() {
                            if !resp.contains('⇡') {
                                after_push = Some(resp);
                                break;
                            }
                            after_push = Some(resp);
                        }
                    }
                }
            }
            let after_push = after_push.expect("after push (no response within 5s)");
            assert!(!after_push.contains('⇡'),
                "prompt should NOT contain ⇡ after push (no longer ahead)");
        },
    );
}

fn setup_ahead_repo() -> (tempfile::TempDir, tempfile::TempDir, String) {
    let bare_dir = tempfile::TempDir::new().unwrap();
    let bare_path = bare_dir.path().join("remote.git");
    std::fs::create_dir_all(&bare_path).unwrap();
    common::git(&bare_path, &["init", "--bare"]);

    let work_dir = tempfile::TempDir::new().unwrap();
    let repo_path = work_dir.path().join("repo");
    std::fs::create_dir_all(&repo_path).unwrap();
    common::git(&repo_path, &["init"]);
    common::git(&repo_path, &["branch", "-M", "main"]);
    common::git(&repo_path, &["config", "user.email", "test@test"]);
    common::git(&repo_path, &["config", "user.name", "test"]);
    common::git(&repo_path, &["remote", "add", "origin", bare_path.to_str().unwrap()]);

    std::fs::write(repo_path.join("init.txt"), "init").unwrap();
    common::git(&repo_path, &["add", "init.txt"]);
    common::git(&repo_path, &["commit", "-m", "init"]);
    common::git(&repo_path, &["push", "-u", "origin", "main"]);
    common::settle();

    std::fs::write(repo_path.join("ahead.txt"), "ahead").unwrap();
    common::git(&repo_path, &["add", "ahead.txt"]);
    common::git(&repo_path, &["commit", "-m", "ahead"]);

    let repo_str = repo_path.to_str().unwrap().to_string();
    (bare_dir, work_dir, repo_str)
}

#[test]
fn ipc_stalled_client_does_not_freeze_daemon() {
    with_daemon(|| {
        let a = PipeClient::connect(1000).expect("A connect");
        // Send only the 4-byte cwd_len header, then stall. The async read
        // state machine must not block other sessions while A's body is
        // incomplete (main.rs:183).
        assert!(a.write_raw(&1u32.to_le_bytes()));
        std::thread::sleep(Duration::from_millis(300));

        let b = PipeClient::connect(1000).expect("B connect");
        let cwd_len = 1u32.to_le_bytes();
        let props_len = 2u32.to_le_bytes();
        assert!(b.write_raw(&cwd_len));
        assert!(b.write_raw(b"."));
        assert!(b.write_raw(&props_len));
        assert!(b.write_raw(b"{}"));
        // While A is stalled mid-request, B must still be served promptly.
        let r = b.read_response_timeout(500);
        assert!(r.is_some(),
            "daemon froze on client A stalled mid-request - C1 refuted, got {r:?}");
        check_response(&r.unwrap());

        // A completes its request -> daemon serves A.
        let rest = [b'.', 2, 0, 0, 0, b'{', b'}'];
        assert!(a.write_raw(&rest));
        let resp_a = a.read_response_timeout(2000).expect("A served after completing");
        check_response(&resp_a);
    });
}

#[test]
fn ipc_zero_cwd_len_served() {
    with_daemon(|| {
        let c = PipeClient::connect(1000).expect("connect");
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_le_bytes()); // cwd_len = 0 is valid
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(b"{}");
        assert!(c.write_raw(&buf));
        let r = c.read_response_timeout(1000);
        assert!(r.is_some(), "daemon disconnected a cwd_len=0 request - C2 refuted, got {r:?}");
        check_response(&r.unwrap());
    });
}

#[test]
fn ipc_fragmented_header_accumulates() {
    with_daemon(|| {
        let c = PipeClient::connect(1000).expect("connect");
        // Split the 4-byte cwd_len header across two writes. The state
        // machine must accumulate partial reads, not disconnect (main.rs:183).
        assert!(c.write_raw(&[1, 0]));
        std::thread::sleep(Duration::from_millis(100));
        assert!(c.write_raw(&[0, 0]));
        assert!(c.write_raw(b"."));
        assert!(c.write_raw(&2u32.to_le_bytes()));
        assert!(c.write_raw(b"{}"));
        let r = c.read_response_timeout(2000);
        assert!(r.is_some(), "fragmented header disconnected the client - C6 refuted, got {r:?}");
        check_response(&r.unwrap());
    });
}

#[test]
fn ipc_fragmented_cwd_accumulates() {
    with_daemon(|| {
        let c = PipeClient::connect(1000).expect("connect");
        assert!(c.write_raw(&5u32.to_le_bytes())); // cwd_len = 5
        std::thread::sleep(Duration::from_millis(100));
        // Only 2 of the 5 cwd bytes; the rest arrive later.
        assert!(c.write_raw(b"ab"));
        std::thread::sleep(Duration::from_millis(200));
        assert!(c.write_raw(b"cde"));
        assert!(c.write_raw(&2u32.to_le_bytes()));
        assert!(c.write_raw(b"{}"));
        let r = c.read_response_timeout(2000);
        assert!(r.is_some(), "fragmented cwd body disconnected the client - C6 refuted, got {r:?}");
        check_response(&r.unwrap());
    });
}

#[test]
fn ipc_git_change_fresh_at_serve() {
    let (_bare_dir, _work_dir, repo_str) = setup_ahead_repo();
    with_daemon_config(
        "format = \"$git_status\"\nadd_newline = false\n[git_status]\nformat = \"$ahead_behind\"\n",
        || {
            let c = PipeClient::connect(1000).expect("connect");
            assert!(c.send_request(&repo_str, "{}"));
            let before = c.read_response().expect("before push");
            assert!(before.contains('⇡'),
                "prompt should show ahead indicator before push, got: {before:?}");

            let t_push = Instant::now();
            common::git(Path::new(&repo_str), &["push"]);

            // RDWC notification delivery is async and the watcher is polled
            // (not event-driven), so the served prompt reflects the push only
            // after the next loop wake. Poll until it clears.
            let deadline = t_push + Duration::from_secs(5);
            let mut stale_polls = 0u32;
            let mut cleared_at = None;
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
                assert!(c.send_request(&repo_str, "{}"));
                let resp = c.read_response().expect("response while polling");
                if resp.contains('⇡') {
                    stale_polls += 1;
                } else {
                    cleared_at = Some(Instant::now());
                    break;
                }
            }
            let elapsed = cleared_at.map(|t| t.duration_since(t_push));
            println!("git-change-fresh: stale prompt for {stale_polls} poll(s), cleared in {elapsed:?} after push");
            assert!(cleared_at.is_some(),
                "served prompt never reflected the push within 5s - C7 refuted");
        },
    );
}

#[test]
fn ipc_firehose_measurement() {
    with_daemon(|| {
        let a = PipeClient::connect(1000).expect("A connect");
        let b = PipeClient::connect(1000).expect("B connect");

        // 30 distinct cwds -> 30 cache misses -> 30 real renders while B waits.
        let mut burst = Vec::new();
        for i in 0..30 {
            let cwd = format!("cwd{i}");
            burst.extend_from_slice(&(cwd.len() as u32).to_le_bytes());
            burst.extend_from_slice(cwd.as_bytes());
            burst.extend_from_slice(&2u32.to_le_bytes());
            burst.extend_from_slice(b"{}");
        }
        assert!(a.write_raw(&burst));
        std::thread::sleep(Duration::from_millis(20));

        let t0 = Instant::now();
        let cwd_len = 1u32.to_le_bytes();
        let props_len = 2u32.to_le_bytes();
        assert!(b.write_raw(&cwd_len));
        assert!(b.write_raw(b"."));
        assert!(b.write_raw(&props_len));
        assert!(b.write_raw(b"{}"));
        let resp_b = b.read_response_timeout(5000);
        let elapsed = t0.elapsed();
        println!("firehose: B's request latency while A drains 30 uncached renders: {elapsed:?}");
        assert!(resp_b.is_some(), "B should eventually be served");

        let mut count = 0;
        while count < 30 {
            if a.read_response_timeout(2000).is_some() {
                count += 1;
            } else {
                break;
            }
        }
        assert_eq!(count, 30, "A should receive all 30 responses, got {count}");
    });
}

/// Runs `f` in a thread, returning its result, or None if it exceeds
/// `timeout_ms`. Converts a daemon-freeze hang into a timed test failure. The
/// worker thread may stay blocked past the timeout; the daemon is killed on
/// drop by the enclosing `with_daemon*`, breaking its pipes and unblocking it.
fn with_hard_timeout<T, F>(timeout_ms: u64, f: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_millis(timeout_ms)).ok()
}

#[test]
fn ipc_unread_responses_do_not_freeze_daemon() {
    // A floods requests but never reads the responses. The daemon's response
    // writes must not block the event loop on the full out-buffer; B must
    // still be served.
    let big_char = ">".repeat(2000);
    let config = format!(
        "format = \"$character\"\nadd_newline = false\n[character]\nformat = \"{big_char}\"\n"
    );
    with_daemon_config(&config, || {
        let a = PipeClient::connect(1000).expect("A connect");
        // ~200 requests (11 bytes each) = ~2.2KB, fits in A's in-buffer. Each
        // response is ~2004 bytes, so ~32 responses fill the 64KB out-buffer.
        // A never drains, so the daemon's writes to A go pending forever.
        let mut burst = Vec::new();
        for _ in 0..200 {
            burst.extend_from_slice(&1u32.to_le_bytes());
            burst.extend_from_slice(b".");
            burst.extend_from_slice(&2u32.to_le_bytes());
            burst.extend_from_slice(b"{}");
        }
        assert!(a.write_raw(&burst), "A should buffer its burst");
        // Give the daemon time to wedge in write_all.
        std::thread::sleep(Duration::from_millis(500));

        let served = with_hard_timeout(7000, || {
            let Some(b) = PipeClient::connect(1000) else { return false; };
            if !b.send_request(".", "{}") { return false; }
            b.read_response_timeout(5000).is_some()
        });
        assert_eq!(served, Some(true),
            "daemon froze on A's unread responses (synchronous write_all)");
    });
}

#[test]
fn ipc_firehose_client_does_not_starve_others() {
    // A streams requests continuously so its in-buffer never drains. The
    // firehose cap self-wakes A via SetEvent and WFMO returns the lowest-index
    // signaled handle, so A monopolizes the loop and B is never serviced. B
    // must still be served while A streams.
    with_daemon(|| {
        let a = PipeClient::connect(1000).expect("A connect");
        // HANDLE is a raw pointer (not Send); pass it as usize (Send) and
        // reconstruct inside the threads. Only writer/reader threads touch the
        // handle, and the daemon stays alive for the whole test, so this is safe.
        let handle = a.handle as usize;
        // Never drop `a`: its Drop closes the handle, which blocks forever if
        // the writer/reader threads are stuck in pending I/O on it (the
        // daemon stops draining A once it is wedged). Leak it; the process
        // cleans up on exit.
        let _a = std::mem::ManuallyDrop::new(a);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        let stop3 = stop.clone();
        let req = {
            let mut r = Vec::new();
            r.extend_from_slice(&1u32.to_le_bytes());
            r.extend_from_slice(b".");
            r.extend_from_slice(&2u32.to_le_bytes());
            r.extend_from_slice(b"{}");
            r
        };
        // Writer floods requests in a tight loop (A's in-buffer stays full).
        // Reader drains responses so the write side never wedges - this
        // isolates the read-side starvation (finding 2).
        let _writer = std::thread::spawn(move || {
            let h = handle as ffi::HANDLE;
            while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
                let mut w: ffi::DWORD = 0;
                unsafe {
                    if ffi::WriteFile(h, req.as_ptr() as *const c_void, req.len() as u32, &mut w, std::ptr::null_mut()) == 0 {
                        break;
                    }
                }
            }
        });
        let _reader = std::thread::spawn(move || {
            let h = handle as ffi::HANDLE;
            while !stop3.load(std::sync::atomic::Ordering::Relaxed) {
                unsafe {
                    let mut avail: ffi::DWORD = 0;
                    if ffi::PeekNamedPipe(h, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut()) == 0 || avail < 4 {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    let mut len_buf = [0u8; 4];
                    let mut r: ffi::DWORD = 0;
                    if ffi::ReadFile(h, len_buf.as_mut_ptr() as *mut c_void, 4, &mut r, std::ptr::null_mut()) == 0 || r != 4 { return; }
                    let body_len = u32::from_le_bytes(len_buf) as usize;
                    let mut body = vec![0u8; body_len];
                    let mut total = 0;
                    while total < body_len {
                        let chunk = (body_len - total) as u32;
                        if ffi::ReadFile(h, body[total..].as_mut_ptr() as *mut c_void, chunk, &mut r, std::ptr::null_mut()) == 0 {
                            return;
                        }
                        total += r as usize;
                    }
                }
            }
        });
        // Let A saturate its pipe.
        std::thread::sleep(Duration::from_millis(500));

        let served = with_hard_timeout(4000, || {
            let Some(b) = PipeClient::connect(1000) else { return false; };
            if !b.send_request(".", "{}") { return false; }
            b.read_response_timeout(1500).is_some()
        });
        // Stop the flood and let the writer/reader threads exit on their own.
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(served, Some(true),
            "daemon starved B while A streamed continuously (firehose self-wake)");
    });
}

#[test]
fn lru_eviction_rotates_slot_at_capacity() {
    // 9 pipe instances: at most 8 serve an active client. The moment a 9th
    // connects, the least-recently-active session is evicted so a free instance
    // always remains for a queued client. The 9th must connect immediately (no
    // idle bound), and the evicted LRU's handle must break.
    // All client work stays inside the closure: PipeClient{handle: *mut c_void}
    // is not Send, so it cannot cross the with_hard_timeout thread boundary.
    with_daemon(|| {
        let result = with_hard_timeout(8000, || {
            let mut clients = Vec::new();
            for i in 0..8 {
                let Some(c) = PipeClient::connect(1000) else { eprintln!("connect {i} failed"); return None; };
                clients.push(c);
            }
            // Round-trip a request through each client in order so the daemon's
            // last_activity stamps are strictly ordered (each completes before the
            // next): clients[0] is the oldest and is the deterministic LRU victim.
            for i in 0..8 {
                if !clients[i].send_request(r"C:\", "{}") || clients[i].read_response_timeout(5000).is_none() {
                    eprintln!("round-trip client {i} failed");
                    return None;
                }
            }
            std::thread::sleep(Duration::from_millis(50));

            let start = Instant::now();
            let Some(ninth) = PipeClient::connect(3000) else { eprintln!("connect 9th failed"); return None; };
            let elapsed = start.elapsed();
            let serviced = ninth.send_request(r"C:\", "{}")
                && ninth.read_response_timeout(5000).is_some();
            let lru_evicted = !clients[0].send_request(r"C:\", "{}");
            let others_alive = (1..8).all(|i| clients[i].send_request(r"C:\", "{}")
                && clients[i].read_response_timeout(5000).is_some());

            // A fresh instance must be listening after the eviction: a 10th
            // client connects immediately and is served.
            let tenth_served = PipeClient::connect(3000)
                .map(|c| c.send_request(r"C:\", "{}") && c.read_response_timeout(5000).is_some())
                .unwrap_or(false);
            Some((elapsed, serviced, lru_evicted, others_alive, tenth_served))
        }).flatten();

        match result {
            Some((elapsed, serviced, lru_evicted, others_alive, tenth_served)) => {
                assert!(elapsed < Duration::from_secs(3),
                    "9th client must connect immediately via the free instance, took {elapsed:?}");
                assert!(serviced, "9th client must be serviced");
                assert!(lru_evicted, "the LRU session must be disconnected to free the instance");
                assert!(others_alive, "a non-LRU session must survive and still be served");
                assert!(tenth_served, "a fresh instance must be listening after eviction (10th client served)");
            }
            None => panic!("connect or service path failed within 8s"),
        }
    });
}
