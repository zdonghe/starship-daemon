use std::path::Path;
use std::fs;

pub struct Rule {
    pub parts: Vec<String>,
    pub negate: bool,
    pub dir_only: bool,
    pub anchored: bool,
}

pub struct GitignoreFilter {
    pub rules: Vec<Rule>,
}

pub fn load_gitignore(repo_root: &Path) -> Option<GitignoreFilter> {
    let gitignore_path = repo_root.join(".gitignore");
    let content = fs::read_to_string(&gitignore_path).ok()?;
    let mut rules = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let trimmed = line.strip_prefix('!').unwrap_or(line);
        let stripped = trimmed.strip_prefix('/').unwrap_or(trimmed);
        let pattern = stripped.strip_suffix('/').unwrap_or(stripped);
        let negate = line != trimmed;
        let anchored = trimmed != stripped || (pattern.contains('/') && !pattern.starts_with("**/"));
        let dir_only = stripped != pattern;
        if pattern.is_empty() { continue; }
        let parts: Vec<String> = pattern.split('/').map(|s| s.to_string()).collect();
        rules.push(Rule { parts, negate, dir_only, anchored });
    }
    if rules.is_empty() { return None; }
    Some(GitignoreFilter { rules })
}

pub fn is_ignored(ig: &GitignoreFilter, event_path: &Path) -> bool {
    let path_str = event_path.to_string_lossy().replace('\\', "/");
    is_ignored_str(ig, &path_str)
}

pub fn is_ignored_str(ig: &GitignoreFilter, event_path: &str) -> bool {
    let path_comps: Vec<&str> = event_path.split('/').collect();

    let mut result = false;
    for depth in (0..path_comps.len()).rev() {
        let comps = &path_comps[..=depth];
        for rule in &ig.rules {
            if !rule.dir_only { continue; }
            if path_match(&rule.parts, comps, rule.anchored) {
                result = !rule.negate;
            }
        }
        if result { return true; }
    }

    for rule in &ig.rules {
        if rule.dir_only { continue; }
        if path_match(&rule.parts, &path_comps, rule.anchored) {
            result = !rule.negate;
        }
    }

    result
}

pub fn component_match(pattern: &str, name: &str) -> bool {
    fn match_component(p: &str, n: &str) -> bool {
        if p.is_empty() {
            return n.is_empty();
        }
        if n.is_empty() {
            return p.bytes().all(|b| b == b'*');
        }

        let pc = p.chars().next().unwrap();
        match pc {
            '*' => {
                match_component(&p[pc.len_utf8()..], n)
                    || match_component(p, &n[n.chars().next().unwrap().len_utf8()..])
            }
            '?' => match_component(&p[pc.len_utf8()..], &n[n.chars().next().unwrap().len_utf8()..]),
            c => {
                let nc = n.chars().next().unwrap();
                c == nc && match_component(&p[pc.len_utf8()..], &n[nc.len_utf8()..])
            }
        }
    }

    match_component(pattern, name)
}

pub fn path_match(parts: &[String], path_comps: &[&str], anchored: bool) -> bool {
    if parts.iter().any(|p| p == "**") {
        return match_with_doublestar(parts, path_comps, anchored);
    }
    match parts.len() {
        1 => match_single_component(&parts[0], path_comps, anchored),
        n if n <= path_comps.len() => match_leading_components(parts, path_comps),
        _ => false,
    }
}

fn match_single_component(part: &str, path_comps: &[&str], anchored: bool) -> bool {
    if anchored {
        path_comps.first().map_or(false, |c| component_match(part, c))
    } else {
        path_comps.iter().any(|c| component_match(part, c))
    }
}

// Multi-component patterns with no "**" are anchored by construction (a "/" in
// a pattern implies anchoring in gitignore), so compare the leading prefix.
fn match_leading_components(parts: &[String], path_comps: &[&str]) -> bool {
    parts.iter().zip(path_comps.iter()).all(|(p, c)| component_match(p, c))
}

fn match_with_doublestar(parts: &[String], path_comps: &[&str], anchored: bool) -> bool {
    fn walk(pi: usize, ci: usize, parts: &[String], comps: &[&str]) -> bool {
        if pi == parts.len() {
            return ci == comps.len();
        }
        if parts[pi] == "**" {
            let remaining = parts.len() - pi - 1;
            // A trailing "**" after other parts must consume at least one
            // component, so "a/**" does not also match "a" itself.
            let min_consume = if remaining == 0 && parts.len() > 1 { 1 } else { 0 };
            let max = comps.len().saturating_sub(remaining);
            for end in (ci + min_consume)..=max {
                if walk(pi + 1, end, parts, comps) {
                    return true;
                }
            }
            return false;
        }
        if ci >= comps.len() { return false; }
        if component_match(&parts[pi], comps[ci]) {
            return walk(pi + 1, ci + 1, parts, comps);
        }
        false
    }

    if anchored {
        walk(0, 0, parts, path_comps)
    } else {
        (0..=path_comps.len()).any(|start| walk(0, start, parts, path_comps))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mkrule(pat: &str, dir_only: bool, anchored: bool) -> Rule {
        let parts: Vec<String> = pat.split('/').map(|s| s.to_string()).collect();
        Rule { parts, negate: false, dir_only, anchored }
    }

    fn mkrulen(pat: &str, negate: bool, dir_only: bool, anchored: bool) -> Rule {
        let has_bang = pat.starts_with('!');
        let stripped = if has_bang { &pat[1..] } else { pat };
        let parts: Vec<String> = stripped.split('/').map(|s| s.to_string()).collect();
        Rule { parts, negate: negate || has_bang, dir_only, anchored }
    }

    fn is_ignored_path(ig: &GitignoreFilter, path: &str) -> bool {
        is_ignored(ig, Path::new(path))
    }

    fn mkfilter(rules: Vec<Rule>) -> GitignoreFilter {
        GitignoreFilter { rules }
    }

    #[test]
    fn component_match_exact() {
        assert!(component_match("hello", "hello"));
    }

    #[test]
    fn component_match_star_zero() {
        assert!(component_match("*.txt", ".txt"));
    }

    #[test]
    fn component_match_star_prefix() {
        assert!(component_match("*.txt", "readme.txt"));
    }

    #[test]
    fn component_match_star_suffix() {
        assert!(component_match("README*", "README.md"));
    }

    #[test]
    fn component_match_star_middle() {
        assert!(component_match("a*c", "abc"));
        assert!(component_match("a*c", "abbc"));
        assert!(component_match("a*c", "ac"));
    }

    #[test]
    fn component_match_qmark_exact() {
        assert!(component_match("?.txt", "a.txt"));
    }

    #[test]
    fn component_match_qmark_wrong_len() {
        assert!(!component_match("?.txt", "ab.txt"));
    }

    #[test]
    fn component_match_qmark_combined_star() {
        assert!(component_match("???-*", "abc-main"));
        assert!(!component_match("???-*", "ab-main"));
    }

    #[test]
    fn component_match_no_match() {
        assert!(!component_match("hello", "world"));
        assert!(!component_match("*.rs", "main.rb"));
    }

    #[test]
    fn component_match_multi_star() {
        assert!(component_match("a*b*c", "axbyc"));
        assert!(component_match("a*b*c", "abc"));
    }

    #[test]
    fn component_match_only_star() {
        assert!(component_match("*", "anything"));
    }

    #[test]
    fn component_match_multibyte() {
        assert!(component_match("*.txt", "数据.txt"));
        assert!(component_match("数据*.txt", "数据导出.txt"));
        assert!(component_match("?.txt", "数.txt"));
        assert!(component_match("?数据?", "A数据B"));
        assert!(component_match("*数据", "x数据"));
        assert!(!component_match("?.txt", "数据.txt"));
        assert!(!component_match("?数据?", "AB"));
    }

    #[test]
    fn mixed_negate_last_match_wins() {
        let ig = mkfilter(vec![
            mkrulen("*.log", false, false, false),
            mkrulen("!important.log", true, false, false),
        ]);
        assert!(is_ignored_path(&ig, "debug.log"));
        assert!(!is_ignored_path(&ig, "important.log"));
    }

    #[test]
    fn path_match_single_unanchored_anywhere() {
        let r = mkrule("build", false, false);
        assert!(path_match(&r.parts, &["build"], false));
        assert!(path_match(&r.parts, &["src", "build"], false));
        assert!(path_match(&r.parts, &["src", "build", "foo.o"], false));
    }

    #[test]
    fn path_match_single_anchored_root_only() {
        let r = mkrule("build", false, true);
        assert!(path_match(&r.parts, &["build"], true));
        assert!(!path_match(&r.parts, &["src", "build"], true));
        assert!(!path_match(&r.parts, &["x", "y", "build"], true));
    }

    #[test]
    fn path_match_multi_anchored() {
        let r = mkrule("sub/dir", false, false);
        assert!(path_match(&r.parts, &["sub", "dir"], false));
        assert!(!path_match(&r.parts, &["a", "sub", "dir"], false));
        assert!(!path_match(&r.parts, &["sub"], false));
    }

    #[test]
    fn path_match_doublestar_cross_component() {
        let r = mkrule("a/**/b", false, true);
        assert!(path_match(&r.parts, &["a", "b"], true));
        assert!(path_match(&r.parts, &["a", "x", "b"], true));
        assert!(path_match(&r.parts, &["a", "x", "y", "b"], true));
        assert!(!path_match(&r.parts, &["a", "x"], true));
        assert!(!path_match(&r.parts, &["x", "a", "b"], true));
    }

    #[test]
    fn path_match_doublestar_prefix() {
        let r = mkrule("**/build", false, false);
        assert!(path_match(&r.parts, &["build"], false));
        assert!(path_match(&r.parts, &["src", "build"], false));
        assert!(!path_match(&r.parts, &["src", "build", "foo.o"], false));
    }

    #[test]
    fn path_match_doublestar_suffix() {
        let r = mkrule("build/**", false, true);
        assert!(!path_match(&r.parts, &["build"], true), "build/** must not match bare build");
        assert!(path_match(&r.parts, &["build", "foo.o"], true));
        assert!(path_match(&r.parts, &["build", "sub", "foo.o"], true));
        assert!(!path_match(&r.parts, &["x", "build", "foo.o"], true));
    }

    #[test]
    fn path_match_trailing_doublestar_requires_directory_context() {
        let r = mkrule("a/**", false, true);
        assert!(!path_match(&r.parts, &["a"], true), "a/** must not match bare a");
        assert!(path_match(&r.parts, &["a", "f"], true));
        assert!(path_match(&r.parts, &["a", "x", "f"], true));
        assert!(!path_match(&r.parts, &["x", "a", "f"], true));
    }

    #[test]
    fn path_match_no_match() {
        let r = mkrule("*.rs", false, false);
        assert!(path_match(&r.parts, &["main.rs"], false));
        assert!(!path_match(&r.parts, &["main.rb"], false));
        assert!(path_match(&r.parts, &["src", "main.rs"], false));
    }

    #[test]
    fn load_gitignore_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_gitignore(dir.path()).is_none());
    }

    #[test]
    fn load_gitignore_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), b"").unwrap();
        assert!(load_gitignore(dir.path()).is_none());
    }

    #[test]
    fn load_gitignore_comments_and_blanks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), b"# comment\n\n  \n").unwrap();
        assert!(load_gitignore(dir.path()).is_none());
    }

    #[test]
    fn load_gitignore_simple_patterns() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"),
            b"*.o\nbuild\ntarget/\n!important.o\n/tmp\n").unwrap();
        let ig = load_gitignore(dir.path()).unwrap();
        assert_eq!(ig.rules.len(), 5);

        assert_eq!(ig.rules[0].parts, vec!["*.o"]);
        assert!(!ig.rules[0].negate);
        assert!(!ig.rules[0].dir_only);
        assert!(!ig.rules[0].anchored);

        assert_eq!(ig.rules[1].parts, vec!["build"]);
        assert!(!ig.rules[1].dir_only);

        assert_eq!(ig.rules[2].parts, vec!["target"]);
        assert!(ig.rules[2].dir_only);

        assert_eq!(ig.rules[3].parts, vec!["important.o"]);
        assert!(ig.rules[3].negate);
        assert!(!ig.rules[3].anchored);

        assert_eq!(ig.rules[4].parts, vec!["tmp"]);
        assert!(ig.rules[4].anchored);
    }

    #[test]
    fn doublestar_middle_slash_is_anchored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), b"a/**/b\n").unwrap();
        let ig = load_gitignore(dir.path()).unwrap();
        assert!(ig.rules[0].anchored, "a/**/b has a middle slash, must be anchored");
        assert!(is_ignored_path(&ig, "a/b"));
        assert!(is_ignored_path(&ig, "a/x/b"));
        assert!(is_ignored_path(&ig, "a/x/y/b"));
        assert!(!is_ignored_path(&ig, "x/a/b"), "anchored a/**/b must not match x/a/b");
    }

    #[test]
    fn doublestar_prefix_is_unanchored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), b"**/build\n").unwrap();
        let ig = load_gitignore(dir.path()).unwrap();
        assert!(!ig.rules[0].anchored, "**/build matches at any level");
        assert!(is_ignored_path(&ig, "build"));
        assert!(is_ignored_path(&ig, "x/build"));
        assert!(is_ignored_path(&ig, "x/y/build"));
    }

    #[test]
    fn doublestar_suffix_is_anchored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), b"build/**\n").unwrap();
        let ig = load_gitignore(dir.path()).unwrap();
        assert!(ig.rules[0].anchored, "build/** has a middle slash, must be anchored");
        assert!(!is_ignored_path(&ig, "build"), "build/** must not ignore bare build");
        assert!(is_ignored_path(&ig, "build/foo.o"));
        assert!(!is_ignored_path(&ig, "x/build"), "anchored build/** must not match x/build");
        assert!(!is_ignored_path(&ig, "x/build/foo.o"));
    }

    #[test]
    fn doublestar_prefix_multi_segment_is_unanchored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), b"**/foo/bar\n").unwrap();
        let ig = load_gitignore(dir.path()).unwrap();
        assert!(!ig.rules[0].anchored, "**/foo/bar matches at any level");
        assert!(is_ignored_path(&ig, "foo/bar"));
        assert!(is_ignored_path(&ig, "x/foo/bar"));
        assert!(is_ignored_path(&ig, "x/y/foo/bar"));
        assert!(!is_ignored_path(&ig, "foo/baz"));
    }

    #[test]
    fn is_ignored_dir_only_transitive() {
        let ig = mkfilter(vec![
            mkrule("node_modules", true, false),
        ]);
        assert!(is_ignored_path(&ig, "node_modules"));
        assert!(is_ignored_path(&ig, "node_modules/foo.js"));
        assert!(is_ignored_path(&ig, "src/node_modules/foo.js"));
    }

    #[test]
    fn is_ignored_non_dir_match() {
        let ig = mkfilter(vec![
            mkrule("*.o", false, false),
        ]);
        assert!(is_ignored_path(&ig, "main.o"));
        assert!(is_ignored_path(&ig, "sub/main.o"));
        assert!(!is_ignored_path(&ig, "main.c"));
    }

    #[test]
    fn is_ignored_negation_reinclude() {
        let ig = mkfilter(vec![
            mkrulen("*.o", false, false, false),
            mkrulen("!important.o", false, false, false),
        ]);
        assert!(is_ignored_path(&ig, "main.o"));
        assert!(!is_ignored_path(&ig, "important.o"));
    }

    #[test]
    fn is_ignored_anchored_single() {
        let ig = mkfilter(vec![
            mkrule("build", false, true),
        ]);
        assert!(is_ignored_path(&ig, "build"));
        assert!(!is_ignored_path(&ig, "src/build"));
        assert!(!is_ignored_path(&ig, "src/build/foo.o"));
    }

    #[test]
    fn is_ignored_anchored_dir_only() {
        let ig = mkfilter(vec![
            mkrule("target", true, true),
        ]);
        assert!(is_ignored_path(&ig, "target"));
        assert!(is_ignored_path(&ig, "target/debug"));
        assert!(!is_ignored_path(&ig, "src/target"));
        assert!(!is_ignored_path(&ig, "src/target/debug"));
    }

    #[test]
    fn is_ignored_multi_segment_anchored() {
        let ig = mkfilter(vec![
            mkrule("sub/dir", false, false),
        ]);
        assert!(is_ignored_path(&ig, "sub/dir"));
        assert!(!is_ignored_path(&ig, "a/sub/dir"));
    }

    #[test]
    fn is_ignored_no_match() {
        let ig = mkfilter(vec![
            mkrule("*.o", false, false),
            mkrule("build", true, true),
        ]);
        assert!(!is_ignored_path(&ig, "main.c"));
        assert!(!is_ignored_path(&ig, "src/lib.rs"));
    }

    #[test]
    fn is_ignored_mixed_rules_negated_then_include() {
        let ig = mkfilter(vec![
            mkrulen("!*.txt", false, false, false),
            mkrulen("*.o", false, false, false),
        ]);
        assert!(is_ignored_path(&ig, "main.o"));
        assert!(!is_ignored_path(&ig, "readme.txt"));
    }

    #[test]
    fn is_ignored_dir_negation_still_blocks() {
        let ig = mkfilter(vec![
            mkrule("build", true, false),
            mkrulen("!build/foo.o", false, false, false),
        ]);
        assert!(is_ignored_path(&ig, "build/foo.o"));
    }

    #[test]
    fn is_ignored_multiple_dir_only() {
        let ig = mkfilter(vec![
            mkrule("node_modules", true, false),
            mkrule("target", true, false),
        ]);
        assert!(is_ignored_path(&ig, "node_modules/pkg/index.js"));
        assert!(is_ignored_path(&ig, "target/debug/build"));
        assert!(!is_ignored_path(&ig, "src/main.rs"));
    }

    #[test]
    fn is_ignored_glob_pattern() {
        let ig = mkfilter(vec![
            mkrule("*.log", false, false),
            mkrule("tmp/**", false, false),
        ]);
        assert!(is_ignored_path(&ig, "error.log"));
        assert!(is_ignored_path(&ig, "tmp/x/y/z.txt"));
        assert!(!is_ignored_path(&ig, "main.rs"));
    }

    #[test]
    fn is_ignored_deep_nested_dir() {
        let ig = mkfilter(vec![
            mkrule("a/deep/dir", true, false),
        ]);
        assert!(is_ignored_path(&ig, "a/deep/dir"));
        assert!(is_ignored_path(&ig, "a/deep/dir/file.txt"));
        assert!(!is_ignored_path(&ig, "a/other/file.txt"));
        assert!(!is_ignored_path(&ig, "other/a/deep/dir"));
    }
}
