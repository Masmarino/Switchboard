use crate::app_config::{AppConfig, AppKind};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum AppStatus {
    Stopped,
    Building,
    Running,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

fn command_words_or(command: &str, defaults: &[&str]) -> Vec<String> {
    if command.trim().is_empty() {
        defaults.iter().map(|s| s.to_string()).collect()
    } else {
        split_command(command).words
    }
}

/// Resultat de [`split_command`] : `unbalanced` signale un guillemet jamais referme
/// (le reste de la ligne est alors pris tel quel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitCommand {
    pub words: Vec<String>,
    pub unbalanced: bool,
}

/// Decoupe comme un shell POSIX (guillemets simples et doubles, antislash) : sans ca,
/// `-c "import x"` etait coupe au milieu des guillemets.
pub fn split_command(command: &str) -> SplitCommand {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut chars = command.chars().peekable();
    let mut unbalanced = false;
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => {
                            unbalanced = true;
                            break;
                        }
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.peek() {
                            Some(&next) if matches!(next, '"' | '\\' | '$' | '`') => {
                                current.push(next);
                                chars.next();
                            }
                            _ => current.push('\\'),
                        },
                        Some(c) => current.push(c),
                        None => {
                            unbalanced = true;
                            break;
                        }
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some(next) => current.push(next),
                    None => current.push('\\'),
                }
            }
            c => {
                in_word = true;
                current.push(c);
            }
        }
    }
    if in_word {
        words.push(current);
    }
    SplitCommand { words, unbalanced }
}

pub fn build_commands(config: &AppConfig) -> Vec<CommandSpec> {
    match config.kind {
        AppKind::Cargo => vec![
            CommandSpec { program: "cargo".to_string(), args: vec!["build".to_string()] },
            CommandSpec { program: "cargo".to_string(), args: vec!["run".to_string()] },
        ],
        AppKind::Npm => vec![CommandSpec {
            program: "npm".to_string(),
            args: split_command(&config.command).words,
        }],
        AppKind::Dotnet => vec![CommandSpec {
            program: "dotnet".to_string(),
            args: ["run".to_string()]
                .into_iter()
                .chain(split_command(&config.command).words)
                .collect(),
        }],
        AppKind::Maven => vec![CommandSpec {
            program: "mvn".to_string(),
            args: command_words_or(&config.command, &["spring-boot:run"]),
        }],
        AppKind::Python => vec![CommandSpec {
            program: "python3".to_string(),
            args: command_words_or(&config.command, &["main.py"]),
        }],
        AppKind::Go => vec![CommandSpec {
            program: "go".to_string(),
            args: ["run".to_string()]
                .into_iter()
                .chain(command_words_or(&config.command, &["."]))
                .collect(),
        }],
        AppKind::Raw => vec![CommandSpec {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), config.command.clone()],
        }],
    }
}

pub fn resolve_final_status(result: &std::io::Result<i32>, stop_requested: bool) -> AppStatus {
    match result {
        Ok(_) if stop_requested => AppStatus::Stopped,
        Ok(0) => AppStatus::Stopped,
        Ok(code) => AppStatus::Failed(format!("exit code {code}")),
        Err(e) => AppStatus::Failed(format!("process error: {e}")),
    }
}

/// Crash = ni arret volontaire, ni sortie propre (code 0) — candidat au redemarrage auto.
fn is_crash(result: &std::io::Result<i32>, stop_requested: bool) -> bool {
    if stop_requested {
        return false;
    }
    !matches!(result, Ok(0))
}

const AUTO_RESTART_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use uuid::Uuid;

use crate::log_stream::{stream_lines, Event};

pub struct RunningHandle {
    pub pgid: Arc<Mutex<Option<i32>>>,
    pub stop_requested: Arc<AtomicBool>,
}

impl Default for RunningHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl RunningHandle {
    pub fn new() -> Self {
        Self {
            pgid: Arc::new(Mutex::new(None)),
            stop_requested: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Unix : vrai group id (negation pour cibler tout le groupe). Windows : PID racine,
/// `taskkill /T` descend l'arbre.
#[cfg(unix)]
pub fn kill_process_group(pgid: i32) {
    unsafe {
        libc::kill(-pgid, libc::SIGTERM);
    }
}

#[cfg(windows)]
pub fn kill_process_group(pid: i32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
}

/// Shell utilise pour lancer les apps (Unix) : celui de l'utilisateur, zsh a defaut.
#[cfg(unix)]
pub fn login_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

/// Un shell interactif ignore SIGTERM : tant qu'il charge encore `.zshrc` (nvm peut prendre
/// plusieurs secondes), seul SIGHUP l'arrete avant qu'il ne lance la commande.
#[cfg(unix)]
pub fn hang_up(pid: i32) {
    unsafe {
        libc::kill(pid, libc::SIGHUP);
    }
}

/// Dernier recours pour un process qui ignore SIGTERM.
#[cfg(unix)]
pub fn force_kill_process_group(pgid: i32) {
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

#[cfg(windows)]
pub fn force_kill_process_group(pid: i32) {
    kill_process_group(pid);
}

/// Simple-quote une valeur pour une commande shell (double les simple-quotes internes).
#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Les apps GUI macOS heritent d'un PATH minimal (pas de `~/.cargo/bin`, nvm, homebrew) —
/// un shell de connexion interactif (`-i -l`) source `.zprofile`/`.zshrc` avant la commande,
/// sinon `cargo`/`npm`/etc. echouent avec "No such file or directory" meme installes.
/// `setsid` plutot que `setpgid` : sans terminal de controle, un shell `-i` lance depuis un
/// terminal (frontend Linux, daemon au premier plan) ne peut pas etre suspendu par SIGTTOU.
/// Le groupe cree a le meme id que le process, donc `kill(-pgid)` vise toujours tout l'arbre.
#[cfg(unix)]
fn spawn_process(
    spec: &CommandSpec,
    working_dir: &Path,
    env_vars: &[(String, String)],
) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;

    let shell = login_shell();
    let full_command = std::iter::once(spec.program.as_str())
        .chain(spec.args.iter().map(|s| s.as_str()))
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ");

    let mut cmd = std::process::Command::new(&shell);
    cmd.args(["-i", "-l", "-c", &full_command])
        .current_dir(working_dir)
        .envs(env_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn()
}

/// Entoure de guillemets un argument qui contient un espace, pour que `cmd.exe /C` le garde entier.
#[cfg(windows)]
fn cmd_quote(value: &str) -> String {
    if !value.is_empty() && !value.chars().any(|c| c.is_whitespace() || c == '"') {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('"', "\\\""))
}

/// `Command::new` ne sait pas executer les `.cmd`/`.bat` (npm.cmd, etc.) directement —
/// `cmd.exe /C` resout le PATH lui-meme, .cmd/.bat compris.
#[cfg(windows)]
fn spawn_process(
    spec: &CommandSpec,
    working_dir: &Path,
    env_vars: &[(String, String)],
) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    let full_command = std::iter::once(spec.program.as_str())
        .chain(spec.args.iter().map(|s| s.as_str()))
        .map(cmd_quote)
        .collect::<Vec<_>>()
        .join(" ");

    let mut cmd = std::process::Command::new("cmd.exe");
    cmd.args(["/C", &full_command])
        .current_dir(working_dir)
        .envs(env_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(CREATE_NEW_PROCESS_GROUP);
    cmd.spawn()
}

fn run_one_command(
    spec: &CommandSpec,
    working_dir: &Path,
    env_vars: &[(String, String)],
    app_id: Uuid,
    pgid_slot: &Arc<Mutex<Option<i32>>>,
    tx: &mpsc::Sender<Event>,
) -> std::io::Result<i32> {
    tx.send(Event::Log(
        app_id,
        format!("[devtool] $ {} {}", spec.program, spec.args.join(" ")),
    ))
    .ok();

    let mut child = spawn_process(spec, working_dir, env_vars)?;
    let pgid = child.id() as i32;
    *pgid_slot.lock().unwrap() = Some(pgid);

    let out_handle = child.stdout.take().map(|s| {
        let tx = tx.clone();
        std::thread::spawn(move || stream_lines(s, app_id, tx))
    });
    let err_handle = child.stderr.take().map(|s| {
        let tx = tx.clone();
        std::thread::spawn(move || stream_lines(s, app_id, tx))
    });

    let status = child.wait()?;
    if let Some(h) = out_handle {
        h.join().ok();
    }
    if let Some(h) = err_handle {
        h.join().ok();
    }
    *pgid_slot.lock().unwrap() = None;
    Ok(status.code().unwrap_or(-1))
}

/// Tourne jusqu'a un arret volontaire, une sortie propre, ou un crash si auto_restart est faux.
fn run_with_auto_restart(
    config: &AppConfig,
    spec: &CommandSpec,
    pgid_slot: &Arc<Mutex<Option<i32>>>,
    stop_requested: &Arc<AtomicBool>,
    tx: &mpsc::Sender<Event>,
) {
    loop {
        tx.send(Event::StatusChanged(config.id, AppStatus::Running)).ok();
        let result = run_one_command(spec, &config.working_dir, &config.env_vars, config.id, pgid_slot, tx);
        let stopped = stop_requested.load(Ordering::SeqCst);

        if config.auto_restart && is_crash(&result, stopped) {
            tx.send(Event::Log(
                config.id,
                "[devtool] crash detecte, redemarrage dans 1s...".to_string(),
            ))
            .ok();
            std::thread::sleep(AUTO_RESTART_DELAY);
            if stop_requested.load(Ordering::SeqCst) {
                tx.send(Event::StatusChanged(config.id, AppStatus::Stopped)).ok();
                return;
            }
            continue;
        }

        tx.send(Event::StatusChanged(config.id, resolve_final_status(&result, stopped))).ok();
        return;
    }
}

pub fn run_app_thread(
    config: AppConfig,
    pgid_slot: Arc<Mutex<Option<i32>>>,
    stop_requested: Arc<AtomicBool>,
    tx: mpsc::Sender<Event>,
) {
    let commands = build_commands(&config);

    if config.kind == AppKind::Cargo {
        tx.send(Event::StatusChanged(config.id, AppStatus::Building)).ok();
        let build_result =
            run_one_command(&commands[0], &config.working_dir, &config.env_vars, config.id, &pgid_slot, &tx);

        if stop_requested.load(Ordering::SeqCst) {
            tx.send(Event::StatusChanged(
                config.id,
                resolve_final_status(&build_result, true),
            ))
            .ok();
            return;
        }
        if !matches!(build_result, Ok(0)) {
            tx.send(Event::StatusChanged(
                config.id,
                resolve_final_status(&build_result, false),
            ))
            .ok();
            return;
        }

        run_with_auto_restart(&config, &commands[1], &pgid_slot, &stop_requested, &tx);
    } else {
        run_with_auto_restart(&config, &commands[0], &pgid_slot, &stop_requested, &tx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn config_with(kind: AppKind, command: &str) -> AppConfig {
        AppConfig {
            id: Uuid::new_v4(),
            name: "test".to_string(),
            working_dir: PathBuf::from("/repo/test"),
            command: command.to_string(),
            kind,
            url: None,
            env_vars: Vec::new(),
            auto_restart: false,
            start_order: 0,
        }
    }

    #[test]
    fn cargo_builds_then_runs() {
        let config = config_with(AppKind::Cargo, "");
        let commands = build_commands(&config);
        assert_eq!(commands, vec![
            CommandSpec { program: "cargo".to_string(), args: vec!["build".to_string()] },
            CommandSpec { program: "cargo".to_string(), args: vec!["run".to_string()] },
        ]);
    }

    #[test]
    fn npm_runs_configured_command() {
        let config = config_with(AppKind::Npm, "start");
        let commands = build_commands(&config);
        assert_eq!(commands, vec![CommandSpec {
            program: "npm".to_string(),
            args: vec!["start".to_string()],
        }]);
    }

    #[test]
    fn dotnet_runs_with_extra_args() {
        let config = config_with(AppKind::Dotnet, "--project Api.csproj");
        let commands = build_commands(&config);
        assert_eq!(commands, vec![CommandSpec {
            program: "dotnet".to_string(),
            args: vec!["run".to_string(), "--project".to_string(), "Api.csproj".to_string()],
        }]);
    }

    #[test]
    fn dotnet_defaults_to_plain_run() {
        let config = config_with(AppKind::Dotnet, "");
        let commands = build_commands(&config);
        assert_eq!(commands, vec![CommandSpec {
            program: "dotnet".to_string(),
            args: vec!["run".to_string()],
        }]);
    }

    #[test]
    fn maven_defaults_to_spring_boot_run() {
        let config = config_with(AppKind::Maven, "");
        let commands = build_commands(&config);
        assert_eq!(commands, vec![CommandSpec {
            program: "mvn".to_string(),
            args: vec!["spring-boot:run".to_string()],
        }]);
    }

    #[test]
    fn maven_runs_configured_goal() {
        let config = config_with(AppKind::Maven, "quarkus:dev");
        let commands = build_commands(&config);
        assert_eq!(commands, vec![CommandSpec {
            program: "mvn".to_string(),
            args: vec!["quarkus:dev".to_string()],
        }]);
    }

    #[test]
    fn python_defaults_to_main_py() {
        let config = config_with(AppKind::Python, "");
        let commands = build_commands(&config);
        assert_eq!(commands, vec![CommandSpec {
            program: "python3".to_string(),
            args: vec!["main.py".to_string()],
        }]);
    }

    #[test]
    fn python_runs_configured_module() {
        let config = config_with(AppKind::Python, "-m flask run");
        let commands = build_commands(&config);
        assert_eq!(commands, vec![CommandSpec {
            program: "python3".to_string(),
            args: vec!["-m".to_string(), "flask".to_string(), "run".to_string()],
        }]);
    }

    #[test]
    fn go_defaults_to_run_dot() {
        let config = config_with(AppKind::Go, "");
        let commands = build_commands(&config);
        assert_eq!(commands, vec![CommandSpec {
            program: "go".to_string(),
            args: vec!["run".to_string(), ".".to_string()],
        }]);
    }

    #[test]
    fn go_runs_configured_target() {
        let config = config_with(AppKind::Go, "./cmd/server");
        let commands = build_commands(&config);
        assert_eq!(commands, vec![CommandSpec {
            program: "go".to_string(),
            args: vec!["run".to_string(), "./cmd/server".to_string()],
        }]);
    }

    #[test]
    fn raw_runs_via_shell() {
        let config = config_with(AppKind::Raw, "./run.sh --flag");
        let commands = build_commands(&config);
        assert_eq!(commands, vec![CommandSpec {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "./run.sh --flag".to_string()],
        }]);
    }

    #[test]
    fn python_keeps_quoted_arguments_whole() {
        let config = config_with(AppKind::Python, r#"-c "import time; time.sleep(1)" 'a b' c\ d"#);
        let commands = build_commands(&config);
        assert_eq!(commands[0].args, vec!["-c", "import time; time.sleep(1)", "a b", "c d"]);
    }

    #[test]
    fn split_command_handles_escapes_and_reports_unbalanced_quotes() {
        assert_eq!(split_command(r#"a "b \"c\" \$d" ''"#).words, vec!["a", r#"b "c" $d"#, ""]);
        assert!(!split_command("run dev").unbalanced);
        let broken = split_command(r#"run "dev"#);
        assert!(broken.unbalanced);
        assert_eq!(broken.words, vec!["run", "dev"]);
    }

    #[test]
    fn resolve_status_stopped_on_clean_exit() {
        let status = resolve_final_status(&Ok(0), false);
        assert_eq!(status, AppStatus::Stopped);
    }

    #[test]
    fn resolve_status_stopped_when_stop_was_requested_even_with_nonzero_exit() {
        let status = resolve_final_status(&Ok(15), true);
        assert_eq!(status, AppStatus::Stopped);
    }

    #[test]
    fn resolve_status_failed_on_nonzero_exit_without_stop_request() {
        let status = resolve_final_status(&Ok(1), false);
        assert_eq!(status, AppStatus::Failed("exit code 1".to_string()));
    }

    #[test]
    fn resolve_status_failed_on_spawn_error() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let status = resolve_final_status(&Err(err), false);
        assert!(matches!(status, AppStatus::Failed(msg) if msg.contains("no such file")));
    }

    #[test]
    fn crash_detected_on_nonzero_exit_without_stop() {
        assert!(is_crash(&Ok(1), false));
    }

    #[test]
    fn crash_not_detected_on_clean_exit() {
        assert!(!is_crash(&Ok(0), false));
    }

    #[test]
    fn crash_not_detected_when_stop_was_requested() {
        assert!(!is_crash(&Ok(1), true));
    }
}
