mod common;

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use starship_daemon::{
    HEADER_LEN, MAX_CONFIG_LEN, MAX_KEYMAP_LEN, MAX_TOTAL_LEN, REQ_PROMPT, REQ_TIMINGS,
    encode_request, ffi, pipe_name,
};

fn unique_pipe_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let name = format!("starship-daemon-test-{}", std::process::id());
        unsafe {
            std::env::set_var("STARSHIP_DAEMON_PIPE", &name);
        }
        name
    })
}

static DAEMON_LOCK: Mutex<()> = Mutex::new(());

fn valid_frame(cwd: &str, status: i32, disable: bool) -> Vec<u8> {
    encode_request(REQ_PROMPT, cwd, status, "", 0, None, disable)
}

fn valid_body(cwd: &str) -> Vec<u8> {
    let frame = valid_frame(cwd, 0, false);
    frame[HEADER_LEN..].to_vec()
}

fn frame_dot_cwd() -> Vec<u8> {
    valid_frame(".", 0, false)
}

struct PipeClient {
    handle: ffi::HANDLE,
}

impl PipeClient {
    fn connect(timeout_ms: u32) -> Option<Self> {
        let deadline = Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
        loop {
            unsafe {
                let wide = ffi::to_wide(&pipe_name());
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

    fn send_request(&self, frame: &[u8]) -> bool {
        unsafe {
            let mut written: ffi::DWORD = 0;
            if ffi::WriteFile(
                self.handle,
                frame.as_ptr() as *const c_void,
                frame.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            ) == 0
            {
                return false;
            }
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
                if ffi::PeekNamedPipe(
                    self.handle,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut avail,
                    std::ptr::null_mut(),
                ) != 0
                    && avail >= 4
                {
                    break;
                }
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        unsafe {
            let mut len_buf = [0u8; 4];
            let mut read: ffi::DWORD = 0;
            if ffi::ReadFile(
                self.handle,
                len_buf.as_mut_ptr() as *mut c_void,
                4,
                &mut read,
                std::ptr::null_mut(),
            ) == 0
                || read != 4
            {
                return None;
            }
            let resp_len = u32::from_le_bytes(len_buf) as usize;
            if resp_len > 65536 {
                return None;
            }
            let mut resp_buf = vec![0u8; resp_len];
            let mut total_read = 0;
            while total_read < resp_len {
                let chunk = (resp_len - total_read) as u32;
                if ffi::ReadFile(
                    self.handle,
                    resp_buf[total_read..].as_mut_ptr() as *mut c_void,
                    chunk,
                    &mut read,
                    std::ptr::null_mut(),
                ) == 0
                {
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
            ffi::WriteFile(
                self.handle,
                bytes.as_ptr() as *const c_void,
                bytes.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            ) != 0
        }
    }
}

impl Drop for PipeClient {
    fn drop(&mut self) {
        unsafe {
            ffi::CloseHandle(self.handle);
        }
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

        let daemon_exe = std::env::var("CARGO_BIN_EXE_starship-daemon").unwrap_or_else(|_| {
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

        DaemonProcess {
            process,
            _config_dir: config_dir,
        }
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
    assert!(
        resp.contains('$') || resp.contains('❮') || resp.contains(">"),
        "response should contain a prompt character, got: {resp:?}"
    );
}

fn expect_disconnect_then_serve<F>(what: &str, write_bad: F)
where
    F: FnOnce(&PipeClient),
{
    let c = PipeClient::connect(1000).expect("connect");
    write_bad(&c);
    let err = c.read_response_timeout(2000);
    assert!(
        err.is_none(),
        "{what} must disconnect (never serve a prompt), got {err:?}"
    );

    std::thread::sleep(std::time::Duration::from_millis(100));
    let c2 = PipeClient::connect(1000).expect("reconnect after disconnect");
    assert!(c2.send_request(&frame_dot_cwd()));
    let resp = c2
        .read_response_timeout(2000)
        .expect("valid request served after disconnect");
    check_response(&resp);
}

#[test]
fn ipc_reconnect() {
    with_daemon(|| {
        let resp1;
        {
            let c = PipeClient::connect(1000).expect("first connect");
            assert!(c.send_request(&frame_dot_cwd()));
            resp1 = c.read_response().expect("first response");
            check_response(&resp1);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        {
            let c = PipeClient::connect(1000).expect("reconnect");
            assert!(c.send_request(&frame_dot_cwd()));
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
            let c = PipeClient::connect(1000).unwrap_or_else(|| panic!("connect attempt {i}"));
            assert!(c.send_request(&valid_frame(".", 0, false)));
            let resp = c.read_response().unwrap_or_else(|| panic!("response {i}"));
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
            ffi::WriteFile(
                raw.handle,
                partial.as_ptr() as *const c_void,
                4,
                &mut written,
                std::ptr::null_mut(),
            );
            ffi::CloseHandle(raw.handle);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));

        let c = PipeClient::connect(1000).expect("reconnect after partial write");
        assert!(c.send_request(&frame_dot_cwd()));
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
            assert!(c.send_request(&valid_frame(".", 0, false)));
            let resp0 = c.read_response().expect("status 0 response");
            assert!(
                resp0.contains("OK"),
                "status 0 prompt should contain OK symbol, got: {resp0:?}"
            );

            let c = PipeClient::connect(1000).expect("reconnect");
            assert!(c.send_request(&valid_frame(".", 1, false)));
            let resp1 = c.read_response().expect("status 1 response");
            assert_ne!(
                resp0, resp1,
                "different status codes must give different prompts"
            );
            assert!(
                resp1.contains("FAIL"),
                "status 1 prompt should contain the FAIL symbol, got: {resp1:?}"
            );
        },
    );
}

#[test]
fn ipc_disable_cache() {
    with_daemon(|| {
        let c = PipeClient::connect(1000).expect("connect");
        assert!(c.send_request(&valid_frame(".", 0, true)));
        let resp = c.read_response().expect("response");
        check_response(&resp);
    });
}

#[test]
fn ipc_sync_drop_race() {
    with_daemon(|| {
        let a = PipeClient::connect(1000).expect("A connect");
        std::thread::sleep(Duration::from_millis(300));

        assert!(a.write_raw(&[REQ_PROMPT, 18, 0, 0]));
        std::thread::sleep(Duration::from_millis(200));

        let b = PipeClient::connect(1000).expect("B connect");
        assert!(b.write_raw(&valid_frame(".", 0, false)));
        std::thread::sleep(Duration::from_millis(200));

        let mut rest = vec![0u8];
        rest.extend_from_slice(&valid_body("."));
        assert!(a.write_raw(&rest));

        let start = Instant::now();
        let resp = b.read_response();
        let elapsed = start.elapsed();
        assert!(
            resp.is_some(),
            "B request dropped by sync-drop race (no response in 5s), elapsed={elapsed:?}"
        );
    });
}

#[test]
fn ipc_git_push_stale_ahead() {
    let (_bare_dir, _work_dir, repo_str) = setup_ahead_repo();

    with_daemon_config(
        "format = \"$git_status\"\nadd_newline = false\n[git_status]\nformat = \"$ahead_behind\"\n",
        || {
            let c = PipeClient::connect(1000).expect("connect");
            assert!(c.send_request(&valid_frame(&repo_str, 0, false)));
            let before_push = c.read_response().expect("before push");
            assert!(
                before_push.contains('⇡'),
                "prompt should show ahead indicator before push, got: {before_push:?}"
            );

            common::git(Path::new(&repo_str), &["push"]);

            let deadline = Instant::now() + Duration::from_secs(5);
            let mut after_push = None;
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
                if let Some(c2) = PipeClient::connect(500)
                    && c2.send_request(&valid_frame(&repo_str, 0, false))
                    && let Some(resp) = c2.read_response()
                {
                    if !resp.contains('⇡') {
                        after_push = Some(resp);
                        break;
                    }
                    after_push = Some(resp);
                }
            }
            let after_push = after_push.expect("after push (no response within 5s)");
            assert!(
                !after_push.contains('⇡'),
                "prompt should NOT contain ⇡ after push (no longer ahead)"
            );
        },
    );
}

fn setup_ahead_repo() -> (tempfile::TempDir, tempfile::TempDir, String) {
    let remote = common::remote_with_worktree("repo");
    common::settle();

    std::fs::write(remote.path.join("ahead.txt"), "ahead").unwrap();
    common::git(&remote.path, &["add", "ahead.txt"]);
    common::git(&remote.path, &["commit", "-m", "ahead"]);

    let repo_str = remote.path.to_str().unwrap().to_string();
    (remote.bare, remote.work, repo_str)
}

#[test]
fn ipc_stalled_client_does_not_freeze_daemon() {
    with_daemon(|| {
        let a = PipeClient::connect(1000).expect("A connect");
        assert!(a.write_raw(&[REQ_PROMPT, 18, 0, 0]));
        std::thread::sleep(Duration::from_millis(300));

        let b = PipeClient::connect(1000).expect("B connect");
        assert!(b.write_raw(&valid_frame(".", 0, false)));
        let r = b.read_response_timeout(500);
        assert!(
            r.is_some(),
            "daemon froze on client A stalled mid-request, got {r:?}"
        );
        check_response(&r.unwrap());

        let mut rest = vec![0u8];
        rest.extend_from_slice(&valid_body("."));
        assert!(a.write_raw(&rest));
        let resp_a = a
            .read_response_timeout(2000)
            .expect("A served after completing");
        check_response(&resp_a);
    });
}

#[test]
fn ipc_zero_cwd_len_served() {
    with_daemon(|| {
        let c = PipeClient::connect(1000).expect("connect");
        assert!(c.write_raw(&valid_frame("", 0, false)));
        let r = c.read_response_timeout(1000);
        assert!(
            r.is_some(),
            "daemon disconnected a cwd_len=0 request, got {r:?}"
        );
        check_response(&r.unwrap());
    });
}

#[test]
fn ipc_fragmented_header_accumulates() {
    with_daemon(|| {
        let c = PipeClient::connect(1000).expect("connect");
        assert!(c.write_raw(&[REQ_PROMPT, 18]));
        std::thread::sleep(Duration::from_millis(100));
        assert!(c.write_raw(&[0, 0]));
        assert!(c.write_raw(&[0]));
        assert!(c.write_raw(&valid_body(".")));
        let r = c.read_response_timeout(2000);
        assert!(
            r.is_some(),
            "fragmented header disconnected the client, got {r:?}"
        );
        check_response(&r.unwrap());
    });
}

#[test]
fn ipc_fragmented_cwd_accumulates() {
    with_daemon(|| {
        let c = PipeClient::connect(1000).expect("connect");
        assert!(c.write_raw(&[REQ_PROMPT, 22, 0, 0, 0]));
        std::thread::sleep(Duration::from_millis(100));
        let body = valid_body("abcde");
        assert_eq!(body.len(), 22);
        assert!(c.write_raw(&body[..6]));
        std::thread::sleep(Duration::from_millis(200));
        assert!(c.write_raw(&body[6..]));
        let r = c.read_response_timeout(2000);
        assert!(
            r.is_some(),
            "fragmented cwd body disconnected the client, got {r:?}"
        );
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
            assert!(c.send_request(&valid_frame(&repo_str, 0, false)));
            let before = c.read_response().expect("before push");
            assert!(
                before.contains('⇡'),
                "prompt should show ahead indicator before push, got: {before:?}"
            );

            let t_push = Instant::now();
            common::git(Path::new(&repo_str), &["push"]);

            let deadline = t_push + Duration::from_secs(5);
            let mut stale_polls = 0u32;
            let mut cleared_at = None;
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
                assert!(c.send_request(&valid_frame(&repo_str, 0, false)));
                let resp = c.read_response().expect("response while polling");
                if resp.contains('⇡') {
                    stale_polls += 1;
                } else {
                    cleared_at = Some(Instant::now());
                    break;
                }
            }
            let elapsed = cleared_at.map(|t| t.duration_since(t_push));
            println!(
                "git-change-fresh: stale prompt for {stale_polls} poll(s), cleared in {elapsed:?} after push"
            );
            assert!(
                cleared_at.is_some(),
                "served prompt never reflected the push within 5s"
            );
        },
    );
}

#[test]
fn ipc_firehose_measurement() {
    with_daemon(|| {
        let a = PipeClient::connect(1000).expect("A connect");
        let b = PipeClient::connect(1000).expect("B connect");

        let mut burst = Vec::new();
        for i in 0..30 {
            let cwd = format!("cwd{i}");
            burst.extend_from_slice(&valid_frame(&cwd, 0, false));
        }
        assert!(a.write_raw(&burst));
        std::thread::sleep(Duration::from_millis(20));

        let t0 = Instant::now();
        assert!(b.write_raw(&valid_frame(".", 0, false)));
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
    let big_char = ">".repeat(2000);
    let config = format!(
        "format = \"$character\"\nadd_newline = false\n[character]\nformat = \"{big_char}\"\n"
    );
    with_daemon_config(&config, || {
        let a = PipeClient::connect(1000).expect("A connect");
        let mut burst = Vec::new();
        for _ in 0..200 {
            burst.extend_from_slice(&valid_frame(".", 0, false));
        }
        assert!(a.write_raw(&burst), "A should buffer its burst");
        std::thread::sleep(Duration::from_millis(500));

        let served = with_hard_timeout(7000, || {
            let Some(b) = PipeClient::connect(1000) else {
                return false;
            };
            if !b.send_request(&frame_dot_cwd()) {
                return false;
            }
            b.read_response_timeout(5000).is_some()
        });
        assert_eq!(served, Some(true), "daemon froze on A's unread responses");
    });
}

#[test]
fn ipc_firehose_client_does_not_starve_others() {
    with_daemon(|| {
        let a = PipeClient::connect(1000).expect("A connect");
        let handle = a.handle as usize;
        let _a = std::mem::ManuallyDrop::new(a);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        let stop3 = stop.clone();
        let req = valid_frame(".", 0, false);
        let _writer = std::thread::spawn(move || {
            let h = handle as ffi::HANDLE;
            while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
                let mut w: ffi::DWORD = 0;
                unsafe {
                    if ffi::WriteFile(
                        h,
                        req.as_ptr() as *const c_void,
                        req.len() as u32,
                        &mut w,
                        std::ptr::null_mut(),
                    ) == 0
                    {
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
                    if ffi::PeekNamedPipe(
                        h,
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        &mut avail,
                        std::ptr::null_mut(),
                    ) == 0
                        || avail < 4
                    {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    let mut len_buf = [0u8; 4];
                    let mut r: ffi::DWORD = 0;
                    if ffi::ReadFile(
                        h,
                        len_buf.as_mut_ptr() as *mut c_void,
                        4,
                        &mut r,
                        std::ptr::null_mut(),
                    ) == 0
                        || r != 4
                    {
                        return;
                    }
                    let body_len = u32::from_le_bytes(len_buf) as usize;
                    let mut body = vec![0u8; body_len];
                    let mut total = 0;
                    while total < body_len {
                        let chunk = (body_len - total) as u32;
                        if ffi::ReadFile(
                            h,
                            body[total..].as_mut_ptr() as *mut c_void,
                            chunk,
                            &mut r,
                            std::ptr::null_mut(),
                        ) == 0
                        {
                            return;
                        }
                        total += r as usize;
                    }
                }
            }
        });
        std::thread::sleep(Duration::from_millis(500));

        let served = with_hard_timeout(4000, || {
            let Some(b) = PipeClient::connect(1000) else {
                return false;
            };
            if !b.send_request(&frame_dot_cwd()) {
                return false;
            }
            b.read_response_timeout(1500).is_some()
        });
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            served,
            Some(true),
            "daemon starved B while A streamed continuously (firehose self-wake)"
        );
    });
}

#[test]
fn lru_eviction_rotates_slot_at_capacity() {
    with_daemon(|| {
        let result = with_hard_timeout(8000, || {
            let mut clients = Vec::new();
            for i in 0..8 {
                let Some(c) = PipeClient::connect(1000) else {
                    eprintln!("connect {i} failed");
                    return None;
                };
                clients.push(c);
            }
            for (i, client) in clients.iter().enumerate().take(8) {
                if !client.send_request(&frame_dot_cwd())
                    || client.read_response_timeout(5000).is_none()
                {
                    eprintln!("round-trip client {i} failed");
                    return None;
                }
            }
            std::thread::sleep(Duration::from_millis(50));

            let start = Instant::now();
            let Some(ninth) = PipeClient::connect(3000) else {
                eprintln!("connect 9th failed");
                return None;
            };
            let elapsed = start.elapsed();
            let serviced =
                ninth.send_request(&frame_dot_cwd()) && ninth.read_response_timeout(5000).is_some();
            let lru_evicted = !clients[0].send_request(&frame_dot_cwd());
            let others_alive = (1..8).all(|i| {
                clients[i].send_request(&frame_dot_cwd())
                    && clients[i].read_response_timeout(5000).is_some()
            });

            let tenth_served = PipeClient::connect(3000)
                .map(|c| {
                    c.send_request(&frame_dot_cwd()) && c.read_response_timeout(5000).is_some()
                })
                .unwrap_or(false);
            Some((elapsed, serviced, lru_evicted, others_alive, tenth_served))
        })
        .flatten();

        match result {
            Some((elapsed, serviced, lru_evicted, others_alive, tenth_served)) => {
                assert!(
                    elapsed < Duration::from_secs(3),
                    "9th client must connect immediately via the free instance, took {elapsed:?}"
                );
                assert!(serviced, "9th client must be serviced");
                assert!(
                    lru_evicted,
                    "the LRU session must be disconnected to free the instance"
                );
                assert!(
                    others_alive,
                    "a non-LRU session must survive and still be served"
                );
                assert!(
                    tenth_served,
                    "a fresh instance must be listening after eviction (10th client served)"
                );
            }
            None => panic!("connect or service path failed within 8s"),
        }
    });
}

#[test]
fn ipc_bad_type_disconnects_then_serves() {
    with_daemon(|| {
        expect_disconnect_then_serve("bad request type", |c| {
            let mut frame = valid_frame(".", 0, false);
            frame[0] = 0xFF;
            assert!(c.write_raw(&frame));
        });
    });
}

#[test]
fn ipc_timings_served_then_session_reusable() {
    with_daemon(|| {
        let Some(c) = PipeClient::connect(10_000) else {
            panic!("connect failed");
        };
        let frame = encode_request(REQ_TIMINGS, ".", 0, "", 120, None, false);
        assert!(c.write_raw(&frame));
        let resp = c
            .read_response_timeout(60_000)
            .expect("timings response must arrive");
        assert!(
            resp.contains("Here are the timings of modules in your prompt"),
            "report must contain module table header, got: {resp}"
        );
        assert!(
            resp.contains("Render path timing"),
            "report must contain render-path section, got: {resp}"
        );

        assert!(c.send_request(&frame_dot_cwd()));
        let served = c
            .read_response_timeout(30_000)
            .expect("prompt after timings");
        assert!(
            !served.is_empty() && (served.contains('$') || served.contains('>')),
            "same session must still serve a prompt after a timings report, got: {served}"
        );
    });
}

#[test]
fn ipc_timings_git_repo_reports_git_modules() {
    let repo = common::TestRepo::new();
    let repo_path = repo.path().to_str().unwrap().to_string();
    with_daemon_config(
        "format = \"$git_branch$git_status\"\nadd_newline = false\n",
        || {
            let Some(c) = PipeClient::connect(10_000) else {
                panic!("connect failed");
            };
            let frame = encode_request(REQ_TIMINGS, &repo_path, 0, "", 120, None, false);
            assert!(c.write_raw(&frame));
            let resp = c
                .read_response_timeout(60_000)
                .expect("timings response must arrive");
            assert!(
                resp.contains("Render path timing"),
                "report must contain render-path section, got: {resp}"
            );
            assert!(
                resp.contains("git_branch"),
                "git repo report must contain a git_branch row, got: {resp}"
            );
            assert!(
                resp.contains("cache: HIT") || resp.contains("cache: MISS"),
                "report must contain a cache line, got: {resp}"
            );
        },
    );
}

#[test]
fn ipc_tail_eating_cwd_disconnects_then_serves() {
    with_daemon(|| {
        expect_disconnect_then_serve("tail-eating cwd", |c| {
            let mut body = Vec::new();
            body.extend_from_slice(&13u32.to_le_bytes());
            body.resize(17, 0);
            let mut frame = Vec::new();
            frame.push(REQ_PROMPT);
            frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
            frame.extend_from_slice(&body);
            assert!(c.write_raw(&frame));
        });
    });
}

#[test]
fn ipc_total_len_over_cap_disconnects_then_serves() {
    with_daemon(|| {
        expect_disconnect_then_serve("over-cap total_len", |c| {
            let mut frame = vec![REQ_PROMPT];
            frame.extend_from_slice(&(MAX_TOTAL_LEN as u32 + 1).to_le_bytes());
            assert!(c.write_raw(&frame));
        });
    });
}

#[test]
fn ipc_trailing_body_bytes_tolerated() {
    with_daemon(|| {
        let c = PipeClient::connect(1000).expect("connect");
        let mut body = valid_body(".");
        body.extend_from_slice(b"future-field");
        let mut frame = vec![REQ_PROMPT];
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(&body);
        assert!(c.write_raw(&frame));
        let resp = c
            .read_response_timeout(2000)
            .expect("trailing body bytes must be ignored, not disconnect");
        check_response(&resp);
    });
}

#[test]
fn ipc_keymap_over_cap_disconnects_then_serves() {
    with_daemon(|| {
        expect_disconnect_then_serve("keymap over MAX_KEYMAP_LEN", |c| {
            let keymap = "k".repeat(MAX_KEYMAP_LEN + 1);
            let mut body = Vec::new();
            body.extend_from_slice(&1u32.to_le_bytes());
            body.push(b'.');
            body.extend_from_slice(&0i32.to_le_bytes());
            body.extend_from_slice(&(keymap.len() as u16).to_le_bytes());
            body.extend_from_slice(keymap.as_bytes());
            body.extend_from_slice(&0u32.to_le_bytes());
            body.extend_from_slice(&0u16.to_le_bytes());
            body.push(0);
            let mut frame = vec![REQ_PROMPT];
            frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
            frame.extend_from_slice(&body);
            assert!(c.write_raw(&frame));
        });
    });
}

#[test]
fn ipc_config_over_cap_disconnects_then_serves() {
    with_daemon(|| {
        expect_disconnect_then_serve("config over MAX_CONFIG_LEN", |c| {
            let config = "c".repeat(MAX_CONFIG_LEN + 1);
            let mut body = Vec::new();
            body.extend_from_slice(&1u32.to_le_bytes());
            body.push(b'.');
            body.extend_from_slice(&0i32.to_le_bytes());
            body.extend_from_slice(&0u16.to_le_bytes());
            body.extend_from_slice(&0u32.to_le_bytes());
            body.extend_from_slice(&(config.len() as u16).to_le_bytes());
            body.extend_from_slice(config.as_bytes());
            body.push(0);
            let mut frame = vec![REQ_PROMPT];
            frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
            frame.extend_from_slice(&body);
            assert!(c.write_raw(&frame));
        });
    });
}

#[test]
fn ipc_empty_format_serves_zero_len_prompt() {
    with_daemon_config("format = \"\"\nadd_newline = false\n", || {
        let c = PipeClient::connect(1000).expect("connect");
        assert!(c.send_request(&frame_dot_cwd()));
        let resp = c.read_response_timeout(2000);
        assert_eq!(
            resp,
            Some(String::new()),
            "format = \"\" must render an empty (zero-length) prompt, got {resp:?}"
        );
    });
}
