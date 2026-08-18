#[cfg(feature = "fork")]
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use lru::LruCache;
#[cfg(feature = "fork")]
use starship::configs::PROMPT_ORDER;
#[cfg(feature = "fork")]
use starship::formatter::StringFormatter;
#[cfg(feature = "fork")]
use starship::formatter::VariableHolder;
#[cfg(feature = "fork")]
use starship::print::{get_prompt_with_cache, ModuleCache};

use crate::cache::{get_mtime_ns, CacheKey};

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
    REPO_CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn get_or_create_ctx(
    git_dir: Option<&Path>,
    current_dir: &Path,
    logical_dir: &Path,
) -> starship::context::Context<'static> {
    if let Some(gd) = git_dir {
        let index_mtime = get_mtime_ns(&gd.join("index"));
        let mut cache = lock_repo_cache();
        if let Some(ref mut cached) = *cache {
            if cached.git_dir == gd && cached.index_mtime == index_mtime {
                if let Some(sctx) = cached.ctx.take() {
                    return sctx;
                }
            }
        }
    }
    let properties = starship::context::Properties::default();
    let env = starship::context::Env::default();
    let sctx = starship::context::Context::new_with_shell_and_path(
        properties, starship::context::Shell::Pwsh, starship::context::Target::Main,
        current_dir.to_path_buf(), logical_dir.to_path_buf(), env,
    );
    sctx
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
    if version == 0 { BustDir::Fresh } else { BustDir::Reuse { version, config_mtime } }
}

fn fresh_bust_version() -> u64 {
    BUST_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn make_bust_dir(git_dir: &Path, kind: BustDir) -> PathBuf {
    let bust = match kind {
        BustDir::Reuse { version, config_mtime } => git_dir.join("bust").join(version.to_string()).join(config_mtime.to_string()),
        BustDir::Fresh => git_dir.join("bust").join("disable").join(fresh_bust_version().to_string()),
    };
    let _ = std::fs::create_dir_all(&bust);
    bust
}

pub fn render_prompt_with_config(ctx: &RenderContext, git_dir: Option<&Path>, config: &toml::Table, bust: BustDir) -> String {
    let (current_dir, bust_dir, resolved_git_dir) = match git_dir.map(Path::to_path_buf).or_else(|| crate::find_git_dir(&ctx.cwd)) {
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

#[cfg(feature = "fork")]
fn expand_all(context: &starship::context::Context) -> String {
    let format_str = &context.root_config.format;

    if !format_str.contains("$all") {
        return format_str.clone();
    }

    let right_str = &context.root_config.right_format;
    let left_vars = StringFormatter::new(format_str).ok()
        .map_or(BTreeSet::new(), |f| f.get_variables());
    let right_vars = StringFormatter::new(right_str).ok()
        .map_or(BTreeSet::new(), |f| f.get_variables());

    let explicit: BTreeSet<&str> = left_vars.union(&right_vars).map(|s| s.as_str()).collect();

    let expanded: Vec<&str> = PROMPT_ORDER.iter().copied()
        .filter(|m| !explicit.contains(m) && !context.is_module_disabled_in_config(m))
        .collect();

    let replacement: String = expanded.iter().map(|m| format!("${}", m)).collect();

    format_str.replace("${all}", &replacement).replace("$all", &replacement)
}

#[cfg(feature = "fork")]
fn populate_cache(
    context: &starship::context::Context,
    format_str: &str,
    cache: &mut ModuleCache,
) {
    let formatter = match StringFormatter::new(format_str) {
        Ok(f) => f,
        Err(_) => return,
    };

    for module in formatter.get_variables() {
        if module == "all" || module == "time" || module == "status" || module == "character"
            || context.is_module_disabled_in_config(&module) || cache.contains_key(&module) {
            continue;
        }
        if let Some(segments) = starship::print::get_module_segments(&module, context) {
            cache.insert(module, segments);
        }
    }
}

fn prepare_ctx(
    git_dir: Option<&Path>,
    current_dir: &Path,
    ctx: &RenderContext,
    config: &toml::Table,
) -> starship::context::Context<'static> {
    let mut sctx = get_or_create_ctx(
        git_dir, current_dir, &ctx.cwd,
    );
    sctx.current_dir = current_dir.to_path_buf();
    sctx.logical_dir = ctx.cwd.clone();
    sctx.width = ctx.terminal_width;
    sctx.properties.status_code = Some(ctx.status_code.to_string());
    sctx.properties.keymap = ctx.keymap.clone();
    sctx.set_config(config.clone())
}

#[cfg(feature = "fork")]
fn resolve_format(sctx: &starship::context::Context) -> String {
    if sctx.root_config.format.contains('$') {
        expand_all(sctx)
    } else {
        sctx.root_config.format.clone()
    }
}

fn save_repo_cache(gd: &Path, sctx: starship::context::Context<'static>) {
    let index_mtime = get_mtime_ns(&gd.join("index"));
    let mut rc = lock_repo_cache();
    *rc = Some(RepoCache { git_dir: gd.to_path_buf(), index_mtime, ctx: Some(sctx) });
}

#[cfg(feature = "fork")]
fn prepare_and_resolve(
    resolved_gd: Option<&Path>,
    current_dir: &Path,
    ctx: &RenderContext,
    config: &toml::Table,
) -> (starship::context::Context<'static>, String) {
    let sctx = prepare_ctx(resolved_gd, current_dir, ctx, config);
    let fmt = resolve_format(&sctx);
    (sctx, fmt)
}

fn trim_prompt(s: &str) -> String {
    s.trim_end_matches('\n').to_string()
}

#[cfg(feature = "fork")]
pub fn render_cached(
    ctx: &RenderContext,
    git_dir: Option<&Path>,
    config: &toml::Table,
    full_key: &CacheKey,
    lru: &mut LruCache<CacheKey, CachedValue>,
) -> String {
    let tb = crate::cache::current_minute();
    let resolved_gd = git_dir.map(Path::to_path_buf);

    if let Some(entry) = lru.get(&full_key).filter(|e| e.time_bucket == tb && e.status_code == ctx.status_code) {
        return entry.rendered.clone();
    }

    let (key, current_dir, bust_dir, segments) = match lru.pop_entry(full_key) {
        Some((key, entry)) => (key, ctx.cwd.clone(), None, Some(entry.segments)),
        None => {
            let (current_dir, bust_dir) = match resolved_gd {
                Some(ref gd) => {
                    let bust = make_bust_dir(gd, bust_for_version(full_key.watcher_version, full_key.config_mtime));
                    (bust.clone(), Some(bust))
                }
                None => (ctx.cwd.clone(), None),
            };
            (full_key.clone(), current_dir, bust_dir, None)
        }
    };

    let (sctx, fmt) = prepare_and_resolve(resolved_gd.as_deref(), &current_dir, ctx, config);
    let segments = match segments {
        Some(seg) => seg,
        None => {
            let mut seg = ModuleCache::new();
            populate_cache(&sctx, &fmt, &mut seg);
            seg
        }
    };
    let rendered = trim_prompt(&get_prompt_with_cache(&sctx, &segments, &fmt));
    lru.put(key, CachedValue { rendered: rendered.clone(), segments, time_bucket: tb, status_code: ctx.status_code });

    if let Some(ref gd) = resolved_gd {
        save_repo_cache(gd, sctx);
    }

    if let Some(dir) = bust_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
    rendered
}

#[cfg(not(feature = "fork"))]
pub fn render_cached(
    ctx: &RenderContext,
    git_dir: Option<&Path>,
    config: &toml::Table,
    full_key: &CacheKey,
    lru: &mut LruCache<CacheKey, CachedValue>,
) -> String {
    let tb = crate::cache::current_minute();
    if let Some(entry) = lru.get(full_key).filter(|e| e.time_bucket == tb && e.status_code == ctx.status_code) {
        return entry.rendered.clone();
    }
    let rendered = render_prompt_with_config(ctx, git_dir, config, bust_for_version(full_key.watcher_version, full_key.config_mtime));
    lru.put(full_key.clone(), CachedValue {
        rendered: rendered.clone(),
        time_bucket: tb,
        status_code: ctx.status_code,
    });
    rendered
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "fork")]
    use std::num::NonZeroUsize;

    #[cfg(feature = "fork")]
    use super::*;
    #[cfg(feature = "fork")]
    use crate::cache::compute_cache_key;

    #[cfg(feature = "fork")]
    fn test_ctx(cwd: &Path) -> starship::context::Context<'static> {
        let mut p = starship::context::Properties::default();
        p.status_code = Some("0".to_string());
        p.keymap = "vi".to_string();
        let mut ctx = starship::context::Context::new_with_shell_and_path(
            p, starship::context::Shell::Pwsh, starship::context::Target::Main,
            cwd.to_path_buf(), cwd.to_path_buf(), starship::context::Env::default(),
        );
        ctx.width = 120;
        ctx = ctx.set_config(toml::toml! {
            format = "$character"
            add_newline = false
            [character]
            format = ">"
        });
        ctx
    }

    #[cfg(feature = "fork")]
    #[test]
    fn expand_all_no_all_in_format() {
        let cwd = tempfile::TempDir::new().unwrap();
        let ctx = test_ctx(cwd.path());
        let fmt = expand_all(&ctx);
        assert_eq!(fmt, "$character");
    }

    #[cfg(feature = "fork")]
    #[test]
    fn expand_all_replaces_all_with_explicit() {
        let mut p = starship::context::Properties::default();
        p.status_code = Some("0".to_string());
        p.keymap = "vi".to_string();
        let cwd = tempfile::TempDir::new().unwrap();
        let mut ctx = starship::context::Context::new_with_shell_and_path(
            p, starship::context::Shell::Pwsh, starship::context::Target::Main,
            cwd.path().to_path_buf(), cwd.path().to_path_buf(), starship::context::Env::default(),
        );
        ctx.width = 120;
        ctx = ctx.set_config(toml::toml! {
            format = "$all"
            add_newline = false
        });

        let fmt = expand_all(&ctx);
        assert!(fmt.starts_with('$'), "expanded format starts with $");
        assert!(fmt.contains("$character"), "expanded format includes character");
        assert!(fmt.len() > 10, "expanded format is longer than $all");
        assert!(!fmt.contains("$all"), "expanded format does not contain $all");
    }

    #[cfg(feature = "fork")]
    #[test]
    fn expand_all_skips_disabled_modules() {
        let cwd = tempfile::TempDir::new().unwrap();
        let mut ctx = test_ctx(cwd.path());
        ctx = ctx.set_config(toml::toml! {
            format = "$all"
            add_newline = false
            [character]
            disabled = true
        });

        let fmt = expand_all(&ctx);
        assert!(!fmt.contains("$character"), "disabled module excluded from expansion");
    }

    #[cfg(feature = "fork")]
    #[test]
    fn expand_all_with_right_format_exclusions() {
        let cwd = tempfile::TempDir::new().unwrap();
        let mut ctx = test_ctx(cwd.path());
        ctx = ctx.set_config(toml::toml! {
            format = "$all"
            right_format = "$time"
            add_newline = false
        });

        let fmt = expand_all(&ctx);
        assert!(!fmt.contains("$all"));
        assert!(!fmt.contains("$time"),
            "right_format modules must be excluded from $all expansion");
    }

    #[cfg(feature = "fork")]
    #[test]
    fn render_cached_matches_render_prompt_with_config() {
        let cwd = tempfile::TempDir::new().unwrap();
        let ctx = RenderContext {
            cwd: cwd.path().to_path_buf(),
            terminal_width: 120,
            status_code: 0,
            keymap: "vi".to_string(),
        };
        let cfg = toml::toml! {
            format = "$character"
            add_newline = false
            [character]
            format = ">"
        };
        let key = compute_cache_key(cwd.path(), "vi", 120, 0, 0);

        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        let expected = render_prompt_with_config(&ctx, None, &cfg, BustDir::Fresh);
        let got = render_cached(&ctx, None, &cfg, &key, &mut lru);
        assert_eq!(got, expected);

        let got2 = render_cached(&ctx, None, &cfg, &key, &mut lru);
        assert_eq!(got2, expected, "second call (full hit) should match");

        let (_, mut evicted) = lru.pop_entry(&key).unwrap();
        evicted.time_bucket = 0;
        lru.put(key.clone(), evicted);
        let got3 = render_cached(&ctx, None, &cfg, &key, &mut lru);
        assert_eq!(got3, expected, "time-only re-render should match full render");
    }

    #[cfg(feature = "fork")]
    #[test]
    fn time_only_re_render_refreshes_bucket() {
        let cwd = tempfile::TempDir::new().unwrap();
        let ctx = RenderContext {
            cwd: cwd.path().to_path_buf(), terminal_width: 120, status_code: 0, keymap: "vi".to_string(),
        };
        let cfg = toml::toml! {
            format = "$character$time"
            add_newline = false
            [character]
            format = "> "
            [time]
            disabled = false
            format = "[$time](bold yellow)"
            time_format = "%H:%M"
        };
        let key = compute_cache_key(cwd.path(), "vi", 120, 0, 0);

        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        let _full = render_cached(&ctx, None, &cfg, &key, &mut lru);

        let (_, mut entry) = lru.pop_entry(&key).unwrap();
        entry.time_bucket = 0;
        lru.put(key.clone(), entry);

        let result = render_cached(&ctx, None, &cfg, &key, &mut lru);
        assert!(!result.is_empty(), "result must not be empty");

        let cached = lru.get(&key).unwrap();
        let tb = crate::cache::current_minute();
        assert_eq!(cached.time_bucket, tb, "time_bucket must be updated");
    }

    #[cfg(feature = "fork")]
    #[test]
    fn multiple_stale_bucket_rereads_preserve_cache() {
        let cwd = tempfile::TempDir::new().unwrap();
        let ctx = RenderContext {
            cwd: cwd.path().to_path_buf(), terminal_width: 120, status_code: 0, keymap: "vi".to_string(),
        };
        let cfg = toml::toml! {
            format = "$character$directory$time"
            add_newline = false
            [character]
            format = "> "
            [directory]
            disabled = false
            [time]
            disabled = false
            format = "[$time](bold yellow)"
            time_format = "%H:%M"
        };
        let key = compute_cache_key(cwd.path(), "vi", 120, 0, 0);

        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        let result = render_cached(&ctx, None, &cfg, &key, &mut lru);
        assert!(result.contains(':'), "output must contain time (HH:MM)");

        let (_, before) = lru.pop_entry(&key).unwrap();
        let segment_keys: Vec<String> = before.segments.keys().cloned().collect();
        assert!(segment_keys.contains(&"directory".to_string()));
        assert!(!segment_keys.contains(&"character".to_string()),
            "character must not appear in cached segments (re-rendered live)");
        assert!(!segment_keys.contains(&"time".to_string()));
        lru.put(key.clone(), before);

        for i in 0..3 {
            let (_, mut entry) = lru.pop_entry(&key).unwrap();
            entry.time_bucket = 0;
            lru.put(key.clone(), entry);
            let r = render_cached(&ctx, None, &cfg, &key, &mut lru);
            assert!(r.contains(':'), "re-render {i} output must contain time");

            let (_, check) = lru.pop_entry(&key).unwrap();
            for mod_name in &segment_keys {
                assert!(check.segments.contains_key(mod_name.as_str()),
                    "module {mod_name} missing after stale-bucket re-render {i}");
            }
            assert!(!check.segments.contains_key("time"),
                "time must not appear after re-render {i}");
            assert_eq!(check.time_bucket, crate::cache::current_minute(),
                "time_bucket must be updated after re-render {i}");
            lru.put(key.clone(), check);
        }
    }

    #[cfg(feature = "fork")]
    #[test]
    fn render_cached_status_code_isolation() {

        clear_repo_cache();
        let cwd = tempfile::TempDir::new().unwrap();

        let gd = tempfile::TempDir::new().unwrap();
        let cfg = toml::toml! {
            format = "$character$status$directory"
            add_newline = false
            [character]
            success_symbol = "C-OK"
            error_symbol = "C-ERR"
            [status]
            disabled = false
            format = "[$symbol$status]($style)"
            success_symbol = "OK"
            symbol = "ERR"
            [directory]
            disabled = false
        };

        let key = compute_cache_key(cwd.path(), "", 120, 0, 0);
        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());

        let ctx0 = RenderContext {
            cwd: cwd.path().to_path_buf(), terminal_width: 120, status_code: 0, keymap: "".to_string(),
        };
        let ctx1 = RenderContext {
            cwd: cwd.path().to_path_buf(), terminal_width: 120, status_code: 1, keymap: "".to_string(),
        };

        let r0 = render_cached(&ctx0, Some(gd.path()), &cfg, &key, &mut lru);
        assert!(r0.contains("C-OK"), "status 0 character must use success symbol, got: {r0}");
        assert!(r0.contains("OK0"), "status 0 module must show OK + 0, got: {r0}");
        let (_, e0) = lru.pop_entry(&key).unwrap();
        assert_eq!(e0.status_code, 0);
        assert!(e0.segments.contains_key("directory"),
            "stable module must be cached on full render");
        assert!(!e0.segments.contains_key("character"));
        assert!(!e0.segments.contains_key("status"),
            "status module must be excluded from cached segments");
        lru.put(key.clone(), e0);

        let r1 = render_cached(&ctx1, Some(gd.path()), &cfg, &key, &mut lru);
        assert!(r1.contains("C-ERR"), "status 1 character must use error symbol, got: {r1}");
        assert!(r1.contains("ERR1"), "status 1 module must show ERR + 1, got: {r1}");
        let (_, e1) = lru.pop_entry(&key).unwrap();
        assert_eq!(e1.status_code, 1, "cached status must be refreshed");
        assert!(e1.segments.contains_key("directory"),
            "directory segment must survive the exit-code change");
        assert!(!e1.segments.contains_key("character"));
        assert!(!e1.segments.contains_key("status"));
        lru.put(key.clone(), e1);

        let r2 = render_cached(&ctx1, Some(gd.path()), &cfg, &key, &mut lru);
        assert_eq!(r1, r2, "same status must hit Path 1 verbatim");

        let r3 = render_cached(&ctx0, Some(gd.path()), &cfg, &key, &mut lru);
        assert!(r3.contains("C-OK") && !r3.contains("C-ERR"),
            "status 1->0 must re-trigger a status-aware render (status check must fail), got: {r3}");

        let key2 = compute_cache_key(cwd.path(), "emacs", 120, 0, 0);
        let r4 = render_cached(&ctx0, Some(gd.path()), &cfg, &key2, &mut lru);
        assert!(r4.contains("C-OK"), "fresh-key render must work, got: {r4}");
    }

} // mod tests

#[cfg(all(test, not(feature = "fork")))]
mod stock_cache_tests {
    use std::num::NonZeroUsize;

    use lru::LruCache;

    use super::*;
    use crate::cache::compute_cache_key;

    fn cfg() -> toml::Table {
        toml::toml! {
            format = "$character"
            add_newline = false
            [character]
            success_symbol = "C-OK"
            error_symbol = "C-ERR"
        }
    }

    fn ctx(cwd: &Path, status: i32) -> RenderContext {
        RenderContext {
            cwd: cwd.to_path_buf(), terminal_width: 120, status_code: status, keymap: "".to_string(),
        }
    }

    #[test]
    fn hit_serves_cached_rendered_without_rerender() {
        let cwd = tempfile::TempDir::new().unwrap();
        let key = compute_cache_key(cwd.path(), "", 120, 0, 0);
        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        lru.put(key.clone(), CachedValue {
            rendered: "SEEDED".to_string(),
            time_bucket: crate::cache::current_minute(),
            status_code: 0,
        });
        let r = render_cached(&ctx(cwd.path(), 0), None, &cfg(), &key, &mut lru);
        assert_eq!(r, "SEEDED", "same minute + status must serve the cache, not re-render");
    }

    #[test]
    fn status_change_misses_and_refreshes() {
        let cwd = tempfile::TempDir::new().unwrap();
        let key = compute_cache_key(cwd.path(), "", 120, 0, 0);
        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        lru.put(key.clone(), CachedValue {
            rendered: "SEEDED".to_string(),
            time_bucket: crate::cache::current_minute(),
            status_code: 0,
        });
        let r = render_cached(&ctx(cwd.path(), 1), None, &cfg(), &key, &mut lru);
        assert_ne!(r, "SEEDED", "status change must miss the cache");
        assert!(r.contains("C-ERR"), "status 1 must render the error symbol, got: {r}");
        let (_, e) = lru.pop_entry(&key).unwrap();
        assert_eq!(e.status_code, 1, "cached status must be refreshed");
    }

    #[test]
    fn minute_rollover_misses_and_refreshes_bucket() {
        let cwd = tempfile::TempDir::new().unwrap();
        let key = compute_cache_key(cwd.path(), "", 120, 0, 0);
        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        lru.put(key.clone(), CachedValue {
            rendered: "SEEDED".to_string(),
            time_bucket: crate::cache::current_minute() - 1,
            status_code: 0,
        });
        let r = render_cached(&ctx(cwd.path(), 0), None, &cfg(), &key, &mut lru);
        assert_ne!(r, "SEEDED", "stale time bucket must miss the cache");
        let (_, e) = lru.pop_entry(&key).unwrap();
        assert_eq!(e.time_bucket, crate::cache::current_minute(), "bucket must refresh");
    }
}

#[cfg(test)]
mod bust_dir_tests {
    use super::{BustDir, bust_for_version, make_bust_dir};

    #[test]
    fn make_bust_dir_is_version_keyed() {
        let gd = tempfile::TempDir::new().unwrap();
        let a1 = make_bust_dir(gd.path(), BustDir::Reuse { version: 3, config_mtime: 7 });
        let a2 = make_bust_dir(gd.path(), BustDir::Reuse { version: 3, config_mtime: 7 });
        assert_eq!(a1, a2, "same version+mtime must yield the same bust path");
        assert_eq!(a1, gd.path().join("bust").join("3").join("7"), "path must be <git_dir>/bust/<version>/<config_mtime>");
        let b = make_bust_dir(gd.path(), BustDir::Reuse { version: 4, config_mtime: 7 });
        assert_ne!(a1, b, "different version must yield a different bust path");
        let c = make_bust_dir(gd.path(), BustDir::Reuse { version: 3, config_mtime: 8 });
        assert_ne!(a1, c, "different config_mtime must yield a different bust path");
    }

    #[test]
    fn make_bust_dir_fresh_never_collides_with_reuse() {
        let gd = tempfile::TempDir::new().unwrap();
        let w = make_bust_dir(gd.path(), BustDir::Reuse { version: 3, config_mtime: 7 });
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
        assert_eq!(bust_for_version(3, 0), BustDir::Reuse { version: 3, config_mtime: 0 });
        assert_eq!(bust_for_version(3, 7), BustDir::Reuse { version: 3, config_mtime: 7 });
    }
}
