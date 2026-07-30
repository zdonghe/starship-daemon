use std::path::{Path, PathBuf};

pub mod config;
pub mod prompt;

pub use config::*;
pub use prompt::*;

pub fn get_mtime_ns(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Hash, Eq, PartialEq, Clone)]
pub struct CacheKey {
    pub cwd: PathBuf,
    pub status_code: i32,
    pub keymap: String,
    pub terminal_width: usize,
    pub config_mtime: u64,
    pub watcher_gen: u64,
}

pub fn compute_cache_key(cwd: &Path, status_code: i32, keymap: &str, terminal_width: usize, config_mtime: u64, watcher_gen: u64) -> CacheKey {
    CacheKey {
        cwd: cwd.to_path_buf(), status_code, keymap: keymap.to_string(), terminal_width,
        config_mtime,
        watcher_gen,
    }
}

pub fn current_minute() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 60).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn get_mtime_ns_missing_returns_zero() {
        assert_eq!(get_mtime_ns(Path::new("__nonexistent_path_xyz__")), 0);
    }

    #[test]
    fn get_mtime_ns_existing_returns_nonzero() {
        let dir = tempfile::TempDir::new().unwrap();
        let f = dir.path().join("t");
        fs::write(&f, "x").unwrap();
        assert!(get_mtime_ns(&f) > 0);
    }

    #[test]
    fn cache_key_equality_reflexive() {
        let k = CacheKey {
            cwd: PathBuf::from("/a"), status_code: 0, keymap: "vi".into(), terminal_width: 120,
            config_mtime: 5, watcher_gen: 0,
        };
        assert_eq!(k, k);
    }

    #[test]
    fn cache_key_equality_different_cwd_not_equal() {
        let k1 = CacheKey {
            cwd: PathBuf::from("/a"), status_code: 0, keymap: "vi".into(), terminal_width: 120,
            config_mtime: 5, watcher_gen: 0,
        };
        let k2 = CacheKey {
            cwd: PathBuf::from("/b"), status_code: 0, keymap: "vi".into(), terminal_width: 120,
            config_mtime: 5, watcher_gen: 0,
        };
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_cache_key_all_fields() {
        let k = compute_cache_key(Path::new("/home/user"), 42, "emacs", 100, 7, 3);
        let expected = CacheKey {
            cwd: PathBuf::from("/home/user"), status_code: 42, keymap: "emacs".into(),
            terminal_width: 100, config_mtime: 7, watcher_gen: 3,
        };
        assert_eq!(k, expected, "struct comparison catches swapped-field bugs");
    }

    #[test]
    fn compute_cache_key_deterministic() {
        let k1 = compute_cache_key(Path::new("/a"), 0, "vi", 120, 0, 0);
        let k2 = compute_cache_key(Path::new("/a"), 0, "vi", 120, 0, 0);
        assert_eq!(k1, k2);
    }

    #[test]
    fn compute_cache_key_different_status_code_differentiates() {
        let k1 = compute_cache_key(Path::new("/a"), 0, "vi", 120, 0, 0);
        let k2 = compute_cache_key(Path::new("/a"), 1, "vi", 120, 0, 0);
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_cache_key_different_keymap_differentiates() {
        let k1 = compute_cache_key(Path::new("/a"), 0, "vi", 120, 0, 0);
        let k2 = compute_cache_key(Path::new("/a"), 0, "emacs", 120, 0, 0);
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_cache_key_different_terminal_width_differentiates() {
        let k1 = compute_cache_key(Path::new("/a"), 0, "vi", 120, 0, 0);
        let k2 = compute_cache_key(Path::new("/a"), 0, "vi", 80, 0, 0);
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_cache_key_different_config_mtime_differentiates() {
        let k1 = compute_cache_key(Path::new("/a"), 0, "vi", 120, 0, 0);
        let k2 = compute_cache_key(Path::new("/a"), 0, "vi", 120, 7, 0);
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_cache_key_different_watcher_gen_differentiates() {
        let k1 = compute_cache_key(Path::new("/a"), 0, "vi", 120, 0, 0);
        let k2 = compute_cache_key(Path::new("/a"), 0, "vi", 120, 0, 5);
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_cache_key_nonexistent_cwd() {
        let k = compute_cache_key(Path::new("__nonexistent__"), 0, "vi", 120, 0, 0);
        let expected = CacheKey {
            cwd: PathBuf::from("__nonexistent__"), status_code: 0, keymap: "vi".into(),
            terminal_width: 120, config_mtime: 0, watcher_gen: 0,
        };
        assert_eq!(k, expected);
    }
}
