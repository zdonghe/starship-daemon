use std::collections::BTreeSet;
use std::path::Path;

use lru::LruCache;

use starship::configs::PROMPT_ORDER;
use starship::formatter::StringFormatter;
use starship::formatter::VariableHolder;
use starship::print::{ModuleCache, get_prompt_with_cache};

use super::{
    CachedValue, RenderContext, bust_for_version, make_bust_dir, prepare_ctx, save_repo_cache,
    trim_prompt,
};
use crate::cache::CacheKey;

pub(super) fn render_miss(
    ctx: &RenderContext,
    git_dir: Option<&Path>,
    config: &toml::Table,
    full_key: &CacheKey,
    lru: &mut LruCache<CacheKey, CachedValue>,
    tb: u64,
) -> String {
    let resolved_gd = git_dir.map(Path::to_path_buf);

    let (key, current_dir, bust_dir, segments) = match lru.pop_entry(full_key) {
        Some((key, entry)) => (key, ctx.cwd.clone(), None, Some(entry.segments)),
        None => {
            let (current_dir, bust_dir) = match resolved_gd {
                Some(ref gd) => {
                    let bust = make_bust_dir(
                        gd,
                        bust_for_version(full_key.watcher_version, full_key.config_mtime),
                    );
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
    lru.put(
        key,
        CachedValue {
            rendered: rendered.clone(),
            segments,
            time_bucket: tb,
            status_code: ctx.status_code,
        },
    );

    if let Some(ref gd) = resolved_gd {
        save_repo_cache(gd, sctx);
    }

    if let Some(dir) = bust_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
    rendered
}

fn expand_all(context: &starship::context::Context) -> String {
    let format_str = &context.root_config.format;

    if !format_str.contains("$all") {
        return format_str.clone();
    }

    let right_str = &context.root_config.right_format;
    let left_vars = StringFormatter::new(format_str)
        .ok()
        .map_or(BTreeSet::new(), |f| f.get_variables());
    let right_vars = StringFormatter::new(right_str)
        .ok()
        .map_or(BTreeSet::new(), |f| f.get_variables());

    let explicit: BTreeSet<&str> = left_vars.union(&right_vars).map(|s| s.as_str()).collect();

    let expanded: Vec<&str> = PROMPT_ORDER
        .iter()
        .copied()
        .filter(|m| !explicit.contains(m) && !context.is_module_disabled_in_config(m))
        .collect();

    let replacement: String = expanded.iter().map(|m| format!("${}", m)).collect();

    format_str
        .replace("${all}", &replacement)
        .replace("$all", &replacement)
}

fn populate_cache(context: &starship::context::Context, format_str: &str, cache: &mut ModuleCache) {
    let formatter = match StringFormatter::new(format_str) {
        Ok(f) => f,
        Err(_) => return,
    };

    for module in formatter.get_variables() {
        if module == "all"
            || module == "time"
            || module == "status"
            || module == "character"
            || context.is_module_disabled_in_config(&module)
            || cache.contains_key(&module)
        {
            continue;
        }
        if let Some(segments) = starship::print::get_module_segments(&module, context) {
            cache.insert(module, segments);
        }
    }
}

fn resolve_format(sctx: &starship::context::Context) -> String {
    if sctx.root_config.format.contains('$') {
        expand_all(sctx)
    } else {
        sctx.root_config.format.clone()
    }
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

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::path::Path;

    use lru::LruCache;
    use toml::toml;

    use super::*;
    use crate::cache::{compute_cache_key, current_minute};
    use crate::render::{BustDir, clear_repo_cache, render_cached, render_prompt_with_config};

    fn test_ctx(cwd: &Path) -> starship::context::Context<'static> {
        let mut p = starship::context::Properties::default();
        p.status_code = Some("0".to_string());
        p.keymap = "vi".to_string();
        let mut ctx = starship::context::Context::new_with_shell_and_path(
            p,
            starship::context::Shell::Pwsh,
            starship::context::Target::Main,
            cwd.to_path_buf(),
            cwd.to_path_buf(),
            starship::context::Env::default(),
        );
        ctx.width = 120;
        ctx = ctx.set_config(toml! {
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
            p,
            starship::context::Shell::Pwsh,
            starship::context::Target::Main,
            cwd.path().to_path_buf(),
            cwd.path().to_path_buf(),
            starship::context::Env::default(),
        );
        ctx.width = 120;
        ctx = ctx.set_config(toml! {
            format = "$all"
            add_newline = false
        });

        let fmt = expand_all(&ctx);
        assert!(fmt.starts_with('$'), "expanded format starts with $");
        assert!(
            fmt.contains("$character"),
            "expanded format includes character"
        );
        assert!(fmt.len() > 10, "expanded format is longer than $all");
        assert!(
            !fmt.contains("$all"),
            "expanded format does not contain $all"
        );
    }

    #[test]
    fn expand_all_skips_disabled_modules() {
        let cwd = tempfile::TempDir::new().unwrap();
        let mut ctx = test_ctx(cwd.path());
        ctx = ctx.set_config(toml! {
            format = "$all"
            add_newline = false
            [character]
            disabled = true
        });

        let fmt = expand_all(&ctx);
        assert!(
            !fmt.contains("$character"),
            "disabled module excluded from expansion"
        );
    }

    #[test]
    fn expand_all_with_right_format_exclusions() {
        let cwd = tempfile::TempDir::new().unwrap();
        let mut ctx = test_ctx(cwd.path());
        ctx = ctx.set_config(toml! {
            format = "$all"
            right_format = "$time"
            add_newline = false
        });

        let fmt = expand_all(&ctx);
        assert!(!fmt.contains("$all"));
        assert!(
            !fmt.contains("$time"),
            "right_format modules must be excluded from $all expansion"
        );
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
        let cfg = toml! {
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
        assert_eq!(
            got3, expected,
            "time-only re-render should match full render"
        );
    }

    #[test]
    fn time_only_re_render_refreshes_bucket() {
        let cwd = tempfile::TempDir::new().unwrap();
        let ctx = RenderContext {
            cwd: cwd.path().to_path_buf(),
            terminal_width: 120,
            status_code: 0,
            keymap: "vi".to_string(),
        };
        let cfg = toml! {
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
        let tb = current_minute();
        assert_eq!(cached.time_bucket, tb, "time_bucket must be updated");
    }

    #[test]
    fn multiple_stale_bucket_rereads_preserve_cache() {
        let cwd = tempfile::TempDir::new().unwrap();
        let ctx = RenderContext {
            cwd: cwd.path().to_path_buf(),
            terminal_width: 120,
            status_code: 0,
            keymap: "vi".to_string(),
        };
        let cfg = toml! {
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
        assert!(
            !segment_keys.contains(&"character".to_string()),
            "character must not appear in cached segments (re-rendered live)"
        );
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
                assert!(
                    check.segments.contains_key(mod_name.as_str()),
                    "module {mod_name} missing after stale-bucket re-render {i}"
                );
            }
            assert!(
                !check.segments.contains_key("time"),
                "time must not appear after re-render {i}"
            );
            assert_eq!(
                check.time_bucket,
                current_minute(),
                "time_bucket must be updated after re-render {i}"
            );
            lru.put(key.clone(), check);
        }
    }

    #[test]
    fn render_cached_status_code_isolation() {
        clear_repo_cache();
        let cwd = tempfile::TempDir::new().unwrap();

        let gd = tempfile::TempDir::new().unwrap();
        let cfg = toml! {
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
            cwd: cwd.path().to_path_buf(),
            terminal_width: 120,
            status_code: 0,
            keymap: "".to_string(),
        };
        let ctx1 = RenderContext {
            cwd: cwd.path().to_path_buf(),
            terminal_width: 120,
            status_code: 1,
            keymap: "".to_string(),
        };

        let r0 = render_cached(&ctx0, Some(gd.path()), &cfg, &key, &mut lru);
        assert!(
            r0.contains("C-OK"),
            "status 0 character must use success symbol, got: {r0}"
        );
        assert!(
            r0.contains("OK0"),
            "status 0 module must show OK + 0, got: {r0}"
        );
        let (_, e0) = lru.pop_entry(&key).unwrap();
        assert_eq!(e0.status_code, 0);
        assert!(
            e0.segments.contains_key("directory"),
            "stable module must be cached on full render"
        );
        assert!(!e0.segments.contains_key("character"));
        assert!(
            !e0.segments.contains_key("status"),
            "status module must be excluded from cached segments"
        );
        lru.put(key.clone(), e0);

        let r1 = render_cached(&ctx1, Some(gd.path()), &cfg, &key, &mut lru);
        assert!(
            r1.contains("C-ERR"),
            "status 1 character must use error symbol, got: {r1}"
        );
        assert!(
            r1.contains("ERR1"),
            "status 1 module must show ERR + 1, got: {r1}"
        );
        let (_, e1) = lru.pop_entry(&key).unwrap();
        assert_eq!(e1.status_code, 1, "cached status must be refreshed");
        assert!(
            e1.segments.contains_key("directory"),
            "directory segment must survive the exit-code change"
        );
        assert!(!e1.segments.contains_key("character"));
        assert!(!e1.segments.contains_key("status"));
        lru.put(key.clone(), e1);

        let r2 = render_cached(&ctx1, Some(gd.path()), &cfg, &key, &mut lru);
        assert_eq!(r1, r2, "same status must hit Path 1 verbatim");

        let r3 = render_cached(&ctx0, Some(gd.path()), &cfg, &key, &mut lru);
        assert!(
            r3.contains("C-OK") && !r3.contains("C-ERR"),
            "status 1->0 must re-trigger a status-aware render (status check must fail), got: {r3}"
        );

        let key2 = compute_cache_key(cwd.path(), "emacs", 120, 0, 0);
        let r4 = render_cached(&ctx0, Some(gd.path()), &cfg, &key2, &mut lru);
        assert!(r4.contains("C-OK"), "fresh-key render must work, got: {r4}");
    }

    #[test]
    fn segment_reuse_output_matches_fresh_reference() {
        use crate::render::fidelity_tests::{default_cfg, reference, repo_with_commit};

        clear_repo_cache();
        let dir = repo_with_commit();
        let ctx = RenderContext {
            cwd: dir.path().to_path_buf(),
            terminal_width: 120,
            status_code: 0,
            keymap: "vi".to_string(),
        };
        let cfg = default_cfg();
        let key = compute_cache_key(dir.path(), "vi", 120, 0, 0);

        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        let _miss = render_cached(&ctx, None, &cfg, &key, &mut lru);

        let (_, mut entry) = lru.pop_entry(&key).unwrap();
        entry.time_bucket = 0;
        lru.put(key.clone(), entry);

        let reused = render_cached(&ctx, None, &cfg, &key, &mut lru);
        let want = reference(dir.path(), 0);
        assert_eq!(
            reused, want,
            "segment-reuse output must equal the starship reference"
        );
    }

    #[test]
    fn populated_modulecache_renders_identically_to_live() {
        let cwd = tempfile::TempDir::new().unwrap();
        let ctx = RenderContext {
            cwd: cwd.path().to_path_buf(),
            terminal_width: 120,
            status_code: 0,
            keymap: "vi".to_string(),
        };
        let cfg = toml! { add_newline = false };

        let (sctx_cached, fmt) = prepare_and_resolve(None, cwd.path(), &ctx, &cfg);
        let mut segments = ModuleCache::new();
        populate_cache(&sctx_cached, &fmt, &mut segments);
        assert!(
            !segments.is_empty(),
            "default $all config must produce cached segments"
        );

        let (sctx_live, fmt_live) = prepare_and_resolve(None, cwd.path(), &ctx, &cfg);
        let out_cached = trim_prompt(&get_prompt_with_cache(&sctx_cached, &segments, &fmt));
        let out_live = trim_prompt(&get_prompt_with_cache(
            &sctx_live,
            &ModuleCache::new(),
            &fmt_live,
        ));
        assert_eq!(
            out_cached, out_live,
            "populated-segment render must equal all-live render"
        );

        segments
            .get_mut("directory")
            .expect("directory must be cached")
            .clear();
        let out_poisoned = trim_prompt(&get_prompt_with_cache(&sctx_cached, &segments, &fmt));
        assert_ne!(
            out_poisoned, out_live,
            "blanked cached segment must change output - proves cache is read"
        );
    }
}
