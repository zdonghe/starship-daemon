use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use lru::LruCache;
use starship_daemon::cache::{self, CacheKey, CachedValue, RenderContext};
use starship_daemon::find_git_dir;
use toml::Table;

fn render_cold(cwd: &PathBuf, config: &Table) -> String {
    let ctx = RenderContext {
        cwd: cwd.clone(),
        terminal_width: 120,
        status_code: 0,
        keymap: "viins".to_string(),
    };
    cache::render_prompt_with_config(&ctx, None, config)
}

fn main() {
    let cwd = std::env::current_dir().unwrap();
    let config_path = cache::default_config_path();
    let config_path = cache::load_config(&config_path).unwrap();
    let config = cache::read_config(&config_path);

    println!("=== starship-daemon hot-path benchmarks ===\n");
    println!("cwd:        {:?}", cwd);
    println!("config:     {:?}", config_path);

    let gd = find_git_dir(&cwd);
    println!("git_dir:    {:?}\n", gd);

    let _ = render_cold(&cwd, &config);
    println!("(warm-up render done)\n");

    // ---------- cache hit: LruCache::get ----------
    let key = cache::compute_cache_key(&cwd, "viins", 120, 0, 0);
    let rendered = "dummy".to_string();
    let segments = starship::print::ModuleCache::new();
    let cached = CachedValue {
        rendered,
        segments,
        time_bucket: cache::current_minute(),
        status_code: 0,
    };
    let mut lru: LruCache<CacheKey, CachedValue> = LruCache::new(NonZeroUsize::new(256).unwrap());
    lru.put(key.clone(), cached);

    let n = 100_000;
    let start = Instant::now();
    for _ in 0..n { let _ = lru.get(&key); }
    let hit_ns = start.elapsed().as_nanos() as f64 / n as f64;
    println!("  LruCache::get:                       {:>8.1} ns", hit_ns);

    // ---------- cache key computation ----------
    let n = 100;
    let start = Instant::now();
    for _ in 0..n { let _ = cache::compute_cache_key(&cwd, "viins", 120, 0, 0); }
    let key_us = start.elapsed().as_nanos() as f64 / n as f64 / 1000.0;
    println!("  compute_cache_key (2 stats):         {:>8.1} us", key_us);

    // ---------- find_git_dir ----------
    let n = 10_000;
    let start = Instant::now();
    for _ in 0..n { let _ = find_git_dir(&cwd); }
    let fgd_ns = start.elapsed().as_nanos() as f64 / n as f64;
    println!("  find_git_dir ancestor walk:          {:>8.1} ns", fgd_ns);

    // ---------- full render (cache miss) ----------
    let n = 10;
    let mut total = 0.0;
    for i in 0..n {
        // Unique bust path bypasses starship's internal git_status cache
        let bust_cwd = cwd.join(format!("__bench_bust_{}__", i));
        let ctx = RenderContext {
            cwd: bust_cwd,
            terminal_width: 120,
            status_code: i,
            keymap: "viins".to_string(),
        };
        let start = Instant::now();
        let _ = cache::render_prompt_with_config(&ctx, None, &config);
        let elapsed = start.elapsed().as_nanos() as f64 / 1_000_000.0;
        if i >= 1 { total += elapsed; } // skip first (filesystem cold cache)
    }
    let avg_ms = total / ((n - 1) as f64);
    println!("  render_prompt_with_config full:      {:>8.1} ms", avg_ms);

    println!("\n--- summary ---");
    println!("  cache hit (key + lookup):             {:>8.1} us", key_us + hit_ns / 1000.0);
    println!("  cache miss (full render):             {:>8.1} ms", avg_ms);
    println!("  render is {:>6.0}x slower than cache hit", avg_ms * 1000.0 / (key_us + hit_ns / 1000.0));
}
