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
        let deadline = Instant::now() + Duration::from_secs(5);
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
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
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
