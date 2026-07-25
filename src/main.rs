use std::collections::HashMap;
use std::ffi::c_void;
use std::mem;
use std::path::{Path, PathBuf};

use fast_git_status::watcher::*;
use serde::Deserialize;

use starship_daemon::prompt::{self, ModuleConfig, RenderContext};

// -- Client request properties (JSON) -----------------------------------

#[derive(Debug, Deserialize)]
struct ClientProps {
    status_code: Option<i32>,
    keymap: Option<String>,
    cmd_duration_ms: Option<u64>,
    jobs: Option<i64>,
    shlvl: Option<i64>,
    terminal_width: Option<usize>,
    starship_config: Option<String>,
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
    dir_handle: HANDLE,
    change_buf: *mut u8,
    overlapped: OVERLAPPED,
    change_event: HANDLE,
    config_path: PathBuf,
}

impl ConfigWatch {
    fn new(config_path: &Path) -> Option<Self> {
        let dir = config_path.parent()?;
        let wide = to_wide(&dir.to_string_lossy());
        unsafe {
            let dir_handle = CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            );
            if dir_handle == INVALID_HANDLE_VALUE { return None; }

            let change_event = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
            if change_event.is_null() {
                CloseHandle(dir_handle);
                return None;
            }

            let change_buf = alloc_change_buf();
            if change_buf.is_null() {
                CloseHandle(dir_handle);
                CloseHandle(change_event);
                return None;
            }

            let mut cw = ConfigWatch {
                dir_handle,
                change_buf,
                overlapped: mem::zeroed(),
                change_event,
                config_path: config_path.to_path_buf(),
            };
            cw.start();
            Some(cw)
        }
    }

    fn start(&mut self) {
        unsafe {
            ResetEvent(self.change_event);
            self.overlapped = mem::zeroed();
            self.overlapped.h_event = self.change_event;
            let mut bytes: DWORD = 0;
            ReadDirectoryChangesW(
                self.dir_handle,
                self.change_buf as LPVOID,
                CHANGE_BUF_SIZE,
                0, // not recursive, just the config dir
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE,
                &mut bytes,
                &mut self.overlapped as *mut _ as *mut c_void,
                std::ptr::null(),
            );
        }
    }

    fn check_event(&mut self) -> bool {
        if self.change_event.is_null() { return false; }
        let rc = unsafe {
            WaitForSingleObject(self.change_event, 0)
        };
        if rc == WAIT_OBJECT_0 {
            // Consume the notification
            unsafe {
                let mut bytes: DWORD = 0;
                GetOverlappedResult(
                    self.dir_handle,
                    &mut self.overlapped as *mut _ as *mut c_void,
                    &mut bytes,
                    0,
                );
            }
            let config_name = self.config_path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let paths = extract_watcher_paths(self.change_buf, CHANGE_BUF_SIZE as usize);
            let matched = paths.iter().any(|(name, _)| name == &config_name);
            self.start(); // re-arm
            return matched;
        }
        false
    }
}

impl Drop for ConfigWatch {
    fn drop(&mut self) {
        unsafe {
            if self.dir_handle != INVALID_HANDLE_VALUE {
                CloseHandle(self.dir_handle);
            }
            if !self.change_buf.is_null() {
                free_change_buf(self.change_buf);
            }
            CloseHandle(self.change_event);
        }
    }
}

// -- Main ----------------------------------------------------------------

fn main() {
    // Load config
    let config_path = prompt::default_config_path();
    let mut module_config = match prompt::load_config(&config_path) {
        Some(cfg) => cfg,
        None => {
            eprintln!("Warning: could not load starship config. Exiting.");
            std::process::exit(1);
        }
    };

    // Start config file watcher
    let mut config_watch = ConfigWatch::new(&config_path);

    // Full prompt cache
    let mut prompt_cache: HashMap<CacheKey, String> = HashMap::new();

    // Create named pipe
    let wide = to_wide(starship_daemon::PIPE_NAME);
    let pipe = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_MESSAGE | PIPE_WAIT,
            1,
            65536,
            65536,
            0,
            std::ptr::null(),
        )
    };

    if pipe == INVALID_HANDLE_VALUE {
        std::process::exit(0);
    }

    let connect_event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };

    let mut repos: Vec<RepoWatch> = Vec::new();
    let mut connect_ol: OVERLAPPED = unsafe { mem::zeroed() };

    issue_connect_inner(pipe, &mut connect_ol, connect_event);

    println!("starship-daemon started on {}", starship_daemon::PIPE_NAME);

    loop {
        // Collect handles: [connect_event, config_watch_event, ...repo_events]
        let config_evt = config_watch.as_ref().map_or(std::ptr::null_mut(), |cw| cw.change_event);
        let mut handles = Vec::with_capacity(2 + repos.len());
        handles.push(connect_event);
        handles.push(config_evt);
        for rw in &repos {
            handles.push(rw.change_event);
        }

        let rc = unsafe {
            WaitForMultipleObjects(
                handles.len() as DWORD,
                handles.as_ptr(),
                0,
                DIRTY_COOLDOWN_MS as DWORD,
            )
        };

        if rc == WAIT_TIMEOUT {
            process_dirty(&mut repos);
            gc(&mut repos);
            continue;
        }

        let idx = (rc - WAIT_OBJECT_0) as usize;

        if idx == 0 {
            // Pipe connect event — handle client
            let _ = handle_client(pipe, &mut repos, &mut module_config, &mut prompt_cache);
            issue_connect_inner(pipe, &mut connect_ol, connect_event);
        } else if idx == 1 {
            // Config watch event
            if let Some(ref mut cw) = config_watch {
                if cw.check_event() {
                    // Config file changed — reload
                    if let Some(new_cfg) = prompt::load_config(&config_path) {
                        module_config = new_cfg;
                        prompt_cache.clear();
                        eprintln!("Config reloaded");
                    }
                }
            }
        } else {
            // Repo watcher event
            let repo_idx = idx - 2;
            if repo_idx < repos.len() {
                handle_watcher_event(&mut repos[repo_idx]);
            }
        }
    }
}

// -- Client handler ------------------------------------------------------

fn handle_client(
    pipe: HANDLE,
    repos: &mut Vec<RepoWatch>,
    module_config: &mut ModuleConfig,
    prompt_cache: &mut HashMap<CacheKey, String>,
) -> Result<(), ()> {
    let mut hdr = [0u8; 4];

    if !read_exact(pipe, &mut hdr) { unsafe { DisconnectNamedPipe(pipe); } return Err(()); }
    let cwd_len = u32::from_le_bytes(hdr) as usize;
    if cwd_len > 32768 { unsafe { DisconnectNamedPipe(pipe); } return Err(()); }

    let mut cwd_bytes = vec![0u8; cwd_len];
    if !read_exact(pipe, &mut cwd_bytes) { unsafe { DisconnectNamedPipe(pipe); } return Err(()); }

    if !read_exact(pipe, &mut hdr) { unsafe { DisconnectNamedPipe(pipe); } return Err(()); }
    let props_len = u32::from_le_bytes(hdr) as usize;
    if props_len > 4096 { unsafe { DisconnectNamedPipe(pipe); } return Err(()); }

    let mut props_bytes = vec![0u8; props_len];
    if !read_exact(pipe, &mut props_bytes) { unsafe { DisconnectNamedPipe(pipe); } return Err(()); }

    let cwd = PathBuf::from(String::from_utf8_lossy(&cwd_bytes).as_ref());

    // Parse client properties
    let props: ClientProps = match serde_json::from_slice(&props_bytes) {
        Ok(p) => p,
        Err(_) => { unsafe { DisconnectNamedPipe(pipe); } return Err(()); }
    };

    let status_code = props.status_code.unwrap_or(0);
    let keymap = props.keymap.unwrap_or_else(|| "vi".to_string());
    let cmd_duration = props.cmd_duration_ms;
    let jobs = props.jobs.unwrap_or(0);
    let shlvl = props.shlvl;
    let terminal_width = props.terminal_width.unwrap_or(120);

    // Reload config if client requests a different one
    if let Some(ref req_config) = props.starship_config {
        let req_path = PathBuf::from(req_config);
        if req_path != module_config.config_path {
            if let Some(new_cfg) = prompt::load_config(&req_path) {
                *module_config = new_cfg;
                prompt_cache.clear();
                // Update env so starship's internal get_prompt() uses the new config
                std::env::set_var("STARSHIP_CONFIG", &req_path);
            }
        }
    }

    // Check prompt cache
    let time_bucket = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0);

    let cache_key = CacheKey {
        cwd: cwd.clone(),
        status_code,
        keymap: keymap.clone(),
        time_bucket,
    };

    if let Some(cached) = prompt_cache.get(&cache_key) {
        let resp_bytes = cached.as_bytes();
        let resp_len = (resp_bytes.len() as u32).to_le_bytes();
        write_all(pipe, &resp_len);
        write_all(pipe, resp_bytes);
        unsafe {
            let mut drain = [0u8; 4];
            let mut read: DWORD = 0;
            ReadFile(pipe, drain.as_mut_ptr() as LPVOID, 4, &mut read, std::ptr::null_mut());
            DisconnectNamedPipe(pipe);
        }
        return Ok(());
    }

    // Ensure repo watch exists for this cwd (warms gix caches)
    let _rw_idx = find_or_create_repo(repos, &cwd);

    // Build render context
    let render_ctx = RenderContext {
        cwd: cwd.clone(),
        terminal_width,
        status_code,
        keymap,
        cmd_duration: cmd_duration.map(|ms| ms as u128),
        jobs,
        shlvl,
    };

    // Render the prompt
    let output = prompt::render_prompt(&render_ctx);

    // Cache the result
    prompt_cache.insert(cache_key, output.clone());

    // Send response
    let resp_bytes = output.as_bytes();
    let resp_len = (resp_bytes.len() as u32).to_le_bytes();
    write_all(pipe, &resp_len);
    write_all(pipe, resp_bytes);

    // Drain remaining data and disconnect
    unsafe {
        let mut drain = [0u8; 4];
        let mut read: DWORD = 0;
        ReadFile(pipe, drain.as_mut_ptr() as LPVOID, 4, &mut read, std::ptr::null_mut());
        DisconnectNamedPipe(pipe);
    }

    Ok(())
}
