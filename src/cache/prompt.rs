use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use lru::LruCache;
use starship::configs::PROMPT_ORDER;
use starship::formatter::{StringFormatter, VariableHolder};
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

pub fn clear_repo_cache() {
    *REPO_CACHE.lock().unwrap() = None;
}

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

fn expand_all(context: &starship::context::Context) -> String {
    let format_str = &context.root_config.format;

    if !format_str.contains("$all") && !format_str.contains("${all}") {
        return format_str.clone();
    }

    let right_str = &context.root_config.right_format;
    let left_vars = StringFormatter::new(format_str).ok()
        .map(|f| f.get_variables()).unwrap_or_default();
    let right_vars = StringFormatter::new(right_str).ok()
        .map(|f| f.get_variables()).unwrap_or_default();

    let explicit: BTreeSet<_> = left_vars.union(&right_vars).cloned().collect();

    let expanded: Vec<&str> = PROMPT_ORDER.iter()
        .filter(|m| !explicit.iter().any(|e| e == *m))
        .filter(|m| !context.is_module_disabled_in_config(m))
        .copied()
        .collect();

    let replacement = if expanded.is_empty() {
        String::new()
    } else {
        expanded.iter().map(|m| format!("${}", m)).collect::<Vec<_>>().join("")
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
        if module == "all" || module == "time" {
            continue;
        }
        if context.is_module_disabled_in_config(&module) {
            continue;
        }
        if cache.contains_key(&module) {
            continue;
        }
        if let Some(segments) = starship::print::get_module_segments(&module, context) {
            cache.insert(module, segments);
        }
    }
}

pub fn render_cached(
    ctx: &RenderContext,
    git_dir: Option<&Path>,
    config: &toml::Table,
    full_key: &CacheKey,
    lru: &mut LruCache<CacheKey, CachedValue>,
) -> String {
    let tb = crate::cache::current_minute();

    // Fast path: full hit before any expensive work
    if lru.peek(&full_key).is_some_and(|e| e.time_bucket == tb) {
        return lru.get(&full_key).unwrap().rendered.clone();
    }

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
        &current_dir, &ctx.cwd,
        ctx.status_code, &ctx.keymap, ctx.terminal_width, config,
    );

    sctx.current_dir = current_dir.clone();
    sctx.logical_dir = ctx.cwd.clone();
    sctx.width = ctx.terminal_width;
    sctx.properties.status_code = Some(ctx.status_code.to_string());
    sctx.properties.keymap = ctx.keymap.clone();
    sctx = sctx.set_config(config.clone());

    let fmt = if sctx.root_config.format.contains('$') {
        expand_all(&sctx)
    } else {
        sctx.root_config.format.clone()
    };

    // Minute tick or full miss: render, populate cache, write back
    let result = if let Some(segments) = lru.get(&full_key).map(|e| e.segments.clone()) {
        let r = get_prompt_with_cache(&sctx, &segments, &fmt);
        let trimmed = r.trim_end_matches('\n').to_string();
        lru.put(full_key.clone(), CachedValue {
            rendered: trimmed.clone(),
            segments,
            time_bucket: tb,
        });
        trimmed
    } else {
        let mut module_cache = ModuleCache::new();
        populate_cache(&sctx, &fmt, &mut module_cache);
        let r = get_prompt_with_cache(&sctx, &module_cache, &fmt);
        let trimmed = r.trim_end_matches('\n').to_string();
        lru.put(full_key.clone(), CachedValue {
            rendered: trimmed.clone(),
            segments: module_cache,
            time_bucket: tb,
        });
        trimmed
    };

    if let Some(ref gd) = resolved_git_dir {
        let index_mtime = get_mtime_ns(&gd.join("index"));
        let mut rc = REPO_CACHE.lock().unwrap();
        *rc = Some(RepoCache { git_dir: gd.clone(), index_mtime, ctx: Some(sctx) });
    }

    if let Some(dir) = bust_dir { let _ = std::fs::remove_dir_all(dir); }
    result
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
        let key = compute_cache_key(cwd.path(), 0, "vi", 120, 0, Path::new("__nonexistent_config__"), None, 0);

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
        let key = compute_cache_key(cwd.path(), 0, "vi", 120, 0, Path::new("__nonexistent_config__"), None, 0);

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
        let key = compute_cache_key(cwd.path(), 0, "vi", 120, 0, Path::new("__nonexistent_config__"), None, 0);

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
}
