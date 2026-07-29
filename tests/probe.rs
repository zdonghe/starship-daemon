use std::path::Path;

fn create_test_context(cwd: &Path) -> starship::context::Context<'static> {
    let mut properties = starship::context::Properties::default();
    properties.status_code = Some("0".to_string());
    properties.keymap = "vi".to_string();
    let env = starship::context::Env::default();
    let mut ctx = starship::context::Context::new_with_shell_and_path(
        properties, starship::context::Shell::Pwsh, starship::context::Target::Main,
        cwd.to_path_buf(), cwd.to_path_buf(), env,
    );
    ctx.width = 120;
    ctx = ctx.set_config(toml::toml! {
        format = "$character"
        add_newline = false
        [character]
        format = ">"
    });
    ctx
}

#[test]
fn probe_get_module_segments() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = create_test_context(dir.path());

    let segments = starship::print::get_module_segments("character", &ctx);
    assert!(segments.is_some(), "get_module_segments returned None");
    assert!(!segments.unwrap().is_empty(), "segments were empty");
}

#[test]
fn probe_module_cache_type_alias() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = create_test_context(dir.path());

    let mut cache: starship::print::ModuleCache = std::collections::HashMap::new();
    let segments = starship::print::get_module_segments("character", &ctx).unwrap();
    cache.insert("character".to_string(), segments);

    assert!(cache.contains_key("character"));
    assert_eq!(cache.len(), 1);
}

#[test]
fn probe_get_prompt_with_cache() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = create_test_context(dir.path());

    let mut cache: starship::print::ModuleCache = std::collections::HashMap::new();
    let segments = starship::print::get_module_segments("character", &ctx).unwrap();
    cache.insert("character".to_string(), segments);

    let result = starship::print::get_prompt_with_cache(&ctx, &cache, "$character");
    let expected = starship::print::get_prompt(&ctx);

    assert_eq!(result, expected, "cached prompt should match get_prompt output");
}

#[test]
fn probe_time_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx_time = starship::context::Context::new_with_shell_and_path(
        starship::context::Properties::default(),
        starship::context::Shell::Pwsh,
        starship::context::Target::Main,
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
        starship::context::Env::default(),
    );
    ctx_time.width = 120;
    ctx_time = ctx_time.set_config(toml::toml! {
        format = "$character$time"
        add_newline = false
        [character]
        format = "> "
        [time]
        disabled = false
        format = "🕐[$time](bold yellow)"
        time_format = "%H:%M"
    });

    let char_segments = starship::print::get_module_segments("character", &ctx_time).unwrap();
    let time_segments = starship::print::get_module_segments("time", &ctx_time).unwrap();

    let mut cache: starship::print::ModuleCache = std::collections::HashMap::new();
    cache.insert("character".to_string(), char_segments);
    cache.insert("time".to_string(), time_segments);

    let full = starship::print::get_prompt(&ctx_time);
    let cached = starship::print::get_prompt_with_cache(&ctx_time, &cache, "$character$time");

    assert_eq!(full, cached, "fresh cache should match get_prompt");

    let time_segments_v2 = starship::print::get_module_segments("time", &ctx_time).unwrap();
    cache.insert("time".to_string(), time_segments_v2);

    let cached_v2 = starship::print::get_prompt_with_cache(&ctx_time, &cache, "$character$time");
    assert_eq!(full, cached_v2, "replaced time segments should still match");
}

#[test]
fn probe_prompt_order_accessible() {
    let order: &[&str] = starship::configs::PROMPT_ORDER;
    assert!(order.contains(&"time"), "PROMPT_ORDER should contain 'time'");
    assert!(order.contains(&"character"), "PROMPT_ORDER should contain 'character'");
    assert!(order.contains(&"directory"), "PROMPT_ORDER should contain 'directory'");
}

#[test]
fn probe_stringformatter_direct_usage() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = create_test_context(dir.path());

    let segs = starship::print::get_module_segments("character", &ctx).unwrap();

    let result = starship::formatter::StringFormatter::new("$character")
        .unwrap()
        .map_variables_to_segments(|_m| Some(Ok(segs.clone())))
        .parse(None, Some(&ctx));

    assert!(result.is_ok(), "StringFormatter::parse should succeed");

    let mut root = starship::module::Module::new("Test", "test", None);
    root.set_segments(result.unwrap());
    let ansi_strings = root.ansi_strings_for_width(Some(120));

    assert!(!ansi_strings.is_empty(), "ansi_strings should not be empty");
    assert_eq!(ansi_strings.len(), 1, "should have one text segment");
    assert_eq!(ansi_strings[0].to_string(), ">");
}

#[test]
fn probe_all_accessible() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = create_test_context(dir.path());

    let _: &[&str] = starship::configs::PROMPT_ORDER;
    let _segs = starship::print::get_module_segments("character", &ctx);
    let _cache: starship::print::ModuleCache = std::collections::HashMap::new();
    let _ = starship::formatter::StringFormatter::new("$character");
    let _module = starship::module::Module::new("x", "y", None);
    let _disabled = ctx.is_module_disabled_in_config("character");
}
