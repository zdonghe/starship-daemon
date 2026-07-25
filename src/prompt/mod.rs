use std::path::{Path, PathBuf};

/// Minimal config wrapper — just tracks path for reload detection.
#[derive(Debug)]
pub struct ModuleConfig {
    pub config_path: PathBuf,
}

/// Per-request context
pub struct RenderContext {
    pub cwd: PathBuf,
    pub terminal_width: usize,
    pub status_code: i32,
    pub keymap: String,
}

/// Cache key with mtime-based invalidation.
/// Checks cwd mtime (file create/delete), .git/index mtime (git add/reset),
/// and .git/HEAD mtime (branch switch/commit) to detect stale cache.
#[derive(Hash, Eq, PartialEq, Clone)]
pub struct CacheKey {
    pub cwd: PathBuf,
    pub status_code: i32,
    pub keymap: String,
    pub terminal_width: usize,
    pub time_bucket: u64,
    pub cwd_mtime: u64,
    pub index_mtime: u64,
    pub head_mtime: u64,
    pub branch_mtime: u64,
    pub remote_mtime: u64,
    pub config_mtime: u64,
}

pub fn get_mtime_ns(p: &std::path::Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn get_branch_ref_mtimes(git_dir: &std::path::Path) -> (u64, u64) {
    let head = git_dir.join("HEAD");
    let content = std::fs::read_to_string(&head).ok();
    let branch = content
        .and_then(|s| s.strip_prefix("ref: refs/heads/").map(|s| s.trim().to_string()));
    let branch_mtime = branch.as_ref()
        .map(|b| get_mtime_ns(&git_dir.join("refs").join("heads").join(b)))
        .unwrap_or(0);
    let remote_mtime = branch.as_ref()
        .map(|b| get_mtime_ns(&git_dir.join("refs").join("remotes").join("origin").join(b)))
        .unwrap_or(0);
    (branch_mtime, remote_mtime)
}

pub fn compute_cache_key(cwd: &Path, status_code: i32, keymap: &str, terminal_width: usize, time_bucket: u64, config_path: &Path) -> CacheKey {
    let git_dir = crate::find_git_dir(cwd);
    let (br_mtime, rr_mtime) = git_dir.as_ref().map(|d| get_branch_ref_mtimes(d)).unwrap_or((0, 0));
    CacheKey {
        cwd: cwd.to_path_buf(),
        status_code,
        keymap: keymap.to_string(),
        terminal_width,
        time_bucket,
        cwd_mtime: get_mtime_ns(cwd),
        index_mtime: git_dir.as_ref().map(|d| get_mtime_ns(&d.join("index"))).unwrap_or(0),
        head_mtime: git_dir.as_ref().map(|d| get_mtime_ns(&d.join("HEAD"))).unwrap_or(0),
        branch_mtime: br_mtime,
        remote_mtime: rr_mtime,
        config_mtime: get_mtime_ns(config_path),
    }
}

/// Validate that a config file exists.
pub fn load_config(path: &Path) -> Option<ModuleConfig> {
    if path.is_file() { Some(ModuleConfig { config_path: path.to_path_buf() }) } else { None }
}

/// Get default starship config path.
pub fn default_config_path() -> PathBuf {
    if let Ok(cfg) = std::env::var("STARSHIP_CONFIG") {
        let p = PathBuf::from(cfg);
        if p.exists() { return p; }
    }
    std::env::var("USERPROFILE")
        .map(|h| PathBuf::from(h).join(".config").join("starship.toml"))
        .unwrap_or_else(|_| PathBuf::from(".config/starship.toml"))
}

/// Render the full prompt using starship's native pipeline.
///
/// Starship caches git status per `current_dir` in a process-global static
/// (`get_static_repo_status` in git_status.rs). Every call uses a unique
/// subdirectory as `current_dir` and the real cwd as `logical_dir`. The
/// cache key is `current_dir`, so unique paths guarantee a cache miss and
/// fresh git status on every prompt.
pub fn render_prompt(ctx: &RenderContext) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static BUST_COUNTER: AtomicU64 = AtomicU64::new(0);

    let (current_dir, cleanup) = match crate::find_git_dir(&ctx.cwd) {
        Some(git_dir) => {
            let bust = git_dir.join("bust").join(format!("{}", BUST_COUNTER.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::create_dir_all(&bust);
            (bust, Some(git_dir))
        }
        None => (ctx.cwd.clone(), None),
    };

    let mut properties = starship::context::Properties::default();
    properties.status_code = Some(ctx.status_code.to_string());
    properties.keymap = ctx.keymap.clone();

    let env = starship::context::Env::default();
    let mut sctx = starship::context::Context::new_with_shell_and_path(
        properties, starship::context::Shell::Pwsh, starship::context::Target::Main,
        current_dir, ctx.cwd.clone(), env,
    );
    sctx.width = ctx.terminal_width;

    let result = starship::print::get_prompt(&sctx);
    if let Some(git_dir) = cleanup {
        let _ = std::fs::remove_dir_all(git_dir.join("bust"));
    }
    result.trim_end_matches('\n').to_string()
}
