use std::sync::mpsc;
use std::time::Instant;

use uuid::Uuid;

use super::{status_label, Engine, PendingHandover, HANDOVER_TIMEOUT};
use crate::control::{
    ActionOutcome, ActionResult, ControlRequest, ControlResponse, LogChunk, RuntimeState, SupervisorInfo, SupervisorRole,
};
use crate::process_manager::AppStatus;

impl Engine {
    /// Requetes du CLI, recues par le `ControlServer` et traitees ici, sur le thread du moteur.
    pub(super) fn handle_control(&mut self, request: ControlRequest, reply: mpsc::Sender<ControlResponse>) {
        if self.shutting_down || self.handover.is_some() {
            let _ = reply.send(ControlResponse::error("shutting_down", "the supervisor is shutting down"));
            return;
        }
        // Une app ajoutee par le CLI juste avant `start` doit etre connue sans attendre le prochain tick.
        self.reload_config_if_changed(true);
        let role = self.control.as_ref().map(|c| c.endpoint().role);
        let response = match request {
            ControlRequest::Ping => ControlResponse::ok(SupervisorInfo {
                pid: std::process::id(),
                role: role.unwrap_or(SupervisorRole::App),
                version: env!("CARGO_PKG_VERSION").to_string(),
                config_path: self.config_path().map(|p| p.display().to_string()),
            }),
            ControlRequest::Status => ControlResponse::ok(self.runtime_states()),
            ControlRequest::Start { ids } => ControlResponse::ok(self.act_on(&ids, Self::start_app)),
            ControlRequest::Stop { ids } => ControlResponse::ok(self.act_on(&ids, Self::stop_app)),
            ControlRequest::Restart { ids } => ControlResponse::ok(self.act_on(&ids, Self::restart_app)),
            ControlRequest::StartAll => ControlResponse::ok(self.start_all()),
            ControlRequest::StopAll => ControlResponse::ok(self.stop_all_running()),
            ControlRequest::Logs { id, since } => match self.log_chunk(id, since) {
                Some(chunk) => ControlResponse::ok(chunk),
                None => ControlResponse::error("app_not_found", format!("no app with id {id}")),
            },
            ControlRequest::ClearLogs { id } => {
                if self.runtimes.contains_key(&id) {
                    self.clear_logs(id);
                    ControlResponse::ok(serde_json::Value::Null)
                } else {
                    ControlResponse::error("app_not_found", format!("no app with id {id}"))
                }
            }
            ControlRequest::Handover if role == Some(SupervisorRole::Daemon) => {
                let ids = self.stop_all_running();
                self.handover = Some(PendingHandover { reply, ids, deadline: Instant::now() + HANDOVER_TIMEOUT });
                self.finish_handover_if_ready();
                return;
            }
            ControlRequest::Shutdown if role == Some(SupervisorRole::Daemon) => {
                let stopped = self.stop_all_running();
                self.shutdown_requested = true;
                ControlResponse::ok(stopped)
            }
            ControlRequest::Handover | ControlRequest::Shutdown => ControlResponse::error(
                "not_supported",
                "the native app supervises the apps; quit it from its window instead",
            ),
        };
        let _ = reply.send(response);
    }

    /// Repond au handover une fois les process arretes (ports liberes) : le daemon ferme
    /// d'abord son canal de controle pour que l'app native puisse publier le sien.
    pub(super) fn finish_handover_if_ready(&mut self) {
        let Some(pending) = &self.handover else { return };
        if !self.handles.is_empty() && Instant::now() < pending.deadline {
            return;
        }
        let Some(pending) = self.handover.take() else { return };
        self.control = None;
        self.shutdown_requested = true;
        let _ = pending.reply.send(ControlResponse::ok(pending.ids));
    }

    fn act_on(&mut self, ids: &[Uuid], action: fn(&mut Self, Uuid) -> ActionOutcome) -> Vec<ActionResult> {
        ids.iter()
            .map(|&id| {
                let outcome = action(self, id);
                let message = match (outcome, self.runtimes.get(&id).map(|rt| &rt.status)) {
                    (ActionOutcome::Failed, Some(AppStatus::Failed(msg))) => Some(msg.clone()),
                    (ActionOutcome::NotFound, _) => Some(format!("no app with id {id}")),
                    _ => None,
                };
                ActionResult { id, outcome, message }
            })
            .collect()
    }

    fn runtime_states(&self) -> Vec<RuntimeState> {
        self.config
            .apps
            .iter()
            .map(|app| {
                let runtime = self.runtimes.get(&app.id);
                let status = runtime.map(|r| r.status.clone()).unwrap_or(AppStatus::Stopped);
                RuntimeState {
                    id: app.id,
                    status: status_label(&status).to_string(),
                    error: match status {
                        AppStatus::Failed(msg) => Some(msg),
                        _ => None,
                    },
                    active: self.handles.contains_key(&app.id),
                    healthy: runtime.and_then(|r| r.healthy),
                    cpu_percent: runtime.map(|r| r.cpu_percent).unwrap_or(0.0),
                    memory_mb: runtime.map(|r| r.memory_mb).unwrap_or(0.0),
                    restarting: self.pending_restart.contains(&app.id),
                    queued: self.queued_starts.contains(&app.id),
                }
            })
            .collect()
    }

    fn log_chunk(&self, id: Uuid, since: u64) -> Option<LogChunk> {
        let runtime = self.runtimes.get(&id)?;
        let (lines, first_seq, replaced) = runtime.lines_since(since);
        Some(LogChunk {
            id,
            status: status_label(&runtime.status).to_string(),
            active: self.handles.contains_key(&id),
            lines,
            first_seq,
            next_seq: runtime.next_seq(),
            replaced,
        })
    }
}
