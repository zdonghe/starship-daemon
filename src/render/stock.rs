use std::path::Path;

use lru::LruCache;

use super::{CachedValue, RenderContext, bust_for_version, render_prompt_with_config};
use crate::cache::CacheKey;

pub(super) fn render_miss(
    ctx: &RenderContext,
    git_dir: Option<&Path>,
    config: &toml::Table,
    full_key: &CacheKey,
    lru: &mut LruCache<CacheKey, CachedValue>,
    tb: u64,
) -> String {
    let rendered = render_prompt_with_config(
        ctx,
        git_dir,
        config,
        bust_for_version(full_key.watcher_version, full_key.config_mtime),
    );
    lru.put(
        full_key.clone(),
        CachedValue {
            rendered: rendered.clone(),
            time_bucket: tb,
            status_code: ctx.status_code,
        },
    );
    rendered
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::path::Path;

    use lru::LruCache;
    use toml::toml;

    use super::*;
    use crate::cache::{compute_cache_key, current_minute};
    use crate::render::render_cached;

    fn cfg() -> toml::Table {
        toml! {
            format = "$character"
            add_newline = false
            [character]
            success_symbol = "C-OK"
            error_symbol = "C-ERR"
        }
    }

    fn ctx(cwd: &Path, status: i32) -> RenderContext {
        RenderContext {
            cwd: cwd.to_path_buf(),
            terminal_width: 120,
            status_code: status,
            keymap: "".to_string(),
        }
    }

    #[test]
    fn hit_serves_cached_rendered_without_rerender() {
        let cwd = tempfile::TempDir::new().unwrap();
        let key = compute_cache_key(cwd.path(), "", 120, 0, 0);
        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        lru.put(
            key.clone(),
            CachedValue {
                rendered: "SEEDED".to_string(),
                time_bucket: current_minute(),
                status_code: 0,
            },
        );
        let r = render_cached(&ctx(cwd.path(), 0), None, &cfg(), &key, &mut lru);
        assert_eq!(
            r, "SEEDED",
            "same minute + status must serve the cache, not re-render"
        );
    }

    #[test]
    fn status_change_misses_and_refreshes() {
        let cwd = tempfile::TempDir::new().unwrap();
        let key = compute_cache_key(cwd.path(), "", 120, 0, 0);
        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        lru.put(
            key.clone(),
            CachedValue {
                rendered: "SEEDED".to_string(),
                time_bucket: current_minute(),
                status_code: 0,
            },
        );
        let r = render_cached(&ctx(cwd.path(), 1), None, &cfg(), &key, &mut lru);
        assert_ne!(r, "SEEDED", "status change must miss the cache");
        assert!(
            r.contains("C-ERR"),
            "status 1 must render the error symbol, got: {r}"
        );
        let (_, e) = lru.pop_entry(&key).unwrap();
        assert_eq!(e.status_code, 1, "cached status must be refreshed");
    }

    #[test]
    fn minute_rollover_misses_and_refreshes_bucket() {
        let cwd = tempfile::TempDir::new().unwrap();
        let key = compute_cache_key(cwd.path(), "", 120, 0, 0);
        let mut lru = LruCache::new(NonZeroUsize::new(256).unwrap());
        lru.put(
            key.clone(),
            CachedValue {
                rendered: "SEEDED".to_string(),
                time_bucket: current_minute() - 1,
                status_code: 0,
            },
        );
        let r = render_cached(&ctx(cwd.path(), 0), None, &cfg(), &key, &mut lru);
        assert_ne!(r, "SEEDED", "stale time bucket must miss the cache");
        let (_, e) = lru.pop_entry(&key).unwrap();
        assert_eq!(e.time_bucket, current_minute(), "bucket must refresh");
    }
}
