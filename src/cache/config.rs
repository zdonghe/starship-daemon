use std::path::{Path, PathBuf};

pub fn load_config(path: &Path) -> Option<PathBuf> {
    path.is_file().then(|| path.to_path_buf())
}

pub(crate) fn config_path_from_env(
    config_var: Option<String>,
    home_dir: Option<String>,
) -> PathBuf {
    if let Some(cfg) = config_var {
        return PathBuf::from(cfg);
    }
    home_dir
        .map(|h| PathBuf::from(h).join(".config").join("starship.toml"))
        .unwrap_or_else(|| PathBuf::from(".config/starship.toml"))
}

pub fn default_config_path() -> PathBuf {
    config_path_from_env(
        std::env::var("STARSHIP_CONFIG").ok(),
        std::env::var("USERPROFILE").ok(),
    )
}

pub fn read_config(path: &Path) -> toml::Table {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.parse::<toml::Table>().ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_config_returns_none_for_missing() {
        assert!(load_config(Path::new("__nonexistent__")).is_none());
    }

    #[test]
    fn load_config_returns_path_for_existing() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("starship.toml");
        fs::write(&p, "key = 'val'").unwrap();
        assert_eq!(load_config(&p), Some(p.clone()));
    }

    #[test]
    fn read_config_returns_table_for_valid_toml() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("cfg.toml");
        fs::write(&p, "format = 'test'\nsymbol = '$'\n").unwrap();
        let t = read_config(&p);
        assert_eq!(t.get("format").and_then(|v| v.as_str()), Some("test"));
        assert_eq!(t.get("symbol").and_then(|v| v.as_str()), Some("$"));
    }

    #[test]
    fn read_config_invalid_toml_returns_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("bad.toml");
        fs::write(&p, "key = = value").unwrap();
        let t = read_config(&p);
        assert!(t.is_empty());
    }

    #[test]
    fn read_config_missing_returns_empty_table() {
        let t = read_config(Path::new("__nonexistent__"));
        assert!(t.is_empty());
    }

    #[test]
    fn config_path_from_env_uses_config_var() {
        assert_eq!(
            config_path_from_env(Some("C:\\custom\\starship.toml".to_string()), None),
            PathBuf::from("C:\\custom\\starship.toml")
        );
    }

    #[test]
    fn config_path_from_env_config_var_wins_over_home_dir() {
        let cfg = Some("C:\\custom\\starship.toml".to_string());
        let home = Some("C:\\Users\\me".to_string());
        assert_eq!(
            config_path_from_env(cfg, home),
            PathBuf::from("C:\\custom\\starship.toml")
        );
    }

    #[test]
    fn config_path_from_env_uses_home_dir_fallback() {
        assert_eq!(
            config_path_from_env(None, Some("C:\\Users\\me".to_string())),
            PathBuf::from("C:\\Users\\me\\.config\\starship.toml")
        );
    }

    #[test]
    fn config_path_from_env_defaults_to_relative() {
        assert_eq!(
            config_path_from_env(None, None),
            PathBuf::from(".config/starship.toml")
        );
    }

    #[test]
    fn load_config_returns_none_for_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(load_config(dir.path()).is_none());
    }

    #[test]
    fn read_config_empty_file_returns_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("empty.toml");
        fs::write(&p, "").unwrap();
        let t = read_config(&p);
        assert!(t.is_empty());
    }
}
