use std::path::{Path, PathBuf};

use lru::LruCache;

use crate::cache::{self, CacheKey, CachedValue, RenderContext};
use crate::watch::WatcherState;
use crate::ParsedRequest;

#[derive(Debug)]
pub enum DaemonError {
    ConfigNotFound,
    BadFrame,
}

pub struct DaemonState {
    pub config_path: PathBuf,
    pub cached_config: toml::Table,
    pub last_cfg_mtime: u64,
    pub lru: LruCache<CacheKey, CachedValue>,
    pub watcher: WatcherState,
}

impl DaemonState {
    pub fn new(config_path: PathBuf) -> Result<Self, DaemonError> {
        if !config_path.is_file() {
            return Err(DaemonError::ConfigNotFound);
        }
        let cached_config = cache::read_config(&config_path);
        let last_cfg_mtime = cache::get_mtime_ns(&config_path);
        let lru = LruCache::new(std::num::NonZeroUsize::new(256).unwrap());
        Ok(DaemonState { config_path, cached_config, last_cfg_mtime, lru, watcher: WatcherState::new() })
    }

    pub fn warm_up(&mut self) {
        let warm_ctx = RenderContext {
            cwd: PathBuf::from("."),
            terminal_width: 120,
            status_code: 0,
            keymap: "vi".to_string(),
        };
        let warm_key = cache::compute_cache_key(Path::new("."), "vi", 120, 0, 0);
        let _ = cache::render_cached(&warm_ctx, None, &self.cached_config, &warm_key, &mut self.lru);
    }

    pub fn reload_config(&mut self) {
        self.cached_config = cache::read_config(&self.config_path);
        self.lru.clear();
        cache::clear_repo_cache();
        self.last_cfg_mtime = cache::get_mtime_ns(&self.config_path);
    }

    pub fn handle(&mut self, frame: &[u8]) -> Result<String, DaemonError> {
        let ParsedRequest { cwd, props } = match crate::parse_request(frame) {
            Some(r) => r,
            None => return Err(DaemonError::BadFrame),
        };

        let cwd = if cwd.as_os_str().is_empty() { PathBuf::from(".") } else { cwd };
        let git_dir = crate::find_git_dir(&cwd);
        let status_code = props.status_code;
        let keymap = props.keymap.unwrap_or_else(|| "vi".to_string());
        let config_mtime = self.sync_config(props.starship_config.as_deref());
        let ctx = RenderContext { cwd: cwd.clone(), terminal_width: props.terminal_width, status_code, keymap };
        let output = self.render_prompt(&ctx, git_dir.as_deref(), props.disable_cache, config_mtime);
        Ok(output)
    }

    fn sync_config(&mut self, requested: Option<&str>) -> u64 {
        if let Some(req) = requested {
            let p = Path::new(req);
            if p != self.config_path.as_path() {
                if let Some(new_cfg) = cache::load_config(p) {
                    self.config_path = new_cfg;
                    unsafe { std::env::set_var("STARSHIP_CONFIG", req); }
                    self.reload_config();
                    return self.last_cfg_mtime;
                }
            }
        }
        let mtime = cache::get_mtime_ns(&self.config_path);
        if mtime != self.last_cfg_mtime {
            self.reload_config();
        }
        mtime
    }

    fn render_prompt(&mut self, ctx: &RenderContext, git_dir: Option<&Path>, disable_cache: bool, config_mtime: u64) -> String {
        if disable_cache {
            return cache::render_prompt_with_config(ctx, git_dir, &self.cached_config);
        }
        let watcher_version = if let Some(repo) = git_dir.and_then(Path::parent) {
            self.watcher.ensure(repo);
            self.watcher.flush();
            self.watcher.version(repo)
        } else {
            0
        };
        let key = cache::compute_cache_key(&ctx.cwd, &ctx.keymap, ctx.terminal_width, config_mtime, watcher_version);
        cache::render_cached(ctx, git_dir, &self.cached_config, &key, &mut self.lru)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard(Option<String>);
    impl EnvGuard {
        fn new() -> Self {
            EnvGuard(std::env::var("STARSHIP_CONFIG").ok())
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(p) => unsafe { std::env::set_var("STARSHIP_CONFIG", p); },
                None => unsafe { std::env::remove_var("STARSHIP_CONFIG") },
            }
        }
    }

    fn frame(cwd: &str, status: i32, keymap: &str, width: u32, config: Option<&str>, disable: bool) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&(cwd.len() as u32).to_le_bytes());
        body.extend_from_slice(cwd.as_bytes());
        body.extend_from_slice(&status.to_le_bytes());
        body.extend_from_slice(&(keymap.len() as u16).to_le_bytes());
        body.extend_from_slice(keymap.as_bytes());
        body.extend_from_slice(&width.to_le_bytes());
        match config {
            Some(c) => {
                body.extend_from_slice(&(c.len() as u16).to_le_bytes());
                body.extend_from_slice(c.as_bytes());
            }
            None => body.extend_from_slice(&0u16.to_le_bytes()),
        }
        body.push(disable as u8);
        let mut out = Vec::new();
        out.push(crate::PROTO_VERSION);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    fn char_config(dir: &tempfile::TempDir, symbol: &str) -> PathBuf {
        let p = dir.path().join("starship.toml");
        std::fs::write(&p, format!(
            "format = \"$character\"\nadd_newline = false\n[character]\nformat = \"{symbol}\"\n"
        )).unwrap();
        p
    }

    fn work_dir(dir: &tempfile::TempDir) -> PathBuf {
        let w = dir.path().join("work");
        std::fs::create_dir_all(&w).unwrap();
        w
    }

    #[test]
    fn new_missing_config_is_error() {
        assert!(matches!(DaemonState::new(PathBuf::from("__nonexistent__")), Err(DaemonError::ConfigNotFound)));
    }

    #[test]
    fn bad_frame_is_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg = char_config(&dir, ">");
        let mut d = DaemonState::new(cfg).unwrap();
        assert!(matches!(d.handle(&[0u8; 16]), Err(DaemonError::BadFrame)));
        assert!(matches!(d.handle(b"garbage"), Err(DaemonError::BadFrame)));
    }

    #[test]
    fn renders_and_cache_hits_on_identical_frame() {
        let dir = tempfile::TempDir::new().unwrap();
        let cwd = work_dir(&dir);
        let cfg = char_config(&dir, ">");
        let mut d = DaemonState::new(cfg).unwrap();
        let req = frame(&cwd.to_string_lossy(), 0, "", 120, None, false);
        let out1 = d.handle(&req).unwrap();
        assert_eq!(out1, ">");
        let before = d.lru.len();
        let out2 = d.handle(&req).unwrap();
        assert_eq!(out2, ">");
        assert_eq!(d.lru.len(), before, "identical frame must be a cache hit");
    }

    #[test]
    fn config_mtime_edit_reloads() {
        let dir = tempfile::TempDir::new().unwrap();
        let cwd = work_dir(&dir);
        let cfg = char_config(&dir, ">");
        let mut d = DaemonState::new(cfg.clone()).unwrap();
        let req = frame(&cwd.to_string_lossy(), 0, "", 120, None, false);
        assert_eq!(d.handle(&req).unwrap(), ">");

        let m0 = crate::cache::get_mtime_ns(&cfg);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            std::fs::write(&cfg, "format = \"$character\"\nadd_newline = false\n[character]\nformat = \"x\"\n").unwrap();
            if crate::cache::get_mtime_ns(&cfg) != m0 {
                break;
            }
            assert!(Instant::now() < deadline, "config mtime never changed");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(d.handle(&req).unwrap(), "x");
    }

    #[test]
    fn explicit_config_change_repoints_env_and_reloads() {
        let dir = tempfile::TempDir::new().unwrap();
        let cwd = work_dir(&dir);
        let cfg_a = char_config(&dir, ">");
        let cfg_b = dir.path().join("other.toml");
        std::fs::write(&cfg_b, "format = \"$character\"\nadd_newline = false\n[character]\nformat = \"x\"\n").unwrap();

        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _env = EnvGuard::new();
        let mut d = DaemonState::new(cfg_a).unwrap();
        let req = frame(&cwd.to_string_lossy(), 0, "", 120, Some(&cfg_b.to_string_lossy()), false);
        assert_eq!(d.handle(&req).unwrap(), "x");
        assert_eq!(std::env::var("STARSHIP_CONFIG").unwrap(), cfg_b.to_string_lossy());
    }

    #[test]
    fn disable_cache_renders_fresh() {
        let dir = tempfile::TempDir::new().unwrap();
        let cwd = work_dir(&dir);
        let cfg = char_config(&dir, ">");
        let mut d = DaemonState::new(cfg).unwrap();
        let req = frame(&cwd.to_string_lossy(), 0, "", 120, None, true);
        assert_eq!(d.handle(&req).unwrap(), ">");
        assert_eq!(d.lru.len(), 0, "disable_cache must bypass the render cache");
    }

    #[cfg(fork_starship)]
    fn git_cmd(repo: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C").arg(repo)
            .args(args)
            .output()
            .expect("git command failed");
        assert!(out.status.success(), "git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    }

    #[cfg(fork_starship)]
    fn wait_for_bump(w: &mut WatcherState, repo: &std::path::Path, before: u64) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            w.poll();
            if w.version(repo) > before { return; }
            std::thread::sleep(Duration::from_millis(20));
            assert!(Instant::now() < deadline, "repo version did not bump within 5s");
        }
    }

    #[cfg(fork_starship)]
    #[test]
    fn watcher_through_handle_invalidates_cache() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_cmd(&repo, &["init"]);
        git_cmd(&repo, &["branch", "-M", "main"]);
        git_cmd(&repo, &["config", "user.email", "test@test"]);
        git_cmd(&repo, &["config", "user.name", "test"]);
        std::fs::write(repo.join("a.txt"), "hello").unwrap();
        git_cmd(&repo, &["add", "a.txt"]);
        git_cmd(&repo, &["commit", "-m", "initial"]);
        git_cmd(&repo, &["branch", "other"]);
        let cfg = dir.path().join("starship.toml");
        std::fs::write(&cfg, "format = \"$git_branch\"\nadd_newline = false\n[git_branch]\nformat = \"$branch\"\n").unwrap();

        let mut d = DaemonState::new(cfg).unwrap();
        let req = frame(&repo.to_string_lossy(), 0, "", 120, None, false);
        assert_eq!(d.handle(&req).unwrap(), "main");

        git_cmd(&repo, &["checkout", "other"]);
        let before = d.watcher.version(&repo);
        wait_for_bump(&mut d.watcher, &repo, before);

        let cached = d.lru.len();
        assert_eq!(d.handle(&req).unwrap(), "other");
        assert!(d.lru.len() > cached, "watcher version change must bust the cache key");
    }

    #[test]
    fn missing_explicit_config_falls_back_to_mtime() {
        let dir = tempfile::TempDir::new().unwrap();
        let cwd = work_dir(&dir);
        let cfg = char_config(&dir, ">");
        let mut d = DaemonState::new(cfg.clone()).unwrap();
        let bogus = dir.path().join("bogus.toml");
        let req = frame(&cwd.to_string_lossy(), 0, "", 120, Some(&bogus.to_string_lossy()), false);
        assert_eq!(d.handle(&req).unwrap(), ">");
        assert_eq!(d.config_path, cfg, "unloadable explicit config must keep the current one");
    }
}
