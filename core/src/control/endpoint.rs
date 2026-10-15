use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const ENDPOINT_FILE_NAME: &str = "control.json";
#[cfg(unix)]
pub const SOCKET_FILE_NAME: &str = "control.sock";

/// Qui heberge les process : une app native (fenetre ouverte) ou le daemon headless du CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorRole {
    App,
    Daemon,
}

impl SupervisorRole {
    pub fn as_str(self) -> &'static str {
        match self {
            SupervisorRole::App => "app",
            SupervisorRole::Daemon => "daemon",
        }
    }
}

/// Publie dans `control.json` (lisible par l'utilisateur seul) a cote de `apps.json`.
/// `address` : chemin de socket Unix, ou `127.0.0.1:<port>` sous Windows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    pub pid: u32,
    pub role: SupervisorRole,
    pub version: String,
    pub address: String,
    pub token: String,
}

impl Endpoint {
    pub fn path_in(dir: &Path) -> PathBuf {
        dir.join(ENDPOINT_FILE_NAME)
    }

    pub fn read(dir: &Path) -> Option<Self> {
        let contents = std::fs::read_to_string(Self::path_in(dir)).ok()?;
        serde_json::from_str(&contents).ok()
    }

    pub(crate) fn write(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = Self::path_in(dir);
        let tmp = path.with_extension(format!("json.tmp-{}", self.pid));
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(json.as_bytes())?;
        drop(file);
        std::fs::rename(&tmp, &path)
    }
}
