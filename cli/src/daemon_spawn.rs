use std::fs::OpenOptions;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use switchboard_core::control::SupervisorClient;

use crate::context::Context;
use crate::error::{CliError, ErrorCode};

pub const DAEMON_LOG_FILE: &str = "daemon.log";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Lance `switchboard-cli daemon run` detache du terminal (nouvelle session), sa sortie
/// dans `daemon.log`, et attend qu'il reponde sur son canal de controle.
pub fn spawn(ctx: &Context) -> Result<SupervisorClient, CliError> {
    let failed = |message: String| CliError::new(ErrorCode::DaemonStartFailed, message);
    let exe = std::env::current_exe().map_err(|e| failed(format!("cannot locate the switchboard-cli binary: {e}")))?;
    std::fs::create_dir_all(ctx.dir()).map_err(|e| CliError::io(ctx.dir().display(), e))?;
    let log_path = ctx.dir().join(DAEMON_LOG_FILE);
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| CliError::io(log_path.display(), e))?;
    let log_err = log.try_clone().map_err(|e| CliError::io(log_path.display(), e))?;

    let mut command = Command::new(exe);
    command
        .arg("daemon")
        .arg("run")
        .arg("--config-dir")
        .arg(ctx.dir())
        .env_remove("SWITCHBOARD_JSON")
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err);
    detach(&mut command);
    let mut child = command.spawn().map_err(|e| failed(format!("cannot start the daemon: {e}")))?;

    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Some(client) = SupervisorClient::discover(ctx.dir()) {
            return Ok(client);
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(failed(format!("the daemon exited during startup ({status})"))
                .with_hint(format!("see {}", log_path.display()))
                .with_details(serde_json::json!({ "log_tail": tail(&log_path, 20) })));
        }
        if Instant::now() >= deadline {
            return Err(failed("the daemon did not answer within 10s".to_string()).with_hint(format!("see {}", log_path.display())));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

fn tail(path: &std::path::Path, lines: usize) -> Vec<String> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = content.lines().collect();
    all[all.len().saturating_sub(lines)..].iter().map(|l| l.to_string()).collect()
}
