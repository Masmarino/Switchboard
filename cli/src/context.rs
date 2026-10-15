use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use switchboard_core::control::{ControlRequest, RuntimeState, SupervisorClient, DEFAULT_REQUEST_TIMEOUT};
use switchboard_core::{paths, AppConfigList, ConfigFile};
use uuid::Uuid;

use crate::daemon_spawn;
use crate::error::{CliError, ErrorCode};

/// Ce que chaque commande partage : le fichier de config et le mode de sortie.
pub struct Context {
    pub config: ConfigFile,
    pub json: bool,
}

impl Context {
    pub fn new(config_dir: Option<PathBuf>, json: bool) -> Result<Self, CliError> {
        let dir = config_dir.or_else(paths::config_dir).ok_or_else(|| CliError::from(switchboard_core::ConfigError::NoConfigDir))?;
        let dir = std::path::absolute(&dir).map_err(|e| CliError::io(dir.display(), e))?;
        Ok(Self { config: ConfigFile::in_dir(&dir), json })
    }

    pub fn dir(&self) -> &Path {
        self.config.dir()
    }

    pub fn load(&self) -> Result<AppConfigList, CliError> {
        Ok(self.config.load()?)
    }

    pub fn supervisor(&self) -> Option<SupervisorClient> {
        SupervisorClient::discover(self.dir())
    }

    pub fn require_supervisor(&self) -> Result<SupervisorClient, CliError> {
        self.supervisor().ok_or_else(|| {
            CliError::new(ErrorCode::NoSupervisor, "no Switchboard app or daemon is running for this config")
                .with_hint("start an app first (`switchboard-cli start <app>`), or open Switchboard")
        })
    }

    /// Superviseur courant, ou daemon demarre en arriere-plan si `allow_spawn`.
    pub fn supervisor_or_spawn(&self, allow_spawn: bool) -> Result<(SupervisorClient, bool), CliError> {
        if let Some(client) = self.supervisor() {
            return Ok((client, false));
        }
        if !allow_spawn {
            return Err(self.require_supervisor().unwrap_err());
        }
        daemon_spawn::spawn(self).map(|client| (client, true))
    }

    /// Etat d'execution par app, vide si personne ne supervise (rien ne peut alors tourner).
    pub fn runtimes(&self) -> Result<(Option<SupervisorClient>, HashMap<Uuid, RuntimeState>), CliError> {
        let Some(client) = self.supervisor() else { return Ok((None, HashMap::new())) };
        let states: Vec<RuntimeState> = client.request_as(&ControlRequest::Status, DEFAULT_REQUEST_TIMEOUT)?;
        Ok((Some(client), states.into_iter().map(|s| (s.id, s)).collect()))
    }
}

pub fn supervisor_json(client: Option<&SupervisorClient>) -> Value {
    match client {
        Some(client) => {
            let endpoint = client.endpoint();
            json!({ "role": endpoint.role.as_str(), "pid": endpoint.pid, "version": endpoint.version })
        }
        None => Value::Null,
    }
}

pub fn supervisor_label(client: Option<&SupervisorClient>) -> String {
    match client {
        Some(client) => format!("{} (pid {})", client.endpoint().role.as_str(), client.endpoint().pid),
        None => "none".to_string(),
    }
}
