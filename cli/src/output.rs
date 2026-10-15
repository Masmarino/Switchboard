use serde_json::{json, Value};
use switchboard_core::FieldIssue;

use crate::error::CliError;

/// Resultat d'une commande : `data` pour `--json`, `human` pour le terminal.
pub struct Output {
    pub data: Value,
    pub human: String,
    pub warnings: Vec<FieldIssue>,
}

impl Output {
    pub fn new(data: Value, human: impl Into<String>) -> Self {
        Self { data, human: human.into(), warnings: Vec::new() }
    }

    pub fn with_warnings(mut self, warnings: Vec<FieldIssue>) -> Self {
        self.warnings = warnings;
        self
    }
}

pub fn print_success(command: &str, output: &Output, json_mode: bool) {
    if json_mode {
        let envelope = json!({
            "ok": true,
            "command": command,
            "data": output.data,
            "warnings": output.warnings,
        });
        println!("{}", serde_json::to_string_pretty(&envelope).unwrap_or_default());
        return;
    }
    if !output.human.is_empty() {
        println!("{}", output.human.trim_end_matches('\n'));
    }
    for warning in &output.warnings {
        eprintln!("warning: {} ({}.{})", warning.message, warning.field, warning.code);
    }
}

pub fn print_error(command: &str, error: &CliError, json_mode: bool) {
    if json_mode {
        let envelope = json!({
            "ok": false,
            "command": command,
            "error": {
                "code": error.code.as_str(),
                "message": error.message,
                "hint": error.hint,
                "details": error.details,
            },
            "exit_code": error.code.exit_code(),
        });
        println!("{}", serde_json::to_string_pretty(&envelope).unwrap_or_default());
        return;
    }
    eprintln!("error: {}", error.message);
    if let Some(Value::Array(issues)) = error.details.as_ref().and_then(|d| d.get("errors")) {
        for issue in issues {
            eprintln!(
                "  - {}: {}",
                issue.get("field").and_then(Value::as_str).unwrap_or("?"),
                issue.get("message").and_then(Value::as_str).unwrap_or("?")
            );
        }
    }
    if let Some(hint) = &error.hint {
        eprintln!("hint: {hint}");
    }
}

/// Tableau texte simple, colonnes alignees sur la plus longue valeur.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let render = |cells: Vec<String>| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<width$}", width = widths[i]))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let mut lines = vec![render(headers.iter().map(|h| h.to_string()).collect())];
    lines.extend(rows.iter().map(|r| render(r.clone())));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_aligns_columns() {
        let rendered = table(&["NAME", "STATUS"], &[vec!["api".into(), "running".into()], vec!["website".into(), "stopped".into()]]);
        assert_eq!(rendered, "NAME     STATUS\napi      running\nwebsite  stopped");
    }
}
