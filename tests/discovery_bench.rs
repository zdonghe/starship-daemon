use std::{
    path::Path,
    time::{Duration, Instant},
};

use starship_daemon::prompt::{self, RenderContext};

const N: u32 = 10;

fn bench_one_repo(
    label: &str,
    cwd_str: &str,
    config: &toml::Table,
    full_format: &str,
    results: &mut Vec<(String, String, f64)>,
) {
    let cwd = Path::new(cwd_str);
    let git_dir = starship_daemon::find_git_dir(cwd);

    // warmup
    let warm_ctx = RenderContext {
        cwd: cwd.to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "viins".into(),
    };
    let _ = prompt::render_prompt_with_config(&warm_ctx, git_dir.as_deref(), config);

    // -------------------------------------------------------
    // A. Module-level timing
    // -------------------------------------------------------
    let mut trials: Vec<(&str, &str)> = vec![
        ("full prompt", full_format),
        ("os", "$os"),
        ("directory", "$directory"),
        ("git_branch", "$git_branch"),
        ("git_status", "$git_status"),
        ("fill", "$fill"),
        ("time", "$time"),
        ("character", "$character"),
    ];

    for (mod_name, mod_fmt) in &trials {
        let ctx = RenderContext {
            cwd: cwd.to_path_buf(),
            terminal_width: 120,
            status_code: 0,
            keymap: "viins".into(),
        };
        let mut total = Duration::ZERO;
        for _ in 0..N {
            let mut mod_cfg = config.clone();
            mod_cfg.insert(
                "format".into(),
                toml::Value::String(mod_fmt.to_string()),
            );
            let t = Instant::now();
            let _ = prompt::render_prompt_with_config(&ctx, git_dir.as_deref(), &mod_cfg);
            total += t.elapsed();
        }
        let avg_us = total.as_secs_f64() / N as f64 * 1_000_000.0;
        results.push((label.to_string(), mod_name.to_string(), avg_us));
    }

    // -------------------------------------------------------
    // B. Git sub-operation timing (gix native = Path B)
    // -------------------------------------------------------
    if let Some(gd) = git_dir.as_deref() {
        // B1. gix open + head_name
        {
            let mut total = Duration::ZERO;
            for _ in 0..N {
                let t = Instant::now();
                let repo = gix::ThreadSafeRepository::open(gd).unwrap();
                let thr = repo.to_thread_local();
                let _ = thr.head_name().ok().flatten();
                total += t.elapsed();
            }
            let avg_us = total.as_secs_f64() / N as f64 * 1_000_000.0;
            results.push((label.to_string(), "gix open+head".into(), avg_us));
        }
        // B2. gix status + iterate (index + worktree diff)
        {
            let mut total = Duration::ZERO;
            for _ in 0..N {
                let repo = gix::ThreadSafeRepository::open(gd).unwrap();
                let thr = repo.to_thread_local();
                let t = Instant::now();
                if let Ok(plat) = thr.status(gix::features::progress::Discard) {
                    if let Ok(iter) = plat.into_iter(None) {
                        let _n = iter.count();
                    }
                }
                total += t.elapsed();
            }
            let avg_us = total.as_secs_f64() / N as f64 * 1_000_000.0;
            results.push((label.to_string(), "gix status+iter".into(), avg_us));
        }

        // B3. ahead/behind via for-each-ref (subprocess)
        {
            let mut total = Duration::ZERO;
            for _ in 0..N {
                let t = Instant::now();
                let _ = std::process::Command::new("git")
                    .args([
                        "-C",
                        cwd.to_str().unwrap(),
                        "for-each-ref",
                        "--format",
                        "%(upstream) %(upstream:track)",
                        "refs/heads",
                    ])
                    .output();
                total += t.elapsed();
            }
            let avg_us = total.as_secs_f64() / N as f64 * 1_000_000.0;
            results.push((label.to_string(), "for-each-ref (a/b)".into(), avg_us));
        }

        // B4. stash count via git stash list
        {
            let mut total = Duration::ZERO;
            for _ in 0..N {
                let t = Instant::now();
                let _ = std::process::Command::new("git")
                    .args(["-C", cwd.to_str().unwrap(), "stash", "list"])
                    .output();
                total += t.elapsed();
            }
            let avg_us = total.as_secs_f64() / N as f64 * 1_000_000.0;
            results.push((label.to_string(), "git stash list".into(), avg_us));
        }
    }

    // -------------------------------------------------------
    // C. Git sub-process timing (full git status = Path A)
    // -------------------------------------------------------
    if git_dir.is_some() {
        // C1. git status --porcelain=2 --branch
        {
            let mut total = Duration::ZERO;
            for _ in 0..N {
                let t = Instant::now();
                let _ = std::process::Command::new("git")
                    .args([
                        "-C",
                        cwd.to_str().unwrap(),
                        "status",
                        "--porcelain=2",
                        "--branch",
                        "--ignore-submodules=dirty",
                    ])
                    .output();
                total += t.elapsed();
            }
            let avg_us = total.as_secs_f64() / N as f64 * 1_000_000.0;
            results.push((label.to_string(), "git status --porcelain=2".into(), avg_us));
        }
    }
}

#[test]
fn bench_cross_repo() {
    let config_path = prompt::default_config_path();
    let config = prompt::read_config(&config_path);
    let full_format: String = config
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("$all")
        .to_string();

    let repos: Vec<(&str, &str)> = vec![
        ("dotfiles", r"C:\Users\Dong\Documents\dotfiles"),
        ("starship-daemon", r"C:\Users\Dong\Documents\Code\starship-daemon"),
        ("vimium-c", r"C:\Users\Dong\Documents\Code\vimium-c"),
        ("app-launchers", r"C:\Users\Dong\Documents\Code\app-launchers"),
    ];

    let mut all: Vec<(String, String, f64)> = Vec::new();

    for (label, path) in &repos {
        bench_one_repo(label, path, &config, &full_format, &mut all);
    }

    // Print grouped by repo
    for (label, path) in &repos {
        println!("\n=== {label} ({path}) ===");
        println!("  {:>30} {:>12}", "item", "avg us");

        let mut total_full = 0.0f64;
        let mut total_mods = 0.0f64;
        for (r, key, us) in &all {
            if r != label {
                continue;
            }
            println!("  {:>30} {:>10.1}us", key, us);
            if key == "full prompt" {
                total_full = *us;
            } else if key == "os"
                || key == "directory"
                || key == "git_branch"
                || key == "git_status"
                || key == "fill"
                || key == "time"
                || key == "character"
            {
                total_mods += us;
            }
        }
        println!();
        println!("  {:>30} {:>10.1}us", "sum of modules", total_mods);
        println!("  {:>30} {:>10.1}us  ({:.1}%)",
            "git_status % of full",
            all.iter()
                .find(|(r, k, _)| r == label && k == "git_status")
                .map(|(_, _, us)| *us)
                .unwrap_or(0.0),
            all.iter()
                .find(|(r, k, _)| r == label && k == "git_status")
                .map(|(_, _, us)| *us / total_full * 100.0)
                .unwrap_or(0.0));
    }
}
