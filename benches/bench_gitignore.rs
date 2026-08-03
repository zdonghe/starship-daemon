use std::path::Path;
use std::time::Instant;

use starship_daemon::gitignore::{self, component_match, path_match, GitignoreFilter, Rule, is_ignored_str};

fn make_filter(patterns: &[&str]) -> GitignoreFilter {
    let rules: Vec<Rule> = patterns.iter().filter_map(|s| gitignore::parse_rule_line(s)).collect();
    GitignoreFilter { rules }
}

fn bench_load(n: u32, label: &str, content: &str) {
    let dir = tempfile::tempdir().unwrap();
    let gitignore = dir.path().join(".gitignore");
    std::fs::write(&gitignore, content).unwrap();

    let start = Instant::now();
    for _ in 0..n {
        let _ = gitignore::load_gitignore(dir.path());
    }
    let elapsed = start.elapsed().as_nanos() as f64 / n as f64;
    println!("  load_gitignore ({label}):              {:>8.1} ns", elapsed);
}

fn bench_match_str(n: u32, label: &str, filter: &GitignoreFilter, paths: &[&str]) {
    let start = Instant::now();
    for _ in 0..n {
        for p in paths {
            let _ = is_ignored_str(filter, p);
        }
    }
    let per_call = start.elapsed().as_nanos() as f64 / (n as f64 * paths.len() as f64);
    println!("  is_ignored_str ({label}):              {:>8.1} ns", per_call);
}

fn bench_match(n: u32, label: &str, filter: &GitignoreFilter, paths: &[&str]) {
    let start = Instant::now();
    for _ in 0..n {
        for p in paths {
            let _ = gitignore::is_ignored(filter, Path::new(p));
        }
    }
    let per_call = start.elapsed().as_nanos() as f64 / (n as f64 * paths.len() as f64);
    println!("  is_ignored ({label}):                  {:>8.1} ns", per_call);
}

fn bench_component(n: u32, label: &str, pattern: &str, names: &[&str]) {
    let start = Instant::now();
    for _ in 0..n {
        for name in names {
            let _ = component_match(pattern, name);
        }
    }
    let per_call = start.elapsed().as_nanos() as f64 / (n as f64 * names.len() as f64);
    println!("  component_match ({label}):             {:>8.1} ns", per_call);
}

fn bench_path_match(n: u32, label: &str, parts: &[&str], paths: &[&str]) {
    let p: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
    let start = Instant::now();
    for _ in 0..n {
        for path in paths {
            let comps: Vec<&str> = path.split('/').collect();
            let _ = path_match(&p, &comps, false);
        }
    }
    let per_call = start.elapsed().as_nanos() as f64 / (n as f64 * paths.len() as f64);
    println!("  path_match ({label}):                  {:>8.1} ns", per_call);
}

fn main() {
    println!("=== gitignore overhead benchmarks ===\n");

    // warmup
    let f = make_filter(&["*.log"]);
    let _ = gitignore::is_ignored(&f, Path::new("test.log"));

    let n_load = 10_000;
    let n_match = 100_000;

    // --- load_gitignore ---
    bench_load(n_load, "empty", "");
    bench_load(n_load, "1 rule", "*.log\n");
    bench_load(n_load, "10 rules", "*.log\n*.tmp\nbuild/\ndist/\nnode_modules/\n.DS_Store\n.vscode/\n*.swp\n.env\n*.o\n");
    let big_rules: Vec<String> = (0..100).map(|i| format!("pattern_{}.log\n", i)).collect();
    bench_load(n_load, "100 rules", &big_rules.join(""));

    println!();

    // --- is_ignored_str vs is_ignored (Path round-trip overhead) ---
    let f_str = make_filter(&["*.log"]);
    bench_match(n_match, "Path round-trip, shallow", &f_str, &["test.log"]);
    bench_match_str(n_match, "direct str, shallow", &f_str, &["test.log"]);
    bench_match(n_match, "Path round-trip, deep", &f_str, &["src/foo/bar/baz.log"]);
    bench_match_str(n_match, "direct str, deep", &f_str, &["src/foo/bar/baz.log"]);

    println!();

    // --- is_ignored (1 rule) ---
    let f1 = make_filter(&["*.log"]);
    bench_match(n_match, "1 rule, match shallow", &f1, &["test.log"]);
    bench_match(n_match, "1 rule, match deep", &f1, &["src/foo/bar/baz.log"]);
    bench_match(n_match, "1 rule, no match", &f1, &["test.txt"]);

    println!();

    // --- is_ignored (10 rules) ---
    let f10 = make_filter(&[
        "*.log", "*.tmp", "build/", "dist/", "target/",
        ".DS_Store", ".vscode/", "*.swp", ".env", "*.o",
    ]);
    bench_match(n_match, "10 rules, match shallow", &f10, &["test.log"]);
    bench_match(n_match, "10 rules, match deep", &f10, &["src/foo/target/debug/test.o"]);
    bench_match(n_match, "10 rules, no match", &f10, &["src/main.rs"]);

    println!();

    // --- directory-only (ancestor walk) ---
    let f_dir = make_filter(&["target/"]);
    bench_match(n_match, "dir rule, shallow", &f_dir, &["target"]);
    bench_match(n_match, "dir rule, deep child", &f_dir, &["target/debug/starship.exe"]);
    bench_match(n_match, "dir rule, deep nested", &f_dir, &["src/foo/target/debug/starship.exe"]);

    println!();

    // --- glob patterns ---
    let f_glob = make_filter(&["src/**/*.rs"]);
    bench_match(n_match, "glob **, match", &f_glob, &["src/foo/bar/baz.rs"]);
    bench_match(n_match, "glob **, no match", &f_glob, &["src/foo/bar/baz.txt"]);

    println!();

    // --- anchored patterns ---
    let f_anch = make_filter(&["/build", "/dist", "/*.log"]);
    bench_match(n_match, "anchored, match root", &f_anch, &["build"]);
    bench_match(n_match, "anchored, no match subdir", &f_anch, &["src/build/foo.o"]);
    bench_match(n_match, "anchored, glob match root", &f_anch, &["test.log"]);

    println!();

    // --- negation patterns ---
    let f_neg = make_filter(&["*.o", "!important.o"]);
    bench_match(n_match, "negation, .o ignored", &f_neg, &["foo.o"]);
    bench_match(n_match, "negation, important unignored", &f_neg, &["important.o"]);

    // negation with override: !foo.o then *.o — last wins
    let f_neg_rev = make_filter(&["!foo.o", "*.o"]);
    bench_match(n_match, "negation, last-wins override", &f_neg_rev, &["foo.o"]);

    // negation + dir
    let f_neg_dir = make_filter(&["target/", "!target/important.o"]);
    bench_match(n_match, "negation, dir-excluded no re-include", &f_neg_dir, &["target/important.o"]);

    println!();

    // --- real-world composite ---
    let f_rust = make_filter(&[
        "/target",
        "*.rs.bk",
        "*.wasm",
        ".idea/",
        ".vscode/",
        "*.swp",
        "*.swo",
        ".DS_Store",
        "*.o",
        "*.pyc",
        "node_modules/",
        ".env",
    ]);
    bench_match(n_match, "real rust, match target", &f_rust, &["target/debug/starship.exe"]);
    bench_match(n_match, "real rust, match deep .o", &f_rust, &["src/foo/bar/baz.o"]);
    bench_match(n_match, "real rust, vs .rs", &f_rust, &["src/main.rs"]);
    bench_match(n_match, "real rust, vs Cargo.lock", &f_rust, &["Cargo.lock"]);
    bench_match(n_match, "real rust, matched dir", &f_rust, &[".vscode/settings.json"]);

    println!();

    // --- worst case: 100 rules, no match (must scan all) ---
    let many_patterns: Vec<String> = (0..100).map(|i| format!("*.ext{}", i)).collect();
    let many: Vec<&str> = many_patterns.iter().map(|s| s.as_str()).collect();
    let f100 = make_filter(&many);
    bench_match(n_match, "100 rules, match first", &f100, &["test.ext0"]);
    bench_match(n_match, "100 rules, match last", &f100, &["test.ext99"]);
    bench_match(n_match, "100 rules, no match (worst)", &f100, &["test.txt"]);

    println!();

    // --- path_match sub-operations ---
    bench_path_match(n_match, "single part, match any", &["build"], &["src/build/foo.o"]);
    bench_path_match(n_match, "anchored multi, match root", &["build", "*.o"], &["build/foo.o"]);
    bench_path_match(n_match, "anchored multi, no match", &["build", "*.o"], &["src/foo.o"]);
    bench_path_match(n_match, "** glob, match deep", &["src", "**", "*.rs"], &["src/a/b/c/lib.rs"]);

    println!();

    // --- component_match ---
    bench_component(n_match, "exact match", "build", &["build"]);
    bench_component(n_match, "star suffix", "*.log", &["test.log"]);
    bench_component(n_match, "star prefix", "test.*", &["test.log"]);
    bench_component(n_match, "question mark", "????.txt", &["test.txt"]);
    bench_component(n_match, "star no match", "*.log", &["test.txt"]);
    bench_component(n_match, "mixed star+?", "test-??.*", &["test-01.log"]);

    println!("\n--- benchmark methodology ---");
    println!("  load_gitignore: reads .gitignore from disk. {} iterations.", n_load);
    println!("  is_ignored: calls full filter. {} iterations per path.", n_match);
    println!("  dwarfed by render_prompt (~0.7ms via IPC) by a large factor;");
    println!("  the per-event cost that matters is this matcher, not the render.");
    println!("  {} path_match calls per is_ignored ancestor walk.", "O(depth*rules)");
    println!("  real file events are bounded by coalesced ReadDirectoryChangesW buffer.");
}
