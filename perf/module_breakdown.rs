// Standalone perf benchmarks for starship-daemon.
// Not part of the release build. Run manually:
//   cargo +nightly build --release --manifest-path perf/Cargo.toml

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use starship_daemon::prompt::{self, RenderContext};
use starship_daemon::find_git_dir;
use toml::{Table, Value};

// -- Local ClientProps (duplicated from main.rs) --

struct ClientProps {
    status_code: Option<i32>,
    keymap: Option<String>,
    terminal_width: Option<usize>,
    starship_config: Option<String>,
}

impl ClientProps {
    fn parse_json(data: &[u8]) -> Option<Self> {
        let s = std::str::from_utf8(data).ok()?;
        let s = s.trim().trim_start_matches('{').trim_end_matches('}');
        let mut status_code = None;
        let mut keymap = None;
        let mut terminal_width = None;
        let mut starship_config = None;
        let mut i = 0;
        let bytes = s.as_bytes();
        while i < bytes.len() {
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b',' || bytes[i] == b'\t' || bytes[i] == b'\n') { i += 1; }
            if i >= bytes.len() { break; }
            if bytes[i] != b'"' { break; }
            i += 1;
            let ks = i; while i < bytes.len() && bytes[i] != b'"' { i += 1; }
            if i >= bytes.len() { break; }
            let key = std::str::from_utf8(&bytes[ks..i]).ok()?; i += 1;
            while i < bytes.len() && bytes[i] != b':' { i += 1; }
            if i >= bytes.len() { break; }
            i += 1;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') { i += 1; }
            if i >= bytes.len() { break; }
            if bytes[i] == b'"' {
                i += 1; let vs = i;
                while i < bytes.len() && bytes[i] != b'"' { i += 1; }
                let val = std::str::from_utf8(&bytes[vs..i]).ok()?.to_string(); i += 1;
                match key { "keymap" => keymap = Some(val), "starship_config" => starship_config = Some(val), _ => {} }
            } else if i + 3 < bytes.len() && &bytes[i..i+4] == b"null" { i += 4; }
            else {
                let vs = i; while i < bytes.len() && bytes[i] != b',' && bytes[i] != b'}' && bytes[i] != b' ' { i += 1; }
                let val = std::str::from_utf8(&bytes[vs..i]).ok()?;
                match key { "status_code" => status_code = val.parse::<i32>().ok(), "terminal_width" => terminal_width = val.parse::<usize>().ok(), _ => {} }
            }
        }
        Some(ClientProps { status_code, keymap, terminal_width, starship_config })
    }
}

static BUST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn bust_dir(git_dir: &Path) -> PathBuf {
    let n = BUST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let b = git_dir.join("bust").join(n.to_string());
    let _ = std::fs::create_dir_all(&b);
    b
}

fn render_custom_format(cwd: &PathBuf, format: &str) -> String {
    let git_dir = find_git_dir(cwd);
    let bust = git_dir.as_ref().map(|gd| bust_dir(gd));
    let current_dir = bust.clone().unwrap_or_else(|| cwd.clone());

    let mut properties = starship::context::Properties::default();
    properties.status_code = Some("0".to_string());
    properties.keymap = "viins".to_string();

    let env = starship::context::Env::default();
    let mut sctx = starship::context::Context::new_with_shell_and_path(
        properties,
        starship::context::Shell::Pwsh,
        starship::context::Target::Main,
        current_dir,
        cwd.clone(),
        env,
    );
    sctx.width = 120;

    let mut config = Table::new();
    config.insert("format".into(), Value::String(format.to_string()));
    config.insert("scan_timeout".into(), Value::Integer(20));
    let mut os_cfg = Table::new();
    os_cfg.insert("disabled".into(), Value::Boolean(false));
    config.insert("os".into(), Value::Table(os_cfg));
    let mut time_cfg = Table::new();
    time_cfg.insert("disabled".into(), Value::Boolean(false));
    config.insert("time".into(), Value::Table(time_cfg));

    sctx = sctx.set_config(config);

    let result = starship::print::get_prompt(&sctx);
    if let Some(dir) = bust {
        let _ = std::fs::remove_dir_all(dir);
    }
    result.trim_end_matches('\n').to_string()
}

fn bench_module_breakdown(c: &mut Criterion) {
    let cwd = std::env::current_dir().unwrap();

    let _ = prompt::render_prompt(
        &RenderContext { cwd: cwd.clone(), terminal_width: 120, status_code: 0, keymap: "viins".to_string() },
        None,
    );

    let mut group = c.benchmark_group("module");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(10);

    group.bench_function("full_user_config", |b| {
        b.iter(|| {
            prompt::render_prompt(
                &RenderContext { cwd: cwd.clone(), terminal_width: 120, status_code: black_box(0), keymap: "viins".to_string() },
                None,
            )
        })
    });

    for (name, fmt) in [
        ("empty",          ""),
        ("character",      "$character"),
        ("directory",      "$directory"),
        ("os",             "$os"),
        ("time",           "$time"),
        ("fill",           "$fill"),
        ("git_branch",     "$git_branch"),
        ("git_status",     "$git_status"),
        ("git_branch_st",  "$git_branch$git_status"),
    ] {
        let fmt = fmt.to_string();
        group.bench_function(name, |b| {
            b.iter(|| render_custom_format(&cwd, black_box(&fmt)))
        });
    }

    group.finish();
}

// -- Buffer reuse micro-benchmarks -------------------

fn bench_buffer_reuse(c: &mut Criterion) {
    let sizes = [64usize, 256, 1024, 4096];
    let mut group = c.benchmark_group("buffer");

    for &size in &sizes {
        group.bench_function(format!("fresh_{}", size), |b| {
            b.iter(|| {
                let mut buf = vec![0u8; size];
                black_box(buf[0] = 1);
            })
        });

        group.bench_function(format!("reuse_{}", size), |b| {
            let mut buf = Vec::<u8>::new();
            b.iter(|| {
                buf.clear();
                buf.resize(size, 0);
                black_box(buf[0] = 1);
            })
        });
    }

    group.finish();
}

// -- JSON parser micro-benchmarks -------------------

#[derive(serde::Deserialize)]
struct PropsSerde {
    status_code: Option<i32>,
    keymap: Option<String>,
    terminal_width: Option<usize>,
    starship_config: Option<String>,
}

fn bench_json_parser(c: &mut Criterion) {
    let payloads: [(&str, &str); 3] = [
        ("simple", r#"{"status_code":0,"keymap":"viins","terminal_width":120}"#),
        ("full",   r#"{"status_code":1,"keymap":"vi","terminal_width":80,"starship_config":"C:/Users/user/.config/starship.toml"}"#),
        ("null",   r#"{"status_code":null,"keymap":"viins","terminal_width":null}"#),
    ];

    let mut group = c.benchmark_group("json");

    for (name, payload) in &payloads {
        let bytes = payload.as_bytes();
        group.bench_function(format!("hand_{}", name), |b| {
            b.iter(|| ClientProps::parse_json(black_box(bytes)))
        });

        group.bench_function(format!("serde_{}", name), |b| {
            b.iter(|| serde_json::from_str::<PropsSerde>(black_box(payload)))
        });
    }

    group.finish();
}

criterion_group!(benches, bench_module_breakdown, bench_buffer_reuse, bench_json_parser);
criterion_main!(benches);
