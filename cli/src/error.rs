use std::fmt;

use serde_json::Value;
use switchboard_core::control::ControlError;
use switchboard_core::ConfigError;

/// Codes d'erreur stables (champ `error.code` de la sortie JSON) et leur code de sortie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Internal,
    Io,
    Usage,
    InvalidInput,
    AppNotFound,
    AmbiguousApp,
    ValidationFailed,
    ConfigInvalid,
    NoSupervisor,
    AlreadySupervised,
    SupervisorError,
    DaemonStartFailed,
    Timeout,
    AppFailed,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 14] = [
        ErrorCode::Internal,
        ErrorCode::Io,
        ErrorCode::Usage,
        ErrorCode::InvalidInput,
        ErrorCode::AppNotFound,
        ErrorCode::AmbiguousApp,
        ErrorCode::ValidationFailed,
        ErrorCode::ConfigInvalid,
        ErrorCode::NoSupervisor,
        ErrorCode::AlreadySupervised,
        ErrorCode::SupervisorError,
        ErrorCode::DaemonStartFailed,
        ErrorCode::Timeout,
        ErrorCode::AppFailed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Internal => "internal",
            ErrorCode::Io => "io_error",
            ErrorCode::Usage => "usage",
            ErrorCode::InvalidInput => "invalid_input",
            ErrorCode::AppNotFound => "app_not_found",
            ErrorCode::AmbiguousApp => "ambiguous_app",
            ErrorCode::ValidationFailed => "validation_failed",
            ErrorCode::ConfigInvalid => "config_invalid",
            ErrorCode::NoSupervisor => "no_supervisor",
            ErrorCode::AlreadySupervised => "already_supervised",
            ErrorCode::SupervisorError => "supervisor_error",
            ErrorCode::DaemonStartFailed => "daemon_start_failed",
            ErrorCode::Timeout => "timeout",
            ErrorCode::AppFailed => "app_failed",
        }
    }

    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::Internal | ErrorCode::Io => 1,
            ErrorCode::Usage | ErrorCode::InvalidInput => 2,
            ErrorCode::AppNotFound | ErrorCode::AmbiguousApp => 3,
            ErrorCode::ValidationFailed => 4,
            ErrorCode::ConfigInvalid => 5,
            ErrorCode::NoSupervisor
            | ErrorCode::AlreadySupervised
            | ErrorCode::SupervisorError
            | ErrorCode::DaemonStartFailed => 6,
            ErrorCode::Timeout => 7,
            ErrorCode::AppFailed => 8,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ErrorCode::Internal => "unexpected failure",
            ErrorCode::Io => "file system error (reading/writing a file)",
            ErrorCode::Usage => "invalid command line (unknown flag, missing argument)",
            ErrorCode::InvalidInput => "malformed input (JSON patch, import file, KEY=VALUE pair)",
            ErrorCode::AppNotFound => "no app matches the reference",
            ErrorCode::AmbiguousApp => "the reference matches several apps; use the full id",
            ErrorCode::ValidationFailed => "the app definition breaks a rule; see error.details",
            ErrorCode::ConfigInvalid => "apps.json exists but is not valid JSON; nothing was written",
            ErrorCode::NoSupervisor => "no app window or daemon is running (needed for logs and --no-daemon)",
            ErrorCode::AlreadySupervised => "a native app or another daemon already supervises the apps",
            ErrorCode::SupervisorError => "the supervisor refused or failed the request",
            ErrorCode::DaemonStartFailed => "the background daemon could not be started; see daemon.log",
            ErrorCode::Timeout => "the awaited state was not reached in time",
            ErrorCode::AppFailed => "the app crashed or exited while waiting for it to run",
        }
    }
}

#[derive(Debug)]
pub struct CliError {
    pub code: ErrorCode,
    pub message: String,
    pub hint: Option<String>,
    pub details: Option<Value>,
}

impl CliError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), hint: None, details: None }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Usage, message)
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }

    pub fn io(context: impl fmt::Display, error: std::io::Error) -> Self {
        Self::new(ErrorCode::Io, format!("{context}: {error}"))
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<ConfigError> for CliError {
    fn from(error: ConfigError) -> Self {
        match &error {
            ConfigError::Invalid { path, .. } => CliError::new(ErrorCode::ConfigInvalid, error.to_string())
                .with_hint(format!("fix or move {} by hand; the CLI never overwrites an unreadable config", path.display())),
            ConfigError::NoConfigDir => CliError::new(ErrorCode::Io, error.to_string())
                .with_hint("pass --config-dir or set SWITCHBOARD_CONFIG_DIR"),
            ConfigError::Io { .. } => CliError::new(ErrorCode::Io, error.to_string()),
        }
    }
}

impl From<ControlError> for CliError {
    fn from(error: ControlError) -> Self {
        match error {
            ControlError::Remote { code, message } => {
                CliError::new(ErrorCode::SupervisorError, message).with_details(serde_json::json!({ "supervisor_code": code }))
            }
            other => CliError::new(ErrorCode::SupervisorError, other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_unique_and_never_exit_zero() {
        let mut names: Vec<_> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), ErrorCode::ALL.len());
        assert!(ErrorCode::ALL.iter().all(|c| c.exit_code() != 0));
    }
}
