use serde::Serialize;
use uuid::Uuid;

use crate::app_config::{AppConfigList, AppKind};
use crate::app_draft::AppDraft;
use crate::command_rule::CommandRule;
use crate::process_manager::split_command;

/// Borne haute du selecteur "Ordre de demarrage" des formulaires natifs.
pub const START_ORDER_MAX: i32 = 99;

/// Probleme sur un champ : `code` est stable (machine), `message` est pour l'humain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldIssue {
    pub field: &'static str,
    pub code: &'static str,
    pub message: String,
}

impl FieldIssue {
    fn new(field: &'static str, code: &'static str, message: impl Into<String>) -> Self {
        Self { field, code, message: message.into() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DraftReport {
    pub errors: Vec<FieldIssue>,
    pub warnings: Vec<FieldIssue>,
}

impl DraftReport {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Regles des formulaires natifs (nom et dossier obligatoires, ordre 0-99) plus celles que
/// le README annonce sans que l'UI les impose (commande obligatoire pour Npm/Raw, URL http).
/// `editing` exclut l'app modifiee du controle d'unicite du nom. Attend un draft normalise.
pub fn validate_draft(draft: &AppDraft, existing: &AppConfigList, editing: Option<Uuid>) -> DraftReport {
    let mut report = DraftReport::default();
    let errors = &mut report.errors;
    let warnings = &mut report.warnings;

    if draft.name.is_empty() {
        errors.push(FieldIssue::new("name", "required", "name must not be empty"));
    } else {
        let lowered = draft.name.to_lowercase();
        if existing.apps.iter().any(|a| Some(a.id) != editing && a.name.trim().to_lowercase() == lowered) {
            errors.push(FieldIssue::new(
                "name",
                "duplicate",
                format!("an app named \"{}\" already exists (names are case-insensitive)", draft.name),
            ));
        }
    }

    if draft.working_dir.as_os_str().is_empty() {
        errors.push(FieldIssue::new("working_dir", "required", "working_dir must not be empty"));
    } else if !draft.working_dir.is_absolute() {
        errors.push(FieldIssue::new("working_dir", "not_absolute", "working_dir must be an absolute path"));
    } else if !draft.working_dir.is_dir() {
        warnings.push(FieldIssue::new(
            "working_dir",
            "missing",
            format!("{} does not exist (yet): starting the app will fail until it does", draft.working_dir.display()),
        ));
    }

    match draft.kind.command_rule() {
        CommandRule::Required if draft.command.is_empty() => errors.push(FieldIssue::new(
            "command",
            "required",
            format!("command is required for kind \"{}\"", draft.kind.ffi_str()),
        )),
        CommandRule::Ignored if !draft.command.is_empty() => warnings.push(FieldIssue::new(
            "command",
            "ignored",
            format!("command is ignored for kind \"{}\"", draft.kind.ffi_str()),
        )),
        CommandRule::Required | CommandRule::Optional { .. }
            if draft.kind != AppKind::Raw && split_command(&draft.command).unbalanced =>
        {
            errors.push(FieldIssue::new("command", "unbalanced_quotes", "command has an unclosed quote"))
        }
        _ => {}
    }

    if let Some(url) = &draft.url {
        let valid = ["http://", "https://"]
            .iter()
            .any(|scheme| url.len() > scheme.len() && url.to_ascii_lowercase().starts_with(scheme));
        if !valid || url.chars().any(char::is_whitespace) {
            errors.push(FieldIssue::new("url", "invalid", "url must be an http:// or https:// URL without spaces"));
        }
    }

    let mut seen = std::collections::HashSet::new();
    for (key, value) in &draft.env_vars {
        if key.is_empty() || key.contains('=') || key.contains('\0') || key.chars().any(char::is_whitespace) {
            errors.push(FieldIssue::new(
                "env_vars",
                "invalid_key",
                format!("invalid environment variable name \"{key}\" (no spaces, '=' or NUL)"),
            ));
        } else if !seen.insert(key.as_str()) {
            errors.push(FieldIssue::new("env_vars", "duplicate_key", format!("environment variable \"{key}\" is set twice")));
        }
        if value.contains('\0') {
            errors.push(FieldIssue::new("env_vars", "invalid_value", format!("value of \"{key}\" contains a NUL byte")));
        }
    }

    if !(0..=START_ORDER_MAX).contains(&draft.start_order) {
        errors.push(FieldIssue::new(
            "start_order",
            "out_of_range",
            format!("start_order must be between 0 and {START_ORDER_MAX}"),
        ));
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::AppConfig;
    use std::path::PathBuf;

    fn valid_draft() -> AppDraft {
        AppDraft {
            name: "Api".to_string(),
            working_dir: std::env::temp_dir(),
            kind: AppKind::Npm,
            command: "start".to_string(),
            ..Default::default()
        }
    }

    fn codes(report: &DraftReport) -> Vec<(&'static str, &'static str)> {
        report.errors.iter().map(|i| (i.field, i.code)).collect()
    }

    #[test]
    fn a_complete_draft_is_valid() {
        let report = validate_draft(&valid_draft(), &AppConfigList::default(), None);
        assert!(report.is_valid(), "{report:?}");
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn name_and_dir_are_required() {
        let draft = AppDraft { name: String::new(), working_dir: PathBuf::new(), ..valid_draft() };
        let report = validate_draft(&draft, &AppConfigList::default(), None);
        assert_eq!(codes(&report), vec![("name", "required"), ("working_dir", "required")]);
    }

    #[test]
    fn relative_dir_is_rejected() {
        let draft = AppDraft { working_dir: PathBuf::from("relative/path"), ..valid_draft() };
        assert_eq!(codes(&validate_draft(&draft, &AppConfigList::default(), None)), vec![("working_dir", "not_absolute")]);
    }

    #[test]
    fn missing_dir_is_only_a_warning() {
        let draft = AppDraft { working_dir: std::env::temp_dir().join("switchboard-does-not-exist-42"), ..valid_draft() };
        let report = validate_draft(&draft, &AppConfigList::default(), None);
        assert!(report.is_valid());
        assert_eq!(report.warnings[0].code, "missing");
    }

    #[test]
    fn duplicate_name_is_rejected_except_for_the_app_being_edited() {
        let mut list = AppConfigList::default();
        let existing = AppConfig { name: "API".to_string(), ..Default::default() };
        let existing_id = existing.id;
        list.add(existing);

        assert_eq!(codes(&validate_draft(&valid_draft(), &list, None)), vec![("name", "duplicate")]);
        assert!(validate_draft(&valid_draft(), &list, Some(existing_id)).is_valid());
    }

    #[test]
    fn command_rules_follow_the_kind() {
        let npm = AppDraft { command: String::new(), ..valid_draft() };
        assert_eq!(codes(&validate_draft(&npm, &AppConfigList::default(), None)), vec![("command", "required")]);

        let raw = AppDraft { kind: AppKind::Raw, command: String::new(), ..valid_draft() };
        assert_eq!(codes(&validate_draft(&raw, &AppConfigList::default(), None)), vec![("command", "required")]);

        let python = AppDraft { kind: AppKind::Python, command: String::new(), ..valid_draft() };
        assert!(validate_draft(&python, &AppConfigList::default(), None).is_valid());

        let cargo = AppDraft { kind: AppKind::Cargo, command: "run".to_string(), ..valid_draft() };
        let report = validate_draft(&cargo, &AppConfigList::default(), None);
        assert!(report.is_valid());
        assert_eq!(report.warnings[0].code, "ignored");
    }

    #[test]
    fn unclosed_quotes_are_rejected_except_for_raw_commands() {
        let npm = AppDraft { command: r#"run "dev"#.to_string(), ..valid_draft() };
        assert_eq!(codes(&validate_draft(&npm, &AppConfigList::default(), None)), vec![("command", "unbalanced_quotes")]);
        // Raw commands go to `sh -c` untouched: the shell reports its own syntax errors.
        let raw = AppDraft { kind: AppKind::Raw, command: r#"echo "hi"#.to_string(), ..valid_draft() };
        assert!(validate_draft(&raw, &AppConfigList::default(), None).is_valid());
    }

    #[test]
    fn url_must_be_http() {
        for bad in ["localhost:3000", "ftp://x", "http://", "http://a b"] {
            let draft = AppDraft { url: Some(bad.to_string()), ..valid_draft() };
            assert_eq!(codes(&validate_draft(&draft, &AppConfigList::default(), None)), vec![("url", "invalid")], "{bad}");
        }
        let ok = AppDraft { url: Some("HTTPS://localhost:3000/health".to_string()), ..valid_draft() };
        assert!(validate_draft(&ok, &AppConfigList::default(), None).is_valid());
    }

    #[test]
    fn env_var_keys_are_checked() {
        let draft = AppDraft {
            env_vars: vec![
                ("A=B".to_string(), "x".to_string()),
                ("PORT".to_string(), "1".to_string()),
                ("PORT".to_string(), "2".to_string()),
            ],
            ..valid_draft()
        };
        assert_eq!(
            codes(&validate_draft(&draft, &AppConfigList::default(), None)),
            vec![("env_vars", "invalid_key"), ("env_vars", "duplicate_key")]
        );
    }

    #[test]
    fn start_order_is_bounded() {
        for order in [-1, START_ORDER_MAX + 1] {
            let draft = AppDraft { start_order: order, ..valid_draft() };
            assert_eq!(codes(&validate_draft(&draft, &AppConfigList::default(), None)), vec![("start_order", "out_of_range")]);
        }
    }
}
