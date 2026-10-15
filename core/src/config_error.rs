use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum ConfigError {
    NoConfigDir,
    Io { path: PathBuf, source: std::io::Error },
    Invalid { path: PathBuf, message: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NoConfigDir => write!(f, "no config directory available on this system"),
            ConfigError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            ConfigError::Invalid { path, message } => write!(f, "{} is not a valid Switchboard config: {message}", path.display()),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
