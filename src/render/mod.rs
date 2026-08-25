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
