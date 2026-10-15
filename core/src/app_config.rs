use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::command_rule::CommandRule;
use crate::config_file::ConfigFile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AppKind {
    Cargo,
    Npm,
    Dotnet,
    Maven,
    Python,
    Go,
    #[default]
    Raw,
}

impl AppKind {
    pub const ALL: [AppKind; 7] =
        [AppKind::Cargo, AppKind::Npm, AppKind::Dotnet, AppKind::Maven, AppKind::Python, AppKind::Go, AppKind::Raw];

    /// PascalCase name — matches serde's default enum representation and what the UI displays.
    pub fn display_name(self) -> &'static str {
        match self {
            AppKind::Cargo => "Cargo",
            AppKind::Npm => "Npm",
            AppKind::Dotnet => "Dotnet",
            AppKind::Maven => "Maven",
            AppKind::Python => "Python",
            AppKind::Go => "Go",
            AppKind::Raw => "Raw",
        }
    }

    /// Lowercase wire format used across the FFI boundary (frontends send/read this).
    pub fn ffi_str(self) -> &'static str {
        match self {
            AppKind::Cargo => "cargo",
            AppKind::Npm => "npm",
            AppKind::Dotnet => "dotnet",
            AppKind::Maven => "maven",
            AppKind::Python => "python",
            AppKind::Go => "go",
            AppKind::Raw => "raw",
        }
    }

    pub fn from_ffi_str(s: &str) -> Option<Self> {
        Some(match s {
            "cargo" => AppKind::Cargo,
            "npm" => AppKind::Npm,
            "dotnet" => AppKind::Dotnet,
            "maven" => AppKind::Maven,
            "python" => AppKind::Python,
            "go" => AppKind::Go,
            "raw" => AppKind::Raw,
            _ => return None,
        })
    }

    /// Accepte le format FFI (`cargo`) comme celui du fichier de config (`Cargo`).
    pub fn parse(s: &str) -> Option<Self> {
        Self::from_ffi_str(&s.trim().to_ascii_lowercase())
    }

    /// Ce que le champ "Commande" signifie pour ce type (cf. `process_manager::build_commands`).
    pub fn command_rule(self) -> CommandRule {
        match self {
            AppKind::Cargo => CommandRule::Ignored,
            AppKind::Npm | AppKind::Raw => CommandRule::Required,
            AppKind::Dotnet => CommandRule::Optional { default: None },
            AppKind::Maven => CommandRule::Optional { default: Some("spring-boot:run") },
            AppKind::Python => CommandRule::Optional { default: Some("main.py") },
            AppKind::Go => CommandRule::Optional { default: Some(".") },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AppConfig {
    pub id: Uuid,
    pub name: String,
    pub working_dir: PathBuf,
    pub command: String,
    pub kind: AppKind,
    /// URL locale de l'app (ex: http://localhost:3000), pour "Ouvrir dans le navigateur".
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub env_vars: Vec<(String, String)>,
    /// Relance le process en cas de crash (sortie non nulle, hors arret demande).
    #[serde(default)]
    pub auto_restart: bool,
    /// Ordre de demarrage croissant pour "Tout demarrer" — meme ordre = parallele.
    #[serde(default)]
    pub start_order: i32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportSummary {
    pub to_add: Vec<String>,
    pub to_replace: Vec<String>,
    pub invalid: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AppConfigList {
    pub apps: Vec<AppConfig>,
}

impl AppConfigList {
    pub fn add(&mut self, app: AppConfig) {
        self.apps.push(app);
    }

    pub fn remove(&mut self, id: Uuid) {
        self.apps.retain(|a| a.id != id);
    }

    pub fn update(&mut self, id: Uuid, mutate: impl FnOnce(&mut AppConfig)) {
        if let Some(app) = self.apps.iter_mut().find(|a| a.id == id) {
            mutate(app);
        }
    }

    /// Copie filtree par `ids`, avec `env_vars` vide si `include_env_vars` est faux.
    pub fn export_subset(&self, ids: &[Uuid], include_env_vars: bool) -> AppConfigList {
        let apps = self
            .apps
            .iter()
            .filter(|a| ids.contains(&a.id))
            .cloned()
            .map(|mut a| {
                if !include_env_vars {
                    a.env_vars.clear();
                }
                a
            })
            .collect();
        AppConfigList { apps }
    }

    /// Dedoublonne par nom (trim + minuscules Unicode) : match existant → remplace en
    /// place (id conserve) ; sinon ajoute avec un nouvel id (jamais celui de `incoming`).
    /// Nom vide apres trim → ignore et compte dans `invalid`.
    pub fn merge_import(&mut self, incoming: AppConfigList) -> ImportSummary {
        let mut summary = ImportSummary::default();
        for mut incoming_app in incoming.apps {
            let trimmed_name = incoming_app.name.trim().to_string();
            if trimmed_name.is_empty() {
                summary.invalid += 1;
                continue;
            }
            incoming_app.name = trimmed_name.clone();
            let existing = self
                .apps
                .iter_mut()
                .find(|a| a.name.trim().to_lowercase() == trimmed_name.to_lowercase());
            match existing {
                Some(existing_app) => {
                    let existing_id = existing_app.id;
                    *existing_app = AppConfig { id: existing_id, ..incoming_app };
                    summary.to_replace.push(trimmed_name);
                }
                None => {
                    summary.to_add.push(trimmed_name.clone());
                    self.apps.push(AppConfig { id: Uuid::new_v4(), ..incoming_app });
                }
            }
        }
        summary
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }

    pub fn config_path() -> Option<PathBuf> {
        crate::paths::config_dir().map(|dir| dir.join(crate::paths::CONFIG_FILE_NAME))
    }

    /// Liste vide au premier lancement — pas de projet pre-rempli. Un fichier illisible
    /// donne aussi une liste vide : utiliser `ConfigFile::load` pour distinguer les cas.
    pub fn load_or_default() -> Self {
        ConfigFile::default_location().and_then(|file| file.load()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        ConfigFile::default_location()
            .and_then(|file| file.save(self))
            .map_err(std::io::Error::other)
    }

    pub fn find(&self, id: Uuid) -> Option<&AppConfig> {
        self.apps.iter().find(|a| a.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_appends_a_new_app() {
        let mut list = AppConfigList::default();
        assert_eq!(list.apps.len(), 0);
        list.add(AppConfig {
            id: Uuid::new_v4(),
            name: "Custom".to_string(),
            working_dir: PathBuf::from("/repo/custom"),
            command: "run".to_string(),
            kind: AppKind::Raw,
            url: None,
            env_vars: Vec::new(),
            auto_restart: false,
            start_order: 0,
        });
        assert_eq!(list.apps.len(), 1);
        assert_eq!(list.apps[0].name, "Custom");
    }

    #[test]
    fn remove_drops_app_by_id() {
        let mut list = AppConfigList::default();
        let app = AppConfig { name: "X".to_string(), ..Default::default() };
        let id = app.id;
        list.add(app);
        list.remove(id);
        assert!(list.apps.is_empty());
    }

    #[test]
    fn update_mutates_matching_app() {
        let mut list = AppConfigList::default();
        let app = AppConfig { name: "X".to_string(), ..Default::default() };
        let id = app.id;
        list.add(app);
        list.update(id, |app| {
            app.auto_restart = true;
            app.env_vars.push(("FOO".to_string(), "bar".to_string()));
        });
        assert!(list.apps[0].auto_restart);
        assert_eq!(list.apps[0].env_vars, vec![("FOO".to_string(), "bar".to_string())]);
    }

    #[test]
    fn json_round_trip_preserves_apps() {
        let mut list = AppConfigList::default();
        list.add(AppConfig {
            name: "X".to_string(),
            url: Some("http://localhost:3000".to_string()),
            ..Default::default()
        });
        let json = list.to_json().expect("serialize");
        let parsed = AppConfigList::from_json(&json).expect("deserialize");
        assert_eq!(parsed.apps.len(), list.apps.len());
        assert_eq!(parsed.apps[0].id, list.apps[0].id);
        assert_eq!(parsed.apps[0].name, list.apps[0].name);
        assert_eq!(parsed.apps[0].url, list.apps[0].url);
    }

    #[test]
    fn old_json_without_new_fields_still_parses() {
        let old_json = r#"{"apps":[{"id":"550e8400-e29b-41d4-a716-446655440000","name":"X","working_dir":"/r","command":"","kind":"Cargo"}]}"#;
        let list = AppConfigList::from_json(old_json).expect("should parse with defaults");
        assert_eq!(list.apps[0].url, None);
        assert_eq!(list.apps[0].start_order, 0);
        assert!(!list.apps[0].auto_restart);
        assert!(list.apps[0].env_vars.is_empty());
    }

    #[test]
    fn export_subset_filters_by_id_and_strips_env_vars_when_excluded() {
        let mut list = AppConfigList::default();
        let a = AppConfig { id: Uuid::new_v4(), name: "A".to_string(), env_vars: vec![("K".to_string(), "V".to_string())], ..Default::default() };
        let a_id = a.id;
        let b = AppConfig { id: Uuid::new_v4(), name: "B".to_string(), ..Default::default() };
        list.add(a);
        list.add(b);

        let subset = list.export_subset(&[a_id], false);
        assert_eq!(subset.apps.len(), 1);
        assert_eq!(subset.apps[0].name, "A");
        assert!(subset.apps[0].env_vars.is_empty());
    }

    #[test]
    fn export_subset_keeps_env_vars_when_included() {
        let mut list = AppConfigList::default();
        let a = AppConfig { name: "A".to_string(), env_vars: vec![("K".to_string(), "V".to_string())], ..Default::default() };
        let a_id = a.id;
        list.add(a);

        let subset = list.export_subset(&[a_id], true);
        assert_eq!(subset.apps[0].env_vars, vec![("K".to_string(), "V".to_string())]);
    }

    #[test]
    fn merge_import_replaces_existing_app_by_name_case_insensitive_and_keeps_its_id() {
        let mut list = AppConfigList::default();
        let existing = AppConfig { id: Uuid::new_v4(), name: "Alume API".to_string(), command: "old".to_string(), ..Default::default() };
        let existing_id = existing.id;
        list.add(existing);

        let mut incoming = AppConfigList::default();
        incoming.add(AppConfig { id: Uuid::new_v4(), name: " alume api ".to_string(), command: "new".to_string(), ..Default::default() });

        let summary = list.merge_import(incoming);

        assert_eq!(summary.to_replace, vec!["alume api".to_string()]);
        assert!(summary.to_add.is_empty());
        assert_eq!(list.apps.len(), 1);
        assert_eq!(list.apps[0].id, existing_id);
        assert_eq!(list.apps[0].command, "new");
    }

    #[test]
    fn merge_import_adds_new_app_with_a_fresh_id() {
        let mut list = AppConfigList::default();
        let mut incoming = AppConfigList::default();
        let incoming_app = AppConfig { name: "Brand New".to_string(), ..Default::default() };
        let incoming_id = incoming_app.id;
        incoming.add(incoming_app);

        let summary = list.merge_import(incoming);

        assert_eq!(summary.to_add, vec!["Brand New".to_string()]);
        assert!(summary.to_replace.is_empty());
        assert_eq!(list.apps.len(), 1);
        assert_ne!(list.apps[0].id, incoming_id);
    }

    #[test]
    fn merge_import_counts_blank_name_as_invalid() {
        let mut list = AppConfigList::default();
        let mut incoming = AppConfigList::default();
        incoming.add(AppConfig { name: "   ".to_string(), ..Default::default() });

        let summary = list.merge_import(incoming);

        assert_eq!(summary.invalid, 1);
        assert!(list.apps.is_empty());
    }

    #[test]
    fn kind_parse_accepts_ffi_and_config_spellings() {
        assert_eq!(AppKind::parse("npm"), Some(AppKind::Npm));
        assert_eq!(AppKind::parse("Npm"), Some(AppKind::Npm));
        assert_eq!(AppKind::parse(" DOTNET "), Some(AppKind::Dotnet));
        assert_eq!(AppKind::parse("node"), None);
    }

    #[test]
    fn every_kind_round_trips_through_its_ffi_string() {
        for kind in AppKind::ALL {
            assert_eq!(AppKind::parse(kind.ffi_str()), Some(kind));
            assert_eq!(AppKind::parse(kind.display_name()), Some(kind));
        }
    }

    #[test]
    fn merge_import_matches_names_using_unicode_case_folding() {
        // "É" (U+00C9) lowercases to "é" (U+00E9) only via full Unicode lowercasing;
        // a naive `.to_ascii_lowercase()` would leave it untouched and miss the match.
        let mut list = AppConfigList::default();
        let existing = AppConfig { id: Uuid::new_v4(), name: "Éditeur".to_string(), command: "old".to_string(), ..Default::default() };
        let existing_id = existing.id;
        list.add(existing);

        let mut incoming = AppConfigList::default();
        incoming.add(AppConfig { id: Uuid::new_v4(), name: "éditeur".to_string(), command: "new".to_string(), ..Default::default() });

        let summary = list.merge_import(incoming);

        assert_eq!(summary.to_replace, vec!["éditeur".to_string()]);
        assert!(summary.to_add.is_empty());
        assert_eq!(list.apps.len(), 1);
        assert_eq!(list.apps[0].id, existing_id);
        assert_eq!(list.apps[0].command, "new");
    }
}
