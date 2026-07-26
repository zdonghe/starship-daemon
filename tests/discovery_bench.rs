use std::{
    path::Path,
    time::{Duration, Instant},
};

use starship_daemon::prompt::{self, RenderContext};

const N: u32 = 10;

fn bench_one_repo(
    label: &str,
    cwd: &Path,
    config: &toml::Table,
    full_format: &str,
    results: &mut Vec<(String, String, f64)>,
) {
    let git_dir = starship_daemon::find_git_dir(cwd);

    let warm_ctx = RenderContext {
        cwd: cwd.to_path_buf(),
        terminal_width: 120,
        status_code: 0,
        keymap: "viins".into(),
    };
    let _ = prompt::render_prompt_with_config(&warm_ctx, git_dir.as_deref(), config);

    let trials: Vec<(&str, &str)> = vec![
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

    if git_dir.is_some() {
        let subprocess_trials: Vec<(&str, &[&str])> = vec![
            ("git status --porcelain=2", &["status", "--porcelain=2", "--branch", "--ignore-submodules=dirty"] as &[&str]),
            ("for-each-ref (a/b)", &["for-each-ref", "--format", "%(upstream) %(upstream:track)", "refs/heads"]),
            ("git stash list", &["stash", "list"]),
        ];

        for (name, args) in &subprocess_trials {
            let mut total = Duration::ZERO;
            for _ in 0..N {
                let t = Instant::now();
                let mut cmd = std::process::Command::new("git");
                cmd.arg("-C").arg(cwd.as_os_str());
                for a in *args { cmd.arg(a); }
                let _ = cmd.output();
                total += t.elapsed();
            }
            let avg_us = total.as_secs_f64() / N as f64 * 1_000_000.0;
            results.push((label.to_string(), name.to_string(), avg_us));
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

    let mut repos: Vec<(String, String)> = vec![
        ("this repo".into(), env!("CARGO_MANIFEST_DIR").into()),
    ];

    if let Ok(extra) = std::env::var("STARSHIP_BENCH_DIRS") {
        for (i, dir) in extra.split(';').enumerate() {
            let p = Path::new(dir);
            if p.is_dir() {
                repos.push((format!("extra-{}", i + 1), dir.to_string()));
            }
        }
    }

    let mut all: Vec<(String, String, f64)> = Vec::new();

    for (label, path) in &repos {
        bench_one_repo(label, Path::new(path), &config, &full_format, &mut all);
    }

    for (label, path) in &repos {
        println!("\n=== {label} ({path}) ===");
        println!("  {:>30} {:>12}", "item", "avg us");

        let mut total_full = 0.0f64;
        for (r, key, us) in &all {
            if r != label {
                continue;
            }
            println!("  {:>30} {:>10.1}us", key, us);
            if key == "full prompt" {
                total_full = *us;
            }
        }
        println!();
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
