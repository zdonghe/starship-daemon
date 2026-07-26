use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct RenderContext {
    pub cwd: PathBuf,
    pub terminal_width: usize,
    pub status_code: i32,
    pub keymap: String,
}

#[derive(Debug, Hash, Eq, PartialEq, Clone)]
pub struct CacheKey {
    pub cwd: PathBuf,
    pub status_code: i32,
    pub keymap: String,
    pub terminal_width: usize,
    pub time_bucket: u64,
    pub cwd_mtime: u64,
    pub index_mtime: u64,
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

pub fn compute_cache_key(cwd: &Path, status_code: i32, keymap: &str, terminal_width: usize, time_bucket: u64, config_path: &Path, git_dir: Option<&Path>) -> CacheKey {
    let git_dir: Option<PathBuf> = git_dir.map(Path::to_path_buf).or_else(|| crate::find_git_dir(cwd));
    let (br_mtime, rr_mtime) = git_dir.as_ref().map(|d| get_branch_ref_mtimes(d)).unwrap_or((0, 0));
    CacheKey {
        cwd: cwd.to_path_buf(),
        status_code,
        keymap: keymap.to_string(),
        terminal_width,
        time_bucket,
        cwd_mtime: get_mtime_ns(cwd),
        index_mtime: git_dir.as_ref().map(|d| get_mtime_ns(&d.join("index"))).unwrap_or(0),
        branch_mtime: br_mtime,
        remote_mtime: rr_mtime,
        config_mtime: get_mtime_ns(config_path),
    }
}

/// Validate that a config file exists.
pub fn load_config(path: &Path) -> Option<PathBuf> {
    if path.is_file() { Some(path.to_path_buf()) } else { None }
}

pub fn default_config_path() -> PathBuf {
    if let Ok(cfg) = std::env::var("STARSHIP_CONFIG") {
        return PathBuf::from(cfg);
    }
    std::env::var("USERPROFILE")
        .map(|h| PathBuf::from(h).join(".config").join("starship.toml"))
        .unwrap_or_else(|_| PathBuf::from(".config/starship.toml"))
}

struct RepoCache {
    git_dir: PathBuf,
    index_mtime: u64,
    ctx: Option<starship::context::Context<'static>>,
}

static REPO_CACHE: Mutex<Option<RepoCache>> = Mutex::new(None);

fn get_or_create_ctx(
    git_dir: Option<&Path>,
    current_dir: &Path,
    logical_dir: &Path,
    status_code: i32,
    keymap: &str,
    terminal_width: usize,
    config: &toml::Table,
) -> starship::context::Context<'static> {
    if let Some(gd) = git_dir {
        let index_mtime = get_mtime_ns(&gd.join("index"));
        let mut cache = REPO_CACHE.lock().unwrap();
        if let Some(ref mut cached) = *cache {
            if cached.git_dir == gd && cached.index_mtime == index_mtime {
                if let Some(sctx) = cached.ctx.take() {
                    return sctx;
                }
            }
        }
    }
    let mut properties = starship::context::Properties::default();
    properties.status_code = Some(status_code.to_string());
    properties.keymap = keymap.to_string();
    let env = starship::context::Env::default();
    let mut sctx = starship::context::Context::new_with_shell_and_path(
        properties, starship::context::Shell::Pwsh, starship::context::Target::Main,
        current_dir.to_path_buf(), logical_dir.to_path_buf(), env,
    );
    sctx.width = terminal_width;
    sctx = sctx.set_config(config.clone());
    sctx
}

/// Render the full prompt using starship's native pipeline.
///
/// Starship caches git status per `current_dir` in a process-global static
/// (`get_static_repo_status` in git_status.rs). Every call uses a unique
/// subdirectory as `current_dir` and the real cwd as `logical_dir`. The
/// cache key is `current_dir`, so unique paths guarantee a cache miss and
/// fresh git status on every prompt.
pub fn render_prompt(ctx: &RenderContext, git_dir: Option<&Path>) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static BUST_COUNTER: AtomicU64 = AtomicU64::new(0);

    let (current_dir, bust_dir) = match git_dir.map(Path::to_path_buf).or_else(|| crate::find_git_dir(&ctx.cwd)) {
        Some(git_dir) => {
            let bust = git_dir.join("bust").join(format!("{}", BUST_COUNTER.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::create_dir_all(&bust);
            (bust.clone(), Some(bust))
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
    if let Some(dir) = bust_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
    result.trim_end_matches('\n').to_string()
}

/// Read and parse starship config TOML from disk.
pub fn read_config(path: &Path) -> toml::Table {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.parse::<toml::Table>().ok())
        .unwrap_or_default()
}

/// Like render_prompt but injects a pre-parsed config table via set_config
/// to ensure config changes are picked up immediately.
/// Note: Context::new still reads the file internally once; this override
/// ensures the latest cached config is what's used for rendering.
pub fn render_prompt_with_config(ctx: &RenderContext, git_dir: Option<&Path>, config: &toml::Table) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static BUST_COUNTER: AtomicU64 = AtomicU64::new(0);

    let (current_dir, bust_dir, resolved_git_dir) = match git_dir.map(Path::to_path_buf).or_else(|| crate::find_git_dir(&ctx.cwd)) {
        Some(git_dir) => {
            let bust = git_dir.join("bust").join(format!("{}", BUST_COUNTER.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::create_dir_all(&bust);
            (bust.clone(), Some(bust), Some(git_dir))
        }
        None => (ctx.cwd.clone(), None, None),
    };

    let mut sctx = get_or_create_ctx(
        resolved_git_dir.as_deref(),
        &current_dir,
        &ctx.cwd,
        ctx.status_code,
        &ctx.keymap,
        ctx.terminal_width,
        config,
    );

    sctx.current_dir = current_dir.clone();
    sctx.logical_dir = ctx.cwd.clone();
    sctx.width = ctx.terminal_width;
    sctx.properties.status_code = Some(ctx.status_code.to_string());
    sctx.properties.keymap = ctx.keymap.clone();
    sctx = sctx.set_config(config.clone());

    let result = starship::print::get_prompt(&sctx);

    if let Some(ref gd) = resolved_git_dir {
        let index_mtime = get_mtime_ns(&gd.join("index"));
        let mut cache = REPO_CACHE.lock().unwrap();
        *cache = Some(RepoCache {
            git_dir: gd.clone(),
            index_mtime,
            ctx: Some(sctx),
        });
    }

    if let Some(dir) = bust_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
    result.trim_end_matches('\n').to_string()
}

pub mod test_helpers {
    use super::*;

    pub fn repocache_is_empty() -> bool {
        REPO_CACHE.lock().unwrap().is_none()
    }

    pub fn repocache_clear() {
        *REPO_CACHE.lock().unwrap() = None;
    }

    /// Returns (git_dir, index_mtime) if cache populated.
    pub fn repocache_state() -> Option<(PathBuf, u64)> {
        let cache = REPO_CACHE.lock().unwrap();
        cache.as_ref().map(|c| (c.git_dir.clone(), c.index_mtime))
    }
}
