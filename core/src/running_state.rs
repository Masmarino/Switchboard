use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const RUNNING_FILE_NAME: &str = "running.json";

/// Process lances par le superviseur courant, pour qu'un superviseur suivant puisse arreter
/// ceux qu'un crash (ou un SIGKILL) aurait laisses orphelins.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunningState {
    pub supervisor_pid: u32,
    pub processes: Vec<RunningProcess>,
}

/// `started_at` (secondes depuis l'epoque, selon sysinfo) evite de tuer un process sans
/// rapport qui aurait recupere le meme pid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunningProcess {
    pub app_id: Uuid,
    pub pid: i32,
    pub started_at: u64,
}

impl RunningState {
    pub fn path_in(dir: &Path) -> PathBuf {
        dir.join(RUNNING_FILE_NAME)
    }

    pub fn read(dir: &Path) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(Self::path_in(dir)).ok()?).ok()
    }

    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        let path = Self::path_in(dir);
        if self.processes.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let tmp = path.with_extension(format!("json.tmp-{}", self.supervisor_pid));
        std::fs::write(&tmp, serde_json::to_string_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, &path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_empty_state_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = RunningState {
            supervisor_pid: 42,
            processes: vec![RunningProcess { app_id: Uuid::nil(), pid: 7, started_at: 1000 }],
        };
        state.write(dir.path()).unwrap();
        assert_eq!(RunningState::read(dir.path()), Some(state));
        RunningState { supervisor_pid: 42, processes: vec![] }.write(dir.path()).unwrap();
        assert!(RunningState::read(dir.path()).is_none());
    }
}
