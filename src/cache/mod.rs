use std::path::{Path, PathBuf};

pub mod config;
pub mod git;
pub mod prompt;

pub use config::*;
pub use git::*;
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
    pub time_bucket: u64,
    pub cwd_mtime: u64,
    pub index_mtime: u64,
    pub branch_mtime: u64,
    pub remote_mtime: u64,
    pub config_mtime: u64,
    pub watcher_gen: u64,
}

pub fn compute_cache_key(cwd: &Path, status_code: i32, keymap: &str, terminal_width: usize, time_bucket: u64, config_path: &Path, git_dir: Option<&Path>, watcher_gen: u64) -> CacheKey {
    let git_dir: Option<PathBuf> = git_dir.map(Path::to_path_buf).or_else(|| crate::find_git_dir(cwd));
    let (br_mtime, rr_mtime) = git_dir.as_ref().map(|d| get_branch_ref_mtimes(d)).unwrap_or((0, 0));
    CacheKey {
        cwd: cwd.to_path_buf(), status_code, keymap: keymap.to_string(), terminal_width, time_bucket,
        cwd_mtime: get_mtime_ns(cwd),
        index_mtime: git_dir.as_ref().map(|d| get_mtime_ns(&d.join("index"))).unwrap_or(0),
        branch_mtime: br_mtime, remote_mtime: rr_mtime,
        config_mtime: get_mtime_ns(config_path),
        watcher_gen,
    }
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
            time_bucket: 0, cwd_mtime: 1, index_mtime: 2, branch_mtime: 3, remote_mtime: 4,
            config_mtime: 5, watcher_gen: 0,
        };
        assert_eq!(k, k);
    }

    #[test]
    fn cache_key_equality_different_cwd_not_equal() {
        let k1 = CacheKey {
            cwd: PathBuf::from("/a"), status_code: 0, keymap: "vi".into(), terminal_width: 120,
            time_bucket: 0, cwd_mtime: 1, index_mtime: 2, branch_mtime: 3, remote_mtime: 4,
            config_mtime: 5, watcher_gen: 0,
        };
        let k2 = CacheKey {
            cwd: PathBuf::from("/b"), status_code: 0, keymap: "vi".into(), terminal_width: 120,
            time_bucket: 0, cwd_mtime: 1, index_mtime: 2, branch_mtime: 3, remote_mtime: 4,
            config_mtime: 5, watcher_gen: 0,
        };
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_cache_key_no_git_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg = Path::new("__nonexistent_config__");
        let k = compute_cache_key(dir.path(), 0, "vi", 120, 0, cfg, None, 0);
        assert_eq!(k.status_code, 0);
        assert_eq!(k.keymap, "vi");
        assert_eq!(k.terminal_width, 120);
        assert_eq!(k.time_bucket, 0);
        assert!(k.cwd_mtime > 0, "cwd_mtime should be > 0 for existing dir");
        assert_eq!(k.index_mtime, 0, "no index without git dir");
        assert_eq!(k.branch_mtime, 0);
        assert_eq!(k.remote_mtime, 0);
        assert_eq!(k.config_mtime, 0);
        assert_eq!(k.watcher_gen, 0);
    }

    #[test]
    fn compute_cache_key_different_status_code_differentiates() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg = Path::new("__nonexistent_config__");
        let k1 = compute_cache_key(dir.path(), 0, "vi", 120, 0, cfg, None, 0);
        let k2 = compute_cache_key(dir.path(), 1, "vi", 120, 0, cfg, None, 0);
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_cache_key_different_watcher_gen_differentiates() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg = Path::new("__nonexistent_config__");
        let k1 = compute_cache_key(dir.path(), 0, "vi", 120, 0, cfg, None, 0);
        let k2 = compute_cache_key(dir.path(), 0, "vi", 120, 0, cfg, None, 5);
        assert_ne!(k1, k2);
    }

    #[test]
    fn compute_cache_key_with_nonexistent_cwd_still_works() {
        let cfg = Path::new("__nonexistent_config__");
        let bad_cwd = Path::new("__nonexistent_cwd__");
        let k = compute_cache_key(bad_cwd, 0, "vi", 120, 0, cfg, None, 0);
        assert_eq!(k.cwd_mtime, 0);
    }
}
