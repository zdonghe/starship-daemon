use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use starship::configs::PROMPT_ORDER;
use starship::formatter::{StringFormatter, VariableHolder};

use crate::cache;
use crate::daemon::DaemonState;
use crate::render::{self, BustDir, RenderContext};
use crate::{MAX_TOTAL_LEN, ParsedRequest, RequestKind, find_git_dir};

struct ModuleTiming {
    name: String,
    value: String,
    duration: Duration,
}

fn fmt_dur(d: Duration) -> String {
    let ms = d.as_millis();
    if ms == 0 {
        format!("{}us", d.as_micros())
    } else {
        format!("{}ms", ms)
    }
}

fn time_it<F: FnOnce() -> String>(f: F) -> (String, Duration) {
    let start = Instant::now();
    let out = f();
    (out, start.elapsed())
}

fn explicit_vars(context: &starship::context::Context, base: &str) -> BTreeSet<String> {
    let mut vars: BTreeSet<String> = BTreeSet::new();
    if let Ok(f) = StringFormatter::new(base) {
        vars.extend(f.get_variables());
    }
    if let Ok(f) = StringFormatter::new(&context.root_config.right_format) {
        vars.extend(f.get_variables());
    }
    vars
}

fn resolve_format(
    context: &starship::context::Context,
    base: &str,
    explicit: &BTreeSet<String>,
) -> String {
    if !base.contains("$all") && !base.contains("${all}") {
        return base.to_string();
    }
    let expanded: Vec<&str> = PROMPT_ORDER
        .iter()
        .copied()
        .filter(|m| !explicit.contains(*m) && !context.is_module_disabled_in_config(m))
        .collect();
    let replacement = expanded
        .iter()
        .map(|m| format!("${{{m}}}"))
        .collect::<Vec<_>>()
        .join("");
    base.replace("${all}", &replacement)
        .replace("$all", &replacement)
}

fn implicit_children(
    context: &starship::context::Context,
    kind: &str,
    explicit: &BTreeSet<String>,
) -> Vec<String> {
    let mut out = Vec::new();
    let table = match kind {
        "custom" => context.config.get_custom_modules(),
        "env_var" => context.config.get_env_var_modules(),
        _ => return out,
    };
    if let Some(table) = table {
        for (child, cfg) in table.iter() {
            let full = format!("{kind}.{child}");
            if explicit.contains(&full) {
                continue;
            }
            let disabled = cfg
                .get("disabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if disabled {
                continue;
            }
            if kind == "env_var" && !cfg.is_table() {
                continue;
            }
            out.push(full);
        }
    }
    out
}

fn module_names(
    context: &starship::context::Context,
    resolved_format: &str,
    explicit: &BTreeSet<String>,
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut push_var = |v: String| {
        if v != "all" && !names.contains(&v) {
            names.push(v);
        }
    };
    let mut push_vars = |fmt: &str| {
        if let Ok(f) = StringFormatter::new(fmt) {
            for v in f.get_variables() {
                push_var(v);
            }
        }
    };
    push_vars(resolved_format);
    if let Ok(f) = StringFormatter::new(&context.root_config.right_format) {
        for v in f.get_variables() {
            push_var(v);
        }
    }
    let mut expanded: Vec<String> = Vec::new();
    for name in names {
        match name.as_str() {
            "custom" => {
                expanded.extend(implicit_children(context, &name, explicit));
            }
            "env_var" => {
                expanded.push(name.clone());
                expanded.extend(implicit_children(context, &name, explicit));
            }
            _ => expanded.push(name),
        }
    }
    expanded.retain(|n| !context.is_module_disabled_in_config(n));
    expanded
}

fn truncate_utf8(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn timed_hit_sample(state: &DaemonState, key: &cache::CacheKey, ctx: &RenderContext) -> bool {
    state.lru.peek(key).is_some_and(|v| {
        v.time_bucket == cache::current_minute() && v.status_code == ctx.status_code
    })
}

fn timed_cold_render(
    ctx: &RenderContext,
    git_dir: Option<&Path>,
    config: &toml::Table,
) -> Duration {
    render::clear_repo_cache();
    let (_out, d) =
        time_it(|| render::render_prompt_with_config(ctx, git_dir, config, BustDir::Fresh));
    d
}

fn timed_warm_render(
    ctx: &RenderContext,
    git_dir: Option<&Path>,
    config: &toml::Table,
) -> Duration {
    let (_out, d) =
        time_it(|| render::render_prompt_with_config(ctx, git_dir, config, BustDir::Fresh));
    d
}

fn render_path_rows(cold: Duration, warm: Duration, lru: Duration) -> String {
    let mut rows = String::new();
    for (label, d) in [
        ("cold (fresh context, repo open)", cold),
        ("warm (context+repo cache reused)", warm),
        ("lru (daemon render-cache hit)", lru),
    ] {
        rows.push_str(&format!(" {:<42} {}\n", label, fmt_dur(d)));
    }
    rows
}

fn collect_module_timings(sctx: &starship::context::Context) -> Vec<ModuleTiming> {
    let explicit = explicit_vars(sctx, &sctx.root_config.format);
    let resolved = resolve_format(sctx, &sctx.root_config.format, &explicit);
    let mut mods: Vec<ModuleTiming> = Vec::new();
    for name in module_names(sctx, &resolved, &explicit) {
        let start = Instant::now();
        let value = starship::print::get_module(&name, sctx);
        let duration = start.elapsed();
        if let Some(v) = value
            && (!v.is_empty() || duration.as_millis() > 0)
        {
            mods.push(ModuleTiming {
                name,
                value: v.replace('\n', "\\n"),
                duration,
            });
        }
    }
    mods.sort_by_key(|a| Reverse(a.duration));
    mods
}

fn module_table_rows(mods: Vec<ModuleTiming>) -> String {
    let mut rows = String::new();
    rows.push_str(
        "\n Here are the timings of modules in your prompt (warm repo, >=1ms or output):\n",
    );
    for m in mods {
        rows.push_str(&format!(
            " {}  -  {}  -   \"{}\"\n",
            m.name,
            fmt_dur(m.duration),
            m.value
        ));
    }
    rows
}

pub(crate) fn build_report(state: &mut DaemonState, req: &ParsedRequest) -> String {
    debug_assert_eq!(req.kind, RequestKind::Timings);

    let props = &req.props;
    let cwd = if req.cwd.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        req.cwd.clone()
    };
    let git_dir = find_git_dir(&cwd);
    let keymap = props.keymap.clone().unwrap_or_else(|| "vi".to_string());
    let config_mtime = state.sync_config(props.starship_config.as_deref());

    let ctx = RenderContext {
        cwd: cwd.clone(),
        terminal_width: props.terminal_width,
        status_code: props.status_code,
        keymap,
    };

    let watcher_version = if let Some(repo) = git_dir.as_deref().and_then(Path::parent) {
        state.watcher.ensure(repo);
        state.watcher.flush();
        state.watcher.version(repo)
    } else {
        0
    };
    let key = cache::compute_cache_key(
        &ctx.cwd,
        &ctx.keymap,
        ctx.terminal_width,
        config_mtime,
        watcher_version,
    );

    let hit = timed_hit_sample(state, &key, &ctx);

    let cold = timed_cold_render(&ctx, git_dir.as_deref(), &state.cached_config);
    let warm = timed_warm_render(&ctx, git_dir.as_deref(), &state.cached_config);

    let _ = state.render_prompt(&ctx, git_dir.as_deref(), false, config_mtime);
    let (_lru_out, lru) =
        time_it(|| state.render_prompt(&ctx, git_dir.as_deref(), false, config_mtime));

    let mut report = format!(
        "\n Render path timing (cwd = {}):\n cache: {}\n",
        cwd.display(),
        if hit { "HIT" } else { "MISS" }
    );
    report.push_str(&render_path_rows(cold, warm, lru));

    let sctx = render::prepare_ctx(git_dir.as_deref(), &cwd, &ctx, &state.cached_config);
    let mods = collect_module_timings(&sctx);
    report.push_str(&module_table_rows(mods));

    if let Some(ref gd) = git_dir {
        render::save_repo_cache(gd, sctx);
    }

    truncate_utf8(&report, MAX_TOTAL_LEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_utf8_respects_char_boundary() {
        let s = "abc\u{4e2d}\u{6587}def";
        let out = truncate_utf8(s, 5);
        assert!(out.len() <= 5);
        let mut rebuilt = String::new();
        for ch in s.chars() {
            if rebuilt.len() + ch.len_utf8() > 5 {
                break;
            }
            rebuilt.push(ch);
        }
        assert_eq!(out, rebuilt);
        assert!(out.is_char_boundary(out.len()));
    }

    #[test]
    fn truncate_utf8_short_input_untouched() {
        assert_eq!(truncate_utf8("hello", 100), "hello");
    }

    #[test]
    fn module_names_expands_custom_and_env_var_children() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let cfg_src = r#"
format = "$all"

[custom.slow]
command = "echo slow"
[custom.hidden]
command = "echo hidden"
disabled = true
[custom.explicit_ref]
command = "echo ref"

[env_var]
PLAIN = "scalar-value"

[env_var.TABLEVAR]
variable = "TABLEVAR"
"#;
        let config: toml::Table = toml::from_str(cfg_src).unwrap();

        let props = starship::context::Properties::default();
        let base = starship::context::Context::new_with_shell_and_path(
            props,
            starship::context::Shell::Pwsh,
            starship::context::Target::Main,
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            Default::default(),
        );
        let ctx = base.set_config(config);

        let explicit: BTreeSet<String> = ["custom.explicit_ref".to_string()].into_iter().collect();
        let resolved = resolve_format(&ctx, &ctx.root_config.format, &explicit);
        let names = module_names(&ctx, &resolved, &explicit);

        assert!(names.contains(&"custom.slow".to_string()));
        assert!(!names.contains(&"custom.hidden".to_string()));
        assert!(
            !names.contains(&"custom.explicit_ref".to_string()),
            "explicitly referenced custom child must be excluded"
        );
        assert!(!names.contains(&"custom".to_string()));
        assert!(names.contains(&"env_var".to_string()));
        assert!(names.contains(&"env_var.TABLEVAR".to_string()));
        assert!(!names.contains(&"env_var.PLAIN".to_string()));
        assert!(names.contains(&"directory".to_string()));
    }
}
