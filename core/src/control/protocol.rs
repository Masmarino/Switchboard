use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::control::endpoint::SupervisorRole;

/// Une requete par connexion : une ligne JSON `ControlEnvelope`, une ligne JSON `ControlResponse`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlRequest {
    Ping,
    Status,
    Start { ids: Vec<Uuid> },
    Stop { ids: Vec<Uuid> },
    Restart { ids: Vec<Uuid> },
    StartAll,
    StopAll,
    Logs { id: Uuid, since: u64 },
    ClearLogs { id: Uuid },
    /// Daemon seulement : arrete ses apps, rend la main et repond avec les ids qui tournaient,
    /// pour qu'une app native qui demarre puisse les relancer elle-meme.
    Handover,
    /// Daemon seulement : arrete ses apps puis se termine.
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlEnvelope {
    pub token: String,
    pub request: ControlRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ControlResponse {
    Ok { data: serde_json::Value },
    Error { code: String, message: String },
}

impl ControlResponse {
    pub fn ok(data: impl Serialize) -> Self {
        ControlResponse::Ok { data: serde_json::to_value(data).unwrap_or(serde_json::Value::Null) }
    }

    pub fn error(code: &str, message: impl Into<String>) -> Self {
        ControlResponse::Error { code: code.to_string(), message: message.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SupervisorInfo {
    pub pid: u32,
    pub role: SupervisorRole,
    pub version: String,
    pub config_path: Option<String>,
}

/// Etat d'execution d'une app vu par le superviseur (la config, elle, vient du fichier).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeState {
    pub id: Uuid,
    pub status: String,
    pub error: Option<String>,
    pub active: bool,
    pub healthy: Option<bool>,
    pub cpu_percent: f32,
    pub memory_mb: f64,
    /// Arret en cours avant une relance (`restart`).
    #[serde(default)]
    pub restarting: bool,
    /// En file d'attente d'un `start_all` (palier pas encore atteint).
    #[serde(default)]
    pub queued: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcome {
    Started,
    AlreadyRunning,
    Stopping,
    NotRunning,
    Restarting,
    NotFound,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionResult {
    pub id: Uuid,
    pub outcome: ActionOutcome,
    pub message: Option<String>,
}

/// Lignes de log posterieures a `since`. `replaced` = le client avait du retard sur des lignes
/// evincees (ou effacees) : `lines` est alors le tampon complet, a partir de `first_seq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogChunk {
    pub id: Uuid,
    pub status: String,
    pub active: bool,
    pub lines: Vec<String>,
    pub first_seq: u64,
    pub next_seq: u64,
    pub replaced: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_use_a_snake_case_type_tag() {
        let id = Uuid::nil();
        let json = serde_json::to_string(&ControlRequest::Logs { id, since: 3 }).unwrap();
        assert_eq!(json, format!(r#"{{"type":"logs","id":"{id}","since":3}}"#));
        let parsed: ControlRequest = serde_json::from_str(r#"{"type":"start_all"}"#).unwrap();
        assert_eq!(parsed, ControlRequest::StartAll);
    }

    #[test]
    fn responses_round_trip() {
        let ok = ControlResponse::ok(vec![1, 2]);
        let json = serde_json::to_string(&ok).unwrap();
        assert_eq!(json, r#"{"status":"ok","data":[1,2]}"#);
        assert_eq!(serde_json::from_str::<ControlResponse>(&json).unwrap(), ok);
    }
}
