use std::path::PathBuf;

/// Surcharge le dossier de config (tests, CLI `--config-dir`, plusieurs profils).
pub const CONFIG_DIR_ENV: &str = "SWITCHBOARD_CONFIG_DIR";
pub const CONFIG_FILE_NAME: &str = "apps.json";

pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(CONFIG_DIR_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    directories::ProjectDirs::from("com", "skolln", "switchboard").map(|dirs| dirs.config_dir().to_path_buf())
}
