use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use lru::LruCache;

#[cfg(feature = "fork")]
mod fork;
#[cfg(not(feature = "fork"))]
mod stock;

#[cfg(feature = "fork")]
use starship::print::ModuleCache;

#[cfg(feature = "fork")]
use self::fork::render_miss;
#[cfg(not(feature = "fork"))]
use self::stock::render_miss;

use crate::cache::{CacheKey, get_mtime_ns};

pub struct RenderContext {
    pub cwd: PathBuf,
    pub terminal_width: usize,
    pub status_code: i32,
    pub keymap: String,
}

pub struct CachedValue {
    pub rendered: String,
    #[cfg(feature = "fork")]
    pub segments: ModuleCache,
    pub time_bucket: u64,
    pub status_code: i32,
}

struct RepoCache {
    git_dir: PathBuf,
    index_mtime: u64,
    ctx: Option<starship::context::Context<'static>>,
}

static REPO_CACHE: Mutex<Option<RepoCache>> = Mutex::new(None);
static BUST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn lock_repo_cache() -> std::sync::MutexGuard<'static, Option<RepoCache>> {
    REPO_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn get_or_create_ctx(
    git_dir: Option<&Path>,
    current_dir: &Path,
    logical_dir: &Path,
) -> starship::context::Context<'static> {
    if let Some(gd) = git_dir {
        let index_mtime = get_mtime_ns(&gd.join("index"));
        let mut cache = lock_repo_cache();
        if let Some(ref mut cached) = *cache
            && cached.git_dir == gd
            && cached.index_mtime == index_mtime
            && let Some(sctx) = cached.ctx.take()
        {
            return sctx;
        }
    }
    let properties = starship::context::Properties::default();
    let env = starship::context::Env::default();
    starship::context::Context::new_with_shell_and_path(
        properties,
        starship::context::Shell::Pwsh,
        starship::context::Target::Main,
        current_dir.to_path_buf(),
        logical_dir.to_path_buf(),
        env,
    )
}

pub fn clear_repo_cache() {
    *lock_repo_cache() = None;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BustDir {
    Reuse { version: u64, config_mtime: u64 },
    Fresh,
}

fn bust_for_version(version: u64, config_mtime: u64) -> BustDir {
    if version == 0 {
        BustDir::Fresh
    } else {
        BustDir::Reuse {
            version,
            config_mtime,
        }
    }
}

fn fresh_bust_version() -> u64 {
    BUST_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn make_bust_dir(git_dir: &Path, kind: BustDir) -> PathBuf {
    let bust = match kind {
        BustDir::Reuse {
            version,
            config_mtime,
        } => git_dir
            .join("bust")
            .join(version.to_string())
            .join(config_mtime.to_string()),
        BustDir::Fresh => git_dir
            .join("bust")
            .join("disable")
            .join(fresh_bust_version().to_string()),
    };
    let _ = std::fs::create_dir_all(&bust);
    bust
}

pub fn render_prompt_with_config(
    ctx: &RenderContext,
    git_dir: Option<&Path>,
    config: &toml::Table,
    bust: BustDir,
) -> String {
    let (current_dir, bust_dir, resolved_git_dir) = match git_dir
        .map(Path::to_path_buf)
        .or_else(|| crate::find_git_dir(&ctx.cwd))
    {
        Some(ref gd) => {
            let b = make_bust_dir(gd, bust);
            (b.clone(), Some(b), Some(gd.clone()))
        }
        None => (ctx.cwd.clone(), None, None),
    };

    let sctx = prepare_ctx(resolved_git_dir.as_deref(), &current_dir, ctx, config);
    let result = starship::print::get_prompt(&sctx);
    if let Some(ref gd) = resolved_git_dir {
        save_repo_cache(gd, sctx);
    }

    if let Some(dir) = bust_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
    trim_prompt(&result)
}

/// Shared cache-hit skeleton. On a hit (same minute bucket + same exit code)
/// the stored rendering is served verbatim; otherwise the variant-specific
/// miss path re-renders and repopulates the entry.
pub fn render_cached(
    ctx: &RenderContext,
    git_dir: Option<&Path>,
    config: &toml::Table,
    full_key: &CacheKey,
    lru: &mut LruCache<CacheKey, CachedValue>,
) -> String {
    let tb = crate::cache::current_minute();
    if let Some(entry) = lru
        .get(full_key)
        .filter(|e| e.time_bucket == tb && e.status_code == ctx.status_code)
    {
        return entry.rendered.clone();
    }
    render_miss(ctx, git_dir, config, full_key, lru, tb)
}

fn prepare_ctx(
    git_dir: Option<&Path>,
    current_dir: &Path,
    ctx: &RenderContext,
    config: &toml::Table,
) -> starship::context::Context<'static> {
    let mut sctx = get_or_create_ctx(git_dir, current_dir, &ctx.cwd);
    sctx.current_dir = current_dir.to_path_buf();
    sctx.logical_dir = ctx.cwd.clone();
    sctx.width = ctx.terminal_width;
    sctx.properties.status_code = Some(ctx.status_code.to_string());
    sctx.properties.keymap = ctx.keymap.clone();
    sctx.set_config(config.clone())
}

fn save_repo_cache(gd: &Path, sctx: starship::context::Context<'static>) {
    let index_mtime = get_mtime_ns(&gd.join("index"));
    let mut rc = lock_repo_cache();
    *rc = Some(RepoCache {
        git_dir: gd.to_path_buf(),
        index_mtime,
        ctx: Some(sctx),
    });
}

fn trim_prompt(s: &str) -> String {
    s.trim_end_matches('\n').to_string()
}

#[cfg(test)]
mod bust_dir_tests {
    use super::{BustDir, bust_for_version, make_bust_dir};

    #[test]
    fn make_bust_dir_is_version_keyed() {
        let gd = tempfile::TempDir::new().unwrap();
        let a1 = make_bust_dir(
            gd.path(),
            BustDir::Reuse {
                version: 3,
                config_mtime: 7,
            },
        );
        let a2 = make_bust_dir(
            gd.path(),
            BustDir::Reuse {
                version: 3,
                config_mtime: 7,
            },
        );
        assert_eq!(a1, a2, "same version+mtime must yield the same bust path");
        assert_eq!(
            a1,
            gd.path().join("bust").join("3").join("7"),
            "path must be <git_dir>/bust/<version>/<config_mtime>"
        );
        let b = make_bust_dir(
            gd.path(),
            BustDir::Reuse {
                version: 4,
                config_mtime: 7,
            },
        );
        assert_ne!(a1, b, "different version must yield a different bust path");
        let c = make_bust_dir(
            gd.path(),
            BustDir::Reuse {
                version: 3,
                config_mtime: 8,
            },
        );
        assert_ne!(
            a1, c,
            "different config_mtime must yield a different bust path"
        );
    }

    #[test]
    fn make_bust_dir_fresh_never_collides_with_reuse() {
        let gd = tempfile::TempDir::new().unwrap();
        let w = make_bust_dir(
            gd.path(),
            BustDir::Reuse {
                version: 3,
                config_mtime: 7,
            },
        );
        for _ in 0..5 {
            let f = make_bust_dir(gd.path(), BustDir::Fresh);
            assert_ne!(f, w, "fresh bust must not equal a reuse bust path");
            assert!(
                f.starts_with(gd.path().join("bust").join("disable")),
                "fresh bust must live under bust/disable, got {f:?}"
            );
        }
    }

    #[test]
    fn bust_for_version_zero_degrades_to_fresh() {
        assert_eq!(bust_for_version(0, 7), BustDir::Fresh);
        assert_eq!(
            bust_for_version(3, 0),
            BustDir::Reuse {
                version: 3,
                config_mtime: 0
            }
        );
        assert_eq!(
            bust_for_version(3, 7),
            BustDir::Reuse {
                version: 3,
                config_mtime: 7
            }
        );
    }
}

/// Differential tests: the daemon's rendered prompt must be byte-identical to
/// what plain starship produces for the same inputs (cwd, exit code, width,
/// shell, config table). The reference is computed by building an equivalent
/// starship Context directly and calling `get_prompt`, bypassing every layer
/// of daemon machinery (bust dirs, caches, config plumbing).
#[cfg(test)]
mod fidelity_tests {
    use std::num::NonZeroUsize;
    use std::path::Path;

    use lru::LruCache;
    use toml::toml;

    use super::{
        BustDir, RenderContext, clear_repo_cache, render_cached, render_prompt_with_config,
        trim_prompt,
    };
    use crate::cache::{compute_cache_key, current_minute};

    const WIDTH: usize = 120;
    const KEYMAP: &str = "vi";

    pub(crate) fn default_cfg() -> toml::Table {
        toml! { add_newline = false }
    }

    fn rctx(cwd: &Path, status_code: i32) -> RenderContext {
        RenderContext {
            cwd: cwd.to_path_buf(),
            terminal_width: WIDTH,
            status_code,
            keymap: KEYMAP.to_string(),
        }
    }

    /// What plain starship would print standing in `cwd`: a Context built
    /// exactly like prepare_ctx does, but with the real cwd as both
    /// current_dir and logical_dir (no bust-directory indirection).
    fn ref_ctx(cwd: &Path, status_code: i32) -> starship::context::Context<'static> {
        let mut p = starship::context::Properties::default();
        p.status_code = Some(status_code.to_string());
        p.keymap = KEYMAP.to_string();
        let mut ctx = starship::context::Context::new_with_shell_and_path(
            p,
            starship::context::Shell::Pwsh,
            starship::context::Target::Main,
            cwd.to_path_buf(),
            cwd.to_path_buf(),
            starship::context::Env::default(),
        );
        ctx.width = WIDTH;
        ctx.set_config(default_cfg())
    }

    pub(crate) fn reference(cwd: &Path, status_code: i32) -> String {
        trim_prompt(&starship::print::get_prompt(&ref_ctx(cwd, status_code)))
    }

    fn daemon_fresh(cwd: &Path, status_code: i32) -> String {
        clear_repo_cache();
        render_prompt_with_config(
            &rctx(cwd, status_code),
            None,
            &default_cfg(),
            BustDir::Fresh,
        )
    }

    fn assert_matches_reference(cwd: &Path, status_code: i32) {
        let want = reference(cwd, status_code);
        let got = daemon_fresh(cwd, status_code);
        assert_eq!(got, want, "daemon output diverged from starship reference");
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            // Hermetic against the parent shell: ignore ambient git env and
            // any global signing/hook configuration.
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CEILING_DIRECTORIES")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git must be available");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    pub(crate) fn repo_with_commit() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("tracked.txt"), "a\n").unwrap();
        git(dir.path(), &["init"]);
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "init"]);
        dir
    }

    #[test]
    fn plain_directory_matches_starship_reference() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_matches_reference(dir.path(), 0);
    }

    #[test]
    fn clean_git_repo_matches_starship_reference() {
        let dir = repo_with_commit();
        assert_matches_reference(dir.path(), 0);
    }

    #[test]
    fn dirty_git_repo_matches_starship_reference() {
        let dir = repo_with_commit();
        std::fs::write(dir.path().join("tracked.txt"), "b\n").unwrap();
        assert_matches_reference(dir.path(), 0);
    }

    #[test]
    fn repo_subdirectory_matches_starship_reference() {
        let dir = repo_with_commit();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        assert_matches_reference(&sub, 0);
    }

    #[test]
    fn nonzero_status_matches_starship_reference() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_matches_reference(dir.path(), 1);
    }

    #[test]
    fn warm_cache_hit_matches_starship_reference() {
        let dir = repo_with_commit();
        std::fs::write(dir.path().join("tracked.txt"), "b\n").unwrap();
        clear_repo_cache();

        let ctx = rctx(dir.path(), 0);
        let cfg = default_cfg();
        let key = compute_cache_key(dir.path(), KEYMAP, WIDTH, 0, 0);
        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());

        let first = render_cached(&ctx, None, &cfg, &key, &mut lru);
        let tb = current_minute();
        let second = render_cached(&ctx, None, &cfg, &key, &mut lru);
        let want = reference(dir.path(), 0);
        assert_eq!(first, want, "miss-path output must equal the reference");
        assert_eq!(second, want, "cache-hit output must equal the reference");
        let entry = lru.get(&key).expect("entry must exist after render");
        assert_eq!(
            entry.time_bucket, tb,
            "time bucket untouched by second call - proves the hit branch ran"
        );
    }
}
