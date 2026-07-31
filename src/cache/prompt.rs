use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use lru::LruCache;
use starship::configs::PROMPT_ORDER;
use starship::formatter::StringFormatter;
use starship::formatter::VariableHolder;
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
    pub segments: ModuleCache,
    pub time_bucket: u64,
}

struct RepoCache {
    git_dir: PathBuf,
    index_mtime: u64,
    ctx: Option<starship::context::Context<'static>>,
}

static REPO_CACHE: Mutex<Option<RepoCache>> = Mutex::new(None);
static BUST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn get_or_create_ctx(
    git_dir: Option<&Path>,
    current_dir: &Path,
    logical_dir: &Path,
) -> starship::context::Context<'static> {
    if let Some(gd) = git_dir {
        let index_mtime = get_mtime_ns(&gd.join("index"));
        let mut cache = REPO_CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
    *REPO_CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

fn make_bust_dir(git_dir: &Path) -> PathBuf {
    let bust = git_dir.join("bust").join(format!("{}", BUST_COUNTER.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::create_dir_all(&bust);
    bust
}

pub fn render_prompt(ctx: &RenderContext, git_dir: Option<&Path>) -> String {
    let (current_dir, bust_dir) = match git_dir.map(Path::to_path_buf).or_else(|| crate::find_git_dir(&ctx.cwd)) {
        Some(ref gd) => {
            let bust = make_bust_dir(gd);
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
    result.trim_end_matches('\n').to_string()
}

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

    let replacement = if expanded.is_empty() {
        String::new()
    } else {
        expanded.iter().map(|m| format!("${}", m)).collect::<String>()
    };

    format_str.replace("${all}", &replacement).replace("$all", &replacement)
}

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
        if module == "all" || module == "time" || context.is_module_disabled_in_config(&module) || cache.contains_key(&module) {
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

fn resolve_format(sctx: &starship::context::Context) -> String {
    if sctx.root_config.format.contains('$') {
        expand_all(sctx)
    } else {
        sctx.root_config.format.clone()
    }
}

fn save_repo_cache(gd: &Path, sctx: starship::context::Context<'static>) {
    let index_mtime = get_mtime_ns(&gd.join("index"));
    let mut rc = REPO_CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *rc = Some(RepoCache { git_dir: gd.to_path_buf(), index_mtime, ctx: Some(sctx) });
}

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

pub fn render_cached(
    ctx: &RenderContext,
    git_dir: Option<&Path>,
    config: &toml::Table,
    full_key: &CacheKey,
    lru: &mut LruCache<CacheKey, CachedValue>,
) -> String {
    let tb = crate::cache::current_minute();
    let resolved_gd = git_dir.map(Path::to_path_buf);

    // Path 1: Full hit — time_bucket matches
    if let Some(entry) = lru.get(&full_key).filter(|e| e.time_bucket == tb) {
        return entry.rendered.clone();
    }

    // Path 2: Time-only re-render — key exists, stale time_bucket
    // No bust_dir, no populate_cache, just re-use cached segments
    if let Some((key, mut entry)) = lru.pop_entry(&full_key) {

        let (sctx, fmt) = prepare_and_resolve(resolved_gd.as_deref(), &ctx.cwd, ctx, config);
        let r = get_prompt_with_cache(&sctx, &entry.segments, &fmt);
        let rendered = r.trim_end_matches('\n').to_string();
        entry.rendered = rendered.clone();
        entry.time_bucket = tb;
        lru.put(key, entry);

        if let Some(ref gd) = resolved_gd {
            save_repo_cache(gd, sctx);
        }

        return rendered;
    }

    // Path 3: Full miss — bust_dir + populate_cache

    let (current_dir, bust_dir) = match resolved_gd {
        Some(ref gd) => {
            let bust = make_bust_dir(gd);
            (bust.clone(), Some(bust))
        }
        None => (ctx.cwd.clone(), None),
    };

    let (sctx, fmt) = prepare_and_resolve(resolved_gd.as_deref(), &current_dir, ctx, config);
    let mut module_cache = ModuleCache::new();
    populate_cache(&sctx, &fmt, &mut module_cache);
    let rendered = get_prompt_with_cache(&sctx, &module_cache, &fmt);
    let rendered = rendered.trim_end_matches('\n').to_string();
    lru.put(full_key.clone(), CachedValue {
        rendered: rendered.clone(),
        segments: module_cache,
        time_bucket: tb,
    });

    if let Some(ref gd) = resolved_gd {
        save_repo_cache(gd, sctx);
    }

    if let Some(dir) = bust_dir { let _ = std::fs::remove_dir_all(dir); }
    rendered
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use super::*;
    use crate::cache::compute_cache_key;

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

    #[test]
    fn expand_all_no_all_in_format() {
        let cwd = tempfile::TempDir::new().unwrap();
        let ctx = test_ctx(cwd.path());
        let fmt = expand_all(&ctx);
        assert_eq!(fmt, "$character");
    }

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
    }

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
        let key = compute_cache_key(cwd.path(), 0, "vi", 120, 0, 0);

        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        let expected = render_prompt_with_config(&ctx, None, &cfg);
        let got = render_cached(&ctx, None, &cfg, &key, &mut lru);
        assert_eq!(got, expected);

        // Second call: should be a full hit (same time_bucket + format_str)
        let got2 = render_cached(&ctx, None, &cfg, &key, &mut lru);
        assert_eq!(got2, expected, "second call (full hit) should match");

        // Verify time-only re-render on stale time_bucket
        let (_, mut evicted) = lru.pop_entry(&key).unwrap();
        evicted.time_bucket = 0; // simulate minute tick
        lru.put(key.clone(), evicted);
        let got3 = render_cached(&ctx, None, &cfg, &key, &mut lru);
        assert_eq!(got3, expected, "time-only re-render should match full render");
    }

    #[test]
    fn render_cached_time_isolation() {
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
        let key = compute_cache_key(cwd.path(), 0, "vi", 120, 0, 0);

        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        let full = render_cached(&ctx, None, &cfg, &key, &mut lru);

        // Simulate a minute tick: stale the time_bucket
        let (_, mut entry) = lru.pop_entry(&key).unwrap();
        entry.time_bucket = 0;
        lru.put(key.clone(), entry);

        let after_tick = render_cached(&ctx, None, &cfg, &key, &mut lru);
        assert_eq!(after_tick, full, "time-only re-render must match full render");
    }

    #[test]
    fn render_cached_cache_omits_time_segments() {
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
        let key = compute_cache_key(cwd.path(), 0, "vi", 120, 0, 0);

        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        let _ = render_cached(&ctx, None, &cfg, &key, &mut lru);

        let (_, entry) = lru.pop_entry(&key).unwrap();
        assert!(!entry.segments.contains_key("time"),
            "populate_cache must skip 'time' module, but time segments found in cache");
        assert!(entry.segments.contains_key("character"),
            "populate_cache must include non-time modules, but character missing from cache");
    }

    #[test]
    fn expand_all_with_empty_all_expands_to_nothing() {
        let cwd = tempfile::TempDir::new().unwrap();
        let mut ctx = test_ctx(cwd.path());
        ctx = ctx.set_config(toml::toml! {
            format = "$all$time"
            add_newline = false
            [time]
            disabled = true
            [character]
            disabled = true
        });

        let fmt = expand_all(&ctx);
        assert!(!fmt.contains("$all"));
    }

    #[test]
    fn time_only_re_render_is_fast_no_bust_dir() {
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
        let key = compute_cache_key(cwd.path(), 0, "vi", 120, 0, 0);

        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        let _full = render_cached(&ctx, None, &cfg, &key, &mut lru);

        // Stale time_bucket to force time-only path
        let (_, mut entry) = lru.pop_entry(&key).unwrap();
        entry.time_bucket = 0;
        lru.put(key.clone(), entry);

        // Time-only re-render must be fast (<50ms on any hardware)
        let start = std::time::Instant::now();
        let result = render_cached(&ctx, None, &cfg, &key, &mut lru);
        let elapsed = start.elapsed();
        assert!(elapsed.as_millis() < 50, "time-only re-render took {}ms", elapsed.as_millis());
        assert!(!result.is_empty(), "result must not be empty");

        // Key still exists and has current time_bucket
        let cached = lru.get(&key).unwrap();
        let tb = crate::cache::current_minute();
        assert_eq!(cached.time_bucket, tb, "time_bucket must be updated");
    }

    #[test]
    fn stale_bucket_preserves_existing_segments() {
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
        let key = compute_cache_key(cwd.path(), 0, "vi", 120, 0, 0);

        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        let _ = render_cached(&ctx, None, &cfg, &key, &mut lru);

        // Snapshot segments after full render (Path 3)
        let (_, before) = lru.pop_entry(&key).unwrap();
        let segment_keys: Vec<String> = before.segments.keys().cloned().collect();
        assert!(segment_keys.contains(&"character".to_string()));
        assert!(segment_keys.contains(&"directory".to_string()));
        assert!(!segment_keys.contains(&"time".to_string()));
        // Put back with stale time_bucket
        let mut entry = before;
        entry.time_bucket = 0;
        lru.put(key.clone(), entry);

        // Stale bucket re-render
        let _ = render_cached(&ctx, None, &cfg, &key, &mut lru);

        let (_, after) = lru.pop_entry(&key).unwrap();
        for mod_name in &segment_keys {
            assert!(after.segments.contains_key(mod_name.as_str()),
                "module {mod_name} missing after stale-bucket re-render");
        }
        assert!(!after.segments.contains_key("time"),
            "time must not appear in cached segments after stale-bucket path");
        assert_eq!(after.time_bucket, crate::cache::current_minute(),
            "time_bucket must be updated after stale-bucket re-render");
    }

    #[test]
    fn stale_bucket_time_not_cached() {
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
        let key = compute_cache_key(cwd.path(), 0, "vi", 120, 0, 0);

        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        let result = render_cached(&ctx, None, &cfg, &key, &mut lru);

        // Output should contain a time (digits with colon) — proves time was computed
        assert!(result.contains(':'), "output must contain time (HH:MM)");

        // Stale time_bucket, re-render
        let (_, mut entry) = lru.pop_entry(&key).unwrap();
        entry.time_bucket = 0;
        lru.put(key.clone(), entry);

        let result2 = render_cached(&ctx, None, &cfg, &key, &mut lru);
        assert!(result2.contains(':'), "re-render output must contain time");

        // Time segments not in cache — proves computed on-the-fly by get_prompt_with_cache
        let (_, cached) = lru.pop_entry(&key).unwrap();
        assert!(!cached.segments.contains_key("time"),
            "time must not be stored in cached segments after stale-bucket path");
        assert!(cached.segments.contains_key("character"),
            "character must survive in cached segments");
        assert_eq!(cached.time_bucket, crate::cache::current_minute(),
            "time_bucket must be updated after stale-bucket re-render");
    }

    #[test]
    fn multiple_stale_bucket_rereads_preserve_cache() {
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
        let key = compute_cache_key(cwd.path(), 0, "vi", 120, 0, 0);

        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        let _ = render_cached(&ctx, None, &cfg, &key, &mut lru);

        // Three consecutive stale-bucket re-renders
        for i in 0..3 {
            let (_, mut entry) = lru.pop_entry(&key).unwrap();
            entry.time_bucket = 0;
            lru.put(key.clone(), entry);
            let r = render_cached(&ctx, None, &cfg, &key, &mut lru);
            assert!(r.contains(':'), "re-render {i} output must contain time");

            // Verify cache state after each iteration
            let (_, check) = lru.pop_entry(&key).unwrap();
            assert!(check.segments.contains_key("character"),
                "character must survive after re-render {i}");
            assert!(!check.segments.contains_key("time"),
                "time must not appear after re-render {i}");
            assert_eq!(check.time_bucket, crate::cache::current_minute(),
                "time_bucket must be updated after re-render {i}");
            lru.put(key.clone(), check);
        }

        // Final state: segments intact, time absent from cache
        let (_, cached) = lru.pop_entry(&key).unwrap();
        assert!(cached.segments.contains_key("character"),
            "character must survive 3 stale-bucket re-renders");
        assert!(!cached.segments.contains_key("time"),
            "time must not appear after 3 stale-bucket re-renders");
    }

}
