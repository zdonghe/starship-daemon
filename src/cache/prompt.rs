#[cfg(fork_starship)]
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use lru::LruCache;
#[cfg(fork_starship)]
use starship::configs::PROMPT_ORDER;
#[cfg(fork_starship)]
use starship::formatter::StringFormatter;
#[cfg(fork_starship)]
use starship::formatter::VariableHolder;
#[cfg(fork_starship)]
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
    #[cfg(fork_starship)]
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

fn make_bust_dir(git_dir: &Path) -> PathBuf {
    let bust = git_dir.join("bust").join(format!("{}", BUST_COUNTER.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::create_dir_all(&bust);
    bust
}

pub fn render_prompt_with_config(ctx: &RenderContext, git_dir: Option<&Path>, config: &toml::Table) -> String {
    let (current_dir, bust_dir, resolved_git_dir) = match git_dir.map(Path::to_path_buf).or_else(|| crate::find_git_dir(&ctx.cwd)) {
        Some(ref gd) => {
            let bust = make_bust_dir(gd);
            (bust.clone(), Some(bust), Some(gd.clone()))
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

#[cfg(fork_starship)]
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

#[cfg(fork_starship)]
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

#[cfg(fork_starship)]
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

#[cfg(fork_starship)]
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

#[cfg(fork_starship)]
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
                    let bust = make_bust_dir(gd);
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

#[cfg(not(fork_starship))]
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
    let rendered = render_prompt_with_config(ctx, git_dir, config);
    lru.put(full_key.clone(), CachedValue {
        rendered: rendered.clone(),
        time_bucket: tb,
        status_code: ctx.status_code,
    });
    rendered
}

#[cfg(test)]
mod tests {
    #[cfg(fork_starship)]
    use std::num::NonZeroUsize;

    #[cfg(fork_starship)]
    use super::*;
    #[cfg(fork_starship)]
    use crate::cache::compute_cache_key;

    #[cfg(fork_starship)]
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

    #[cfg(fork_starship)]
    #[test]
    fn expand_all_no_all_in_format() {
        let cwd = tempfile::TempDir::new().unwrap();
        let ctx = test_ctx(cwd.path());
        let fmt = expand_all(&ctx);
        assert_eq!(fmt, "$character");
    }

    #[cfg(fork_starship)]
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

    #[cfg(fork_starship)]
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

    #[cfg(fork_starship)]
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

    #[cfg(fork_starship)]
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
        let expected = render_prompt_with_config(&ctx, None, &cfg);
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

    #[cfg(fork_starship)]
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

    #[cfg(fork_starship)]
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

    #[cfg(fork_starship)]
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

        BUST_COUNTER.store(0, Ordering::Relaxed);
        let base = BUST_COUNTER.load(Ordering::Relaxed);

        let r0 = render_cached(&ctx0, Some(gd.path()), &cfg, &key, &mut lru);
        assert!(r0.contains("C-OK"), "status 0 character must use success symbol, got: {r0}");
        assert!(r0.contains("OK0"), "status 0 module must show OK + 0, got: {r0}");
        assert_eq!(BUST_COUNTER.load(Ordering::Relaxed), base + 1,
            "first full miss (Path 3) must make exactly one bust dir");
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
        assert_eq!(BUST_COUNTER.load(Ordering::Relaxed), base + 1,
            "exit-code change must reuse segments (Path 2), not make a bust dir");
        let (_, e1) = lru.pop_entry(&key).unwrap();
        assert_eq!(e1.status_code, 1, "cached status must be refreshed");
        assert!(e1.segments.contains_key("directory"),
            "directory segment must survive the exit-code change");
        assert!(!e1.segments.contains_key("character"));
        assert!(!e1.segments.contains_key("status"));
        lru.put(key.clone(), e1);

        let r2 = render_cached(&ctx1, Some(gd.path()), &cfg, &key, &mut lru);
        assert_eq!(r1, r2, "same status must hit Path 1 verbatim");
        assert_eq!(BUST_COUNTER.load(Ordering::Relaxed), base + 1,
            "same-status hit (Path 1) must not make a bust dir");

        let r3 = render_cached(&ctx0, Some(gd.path()), &cfg, &key, &mut lru);
        assert!(r3.contains("C-OK") && !r3.contains("C-ERR"),
            "status 1->0 must re-trigger a status-aware render (status check must fail), got: {r3}");
        assert_eq!(BUST_COUNTER.load(Ordering::Relaxed), base + 1,
            "1->0 transition must also be Path 2");

        let key2 = compute_cache_key(cwd.path(), "emacs", 120, 0, 0);
        let r4 = render_cached(&ctx0, Some(gd.path()), &cfg, &key2, &mut lru);
        assert_eq!(BUST_COUNTER.load(Ordering::Relaxed), base + 2,
            "positive control: a fresh key must still full-render (Path 3) and bust");
        assert!(r4.contains("C-OK"), "fresh-key render must work, got: {r4}");
    }

}

#[cfg(all(test, not(fork_starship)))]
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
