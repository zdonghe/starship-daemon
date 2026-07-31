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
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();

    fn match_rec(p: &[char], n: &[char]) -> bool {
        if p.is_empty() { return n.is_empty(); }
        if n.is_empty() { return p.iter().all(|&c| c == '*'); }

        match p[0] {
            '*' => {
                match_rec(&p[1..], n) || match_rec(p, &n[1..])
            }
            '?' => match_rec(&p[1..], &n[1..]),
            c => {
                if c == n[0] {
                    match_rec(&p[1..], &n[1..])
                } else {
                    false
                }
            }
        }
    }

    match_rec(&p, &n)
}

pub fn path_match(parts: &[String], path_comps: &[&str], anchored: bool) -> bool {
    let has_ds = parts.iter().any(|p| p == "**");

    fn match_inner(pi: usize, ci: usize, parts: &[String], comps: &[&str]) -> bool {
        if pi == parts.len() {
            return ci == comps.len();
        }
        if parts[pi] == "**" {
            let remaining = parts.len() - pi - 1;
            let is_trailing = remaining == 0;
            let min_consume = if is_trailing && parts.len() > 1 { 1 } else { 0 };
            let max = comps.len().saturating_sub(remaining);
            for end in (ci + min_consume)..=max {
                if match_inner(pi + 1, end, parts, comps) {
                    return true;
                }
            }
            return false;
        }
        if ci >= comps.len() { return false; }
        if component_match(&parts[pi], comps[ci]) {
            return match_inner(pi + 1, ci + 1, parts, comps);
        }
        false
    }

    if has_ds {
        if anchored {
            match_inner(0, 0, parts, path_comps)
        } else {
            for start in 0..=path_comps.len() {
                if match_inner(0, start, parts, path_comps) {
                    return true;
                }
            }
            false
        }
    } else if parts.len() == 1 {
        if anchored {
            path_comps.first().map_or(false, |c| component_match(&parts[0], c))
        } else {
            path_comps.iter().any(|c| component_match(&parts[0], c))
        }
    } else if parts.len() <= path_comps.len() {
        let anchored_path = &path_comps[..parts.len()];
        parts.iter().zip(anchored_path.iter()).all(|(p, c)| component_match(p, c))
    } else {
        false
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
    fn load_gitignore_trailing_whitespace_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"),
            b"*.o \nbuild  \n").unwrap();
        let ig = load_gitignore(dir.path()).unwrap();
        assert_eq!(ig.rules.len(), 2);
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
