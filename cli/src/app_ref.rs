use serde_json::json;
use switchboard_core::{AppConfig, AppConfigList};

use crate::error::{CliError, ErrorCode};

const MIN_ID_PREFIX: usize = 4;

/// Id exact, nom (insensible a la casse, espaces ignores), puis prefixe d'id unique.
pub fn resolve<'a>(list: &'a AppConfigList, reference: &str) -> Result<&'a AppConfig, CliError> {
    let wanted = reference.trim();
    let lowered = wanted.to_lowercase();
    if let Some(app) = list.apps.iter().find(|a| a.id.to_string() == lowered) {
        return Ok(app);
    }
    let by_name: Vec<&AppConfig> = list.apps.iter().filter(|a| a.name.trim().to_lowercase() == lowered).collect();
    if let [app] = by_name.as_slice() {
        return Ok(app);
    }
    if by_name.len() > 1 {
        return Err(ambiguous(wanted, &by_name));
    }
    if wanted.len() >= MIN_ID_PREFIX && wanted.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        let by_prefix: Vec<&AppConfig> = list.apps.iter().filter(|a| a.id.to_string().starts_with(&lowered)).collect();
        match by_prefix.as_slice() {
            [app] => return Ok(app),
            [] => {}
            _ => return Err(ambiguous(wanted, &by_prefix)),
        }
    }
    let names: Vec<&str> = list.apps.iter().map(|a| a.name.as_str()).collect();
    let hint = if names.is_empty() {
        "no app is configured yet; add one with `switchboard-cli add`".to_string()
    } else {
        format!("known apps: {}", names.join(", "))
    };
    Err(CliError::new(ErrorCode::AppNotFound, format!("no app matches \"{wanted}\""))
        .with_hint(hint)
        .with_details(json!({ "reference": wanted, "known_apps": names })))
}

/// Dans l'ordre donne, sans doublons.
pub fn resolve_many(list: &AppConfigList, references: &[String]) -> Result<Vec<AppConfig>, CliError> {
    let mut apps: Vec<AppConfig> = Vec::new();
    for reference in references {
        let app = resolve(list, reference)?;
        if !apps.iter().any(|a| a.id == app.id) {
            apps.push(app.clone());
        }
    }
    Ok(apps)
}

fn ambiguous(reference: &str, matches: &[&AppConfig]) -> CliError {
    let candidates: Vec<_> = matches.iter().map(|a| json!({ "id": a.id, "name": a.name })).collect();
    CliError::new(ErrorCode::AmbiguousApp, format!("\"{reference}\" matches {} apps", matches.len()))
        .with_hint("use the full app id")
        .with_details(json!({ "reference": reference, "candidates": candidates }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn list() -> AppConfigList {
        let mut list = AppConfigList::default();
        list.add(AppConfig {
            id: Uuid::parse_str("aaaa1111-0000-4000-8000-000000000001").unwrap(),
            name: "Alume API".to_string(),
            ..Default::default()
        });
        list.add(AppConfig {
            id: Uuid::parse_str("aaaa2222-0000-4000-8000-000000000002").unwrap(),
            name: "Website".to_string(),
            ..Default::default()
        });
        list
    }

    #[test]
    fn resolves_by_id_name_and_prefix() {
        let list = list();
        assert_eq!(resolve(&list, "aaaa1111-0000-4000-8000-000000000001").unwrap().name, "Alume API");
        assert_eq!(resolve(&list, "  alume api ").unwrap().name, "Alume API");
        assert_eq!(resolve(&list, "AAAA2222").unwrap().name, "Website");
    }

    #[test]
    fn short_or_shared_prefixes_are_rejected() {
        let list = list();
        assert_eq!(resolve(&list, "aaa").unwrap_err().code, ErrorCode::AppNotFound);
        assert_eq!(resolve(&list, "aaaa").unwrap_err().code, ErrorCode::AmbiguousApp);
    }

    #[test]
    fn unknown_reference_lists_known_apps() {
        let error = resolve(&list(), "nope").unwrap_err();
        assert_eq!(error.code, ErrorCode::AppNotFound);
        assert!(error.hint.unwrap().contains("Alume API"));
    }

    #[test]
    fn resolve_many_dedupes() {
        let list = list();
        let apps = resolve_many(&list, &["website".to_string(), "Website".to_string()]).unwrap();
        assert_eq!(apps.len(), 1);
    }
}
