use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use starship_daemon::cache::{self, CacheKey, RenderContext};
use starship_daemon::find_git_dir;

/// Cold render to force starship's internal cache population
fn render_cold(cwd: &PathBuf) -> String {
    let ctx = RenderContext {
        cwd: cwd.clone(),
        terminal_width: 120,
        status_code: 0,
        keymap: "viins".to_string(),
    };
    cache::render_prompt(&ctx, None)
}

fn main() {
    let cwd = std::env::current_dir().unwrap();
    let config_path = cache::default_config_path();
    let config = cache::load_config(&config_path).unwrap();

    println!("=== starship-daemon hot-path benchmarks ===\n");
    println!("cwd:        {:?}", cwd);
    println!("config:     {:?}", config);

    let gd = find_git_dir(&cwd);
    println!("git_dir:    {:?}\n", gd);

    let _ = render_cold(&cwd);
    println!("(warm-up render done)\n");

    // ---------- cache hit: HashMap::get ----------
    let key = cache::compute_cache_key(&cwd, 0, "viins", 120, 0, &config, gd.as_deref(), 0);
    let mut cache: HashMap<CacheKey, String> = HashMap::new();
    cache.insert(key.clone(), "dummy".into());

    let n = 100_000;
    let start = Instant::now();
    for _ in 0..n { let _ = cache.get(&key); }
    let hit_ns = start.elapsed().as_nanos() as f64 / n as f64;
    println!("  HashMap::get:                        {:>8.1} ns", hit_ns);

    // ---------- cache key computation ----------
    let n = 100;
    let start = Instant::now();
    for _ in 0..n { let _ = cache::compute_cache_key(&cwd, 0, "viins", 120, 0, &config, gd.as_deref(), 0); }
    let key_us = start.elapsed().as_nanos() as f64 / n as f64 / 1000.0;
    println!("  compute_cache_key (6+ stats):        {:>8.1} us", key_us);

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
        let _ = cache::render_prompt(&ctx, None);
        let elapsed = start.elapsed().as_nanos() as f64 / 1_000_000.0;
        if i >= 1 { total += elapsed; } // skip first (filesystem cold cache)
    }
    let avg_ms = total / ((n - 1) as f64);
    println!("  render_prompt full:                  {:>8.1} ms", avg_ms);

    println!("\n--- summary ---");
    println!("  cache hit (key + lookup):             {:>8.1} us", key_us + hit_ns / 1000.0);
    println!("  cache miss (full render):             {:>8.1} ms", avg_ms);
    println!("  render is {:>6.0}x slower than cache hit", avg_ms * 1000.0 / (key_us + hit_ns / 1000.0));
}
