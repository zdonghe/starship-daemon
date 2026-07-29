# Per-Module Segment Cache: Single Merged LRU

## Goal

Replace two-layer cache (HashMap prompt cache + single-entry segment cache) with one `LruCache<CacheKey, CachedValue>`. Fix multi-directory minute ticks (current: seg cache evicted on dir switch -> full 5-15ms re-render). Remove TTL sweep, dead code.

## Data structures

```rust
// in cache::mod.rs — remove time_bucket field
struct CacheKey {
    cwd, status_code, keymap, terminal_width,
    cwd_mtime, index_mtime, branch_mtime, remote_mtime,
    config_mtime, watcher_gen,
}

// in cache::prompt.rs — new
struct CachedValue {
    rendered: String,           // full prompt string (for full hit)
    segments: ModuleCache,      // non-time segments (for time-only re-render)
    time_bucket: u64,           // minute of last full render
    format_str: String,         // format used (detect format change without mtime change)
}

// Single cache
LruCache<CacheKey, CachedValue>  // capacity: 256
```

## Cache lookup logic

```
full_key = CacheKey{state fields}     // no time_bucket in key
tb = current_minute()
fmt = expand_all(context)            // expand $all if needed

// Fast path: full hit — peek to check, then promote + return (no segments clone)
// peek borrows immutably, released before get
if lru.peek(&full_key).is_some_and(|e| e.time_bucket == tb && e.format_str == fmt) {
    return lru.get(&full_key).unwrap().rendered.clone()  // FULL HIT: ~230ns
}

// Minute tick: must clone segments for re-render
// Use get for promotion + clone
if let Some(segments) = lru.get(&full_key).map(|e| e.segments.clone()) {
    let result = get_prompt_with_cache(&sctx, &segments, &fmt)
    let trimmed = result.trim_end_matches('\n').to_string()
    lru.put(full_key.clone(), CachedValue{trimmed.clone(), segments, tb, fmt})
    return trimmed  // MINUTE TICK: ~15μs
}

// Full miss
let mut segments = ModuleCache::new()
populate_cache(&sctx, &fmt, &mut segments)
let result = get_prompt_with_cache(&sctx, &segments, &fmt)
let trimmed = result.trim_end_matches('\n').to_string()
lru.put(full_key.clone(), CachedValue{trimmed.clone(), segments, tb, fmt})
trimmed  // FULL MISS: ~5-15ms
```

Key point: `populate_cache` skips `"time"`. `get_prompt_with_cache` fallthroughs to `handle_module("time", ...)` when cache lacks it. So time is always rendered fresh without explicit code.

## File changes

### 1. Cargo.toml — add lru

```
lru = "0.12"
```

### 2. src/cache/mod.rs — remove time_bucket

- Remove `time_bucket: u64` field from `CacheKey`
- Remove `matches_ignoring_time` method (key equality = state equality now)
- Remove `time_bucket` param from `compute_cache_key`, update body
- Remove all CacheKey literals with `time_bucket` in tests
- Remove `matches_ignoring_time_*` tests
- Add `time_bucket` helper function: `pub fn current_minute() -> u64` (moved from main.rs)

### 3. src/cache/prompt.rs — LRU renderer

- Remove `SegCacheState`, `SEG_CACHE`, `clear_segment_cache`
- Add `CachedValue` struct
- Import `lru::LruCache`, `std::num::NonZeroUsize`
- New function `render_cached`:
  - Signature: `(ctx: &RenderContext, git_dir: Option<&Path>, config: &toml::Table, full_key: &CacheKey, lru: &mut LruCache<CacheKey, CachedValue>) -> String`
  - Setup: create context, expand_all (same as current)
  - Lookup logic as described above
  - Keep REPO_CACHE write-back after render (same)
  - Keep bust directory cleanup (same)
- Remove `render_prompt_with_segment_cache` (replaced by `render_cached`)
- Keep `expand_all`, `populate_cache`, `render_prompt`, `render_prompt_with_config` unchanged
- Internal test: replace `render_prompt_with_segment_cache` test with `render_cached`

### 4. src/main.rs — single cache

- Remove: `CACHE_TTL_MINUTES`, `CACHE_MAX_ENTRIES`, `current_minute()`, `evict_stale()`
- Remove: `use std::collections::HashMap`
- Add: `use lru::LruCache`, `use std::num::NonZeroUsize`, `use starship_daemon::cache::CachedValue`
- Change `prompt_cache: HashMap<CacheKey, String>` -> `lru: LruCache<CacheKey, CachedValue>` with capacity 256
- Warmup: use `render_cached` with `&mut lru`
- Config change handler:
  - `starship_config` override: `lru.clear()` instead of `prompt_cache.clear()`
  - Config mtime change: `lru.clear()` instead of `prompt_cache.clear() + clear_segment_cache()`
- `handle_client`: remove prompt_cache get/insert/evict. Just call `render_cached` with `&mut lru`
- Keep `render_prompt_with_config` path for `disable_cache` (unchanged)

### 5. tests/common/mod.rs — remove time_bucket from helper

- `cache_key()`: remove `time_bucket: u64` param, remove from `compute_cache_key` call
- All callers updated to `r.cache_key(&cfg)`

### 6. tests/mtime_git_ops.rs — update tests

- Remove `time_bucket_field_differentiates_buckets` test (no time_bucket in key)
- All `cache_key(0, &cfg)` -> `cache_key(&cfg)`
- All `compute_cache_key(..., 0, ...)` -> `compute_cache_key(...)`

### 7. benches/bench_hotpath.rs — update

- Remove `HashMap` import, remove HashMap::get benchmark section
- Replace with LruCache::get benchmark (measure LruCache hit cost: ~80ns)
- Update `compute_cache_key` call (remove time_bucket param)
- Keep render_prompt benchmark (uses unchanged function)

## Things NOT changing

- `expand_all` — same logic, same file
- `populate_cache` — same logic (skips "time" and "all")
- `RenderContext` — unchanged
- `RepoCache` / `REPO_CACHE` — context reuse unchanged
- `get_or_create_ctx` — unchanged
- `render_prompt_with_config` — still available, used by disable_cache path and render_git_ops tests
- `render_prompt` — unchanged, used by bench_hotpath
- `probe.rs` tests — unchanged (test Starship API, not daemon cache)
