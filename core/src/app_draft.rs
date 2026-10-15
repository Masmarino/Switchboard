use std::path::PathBuf;

use uuid::Uuid;

use crate::app_config::{AppConfig, AppKind};

/// Champs editables d'une app, partages entre creation et modification.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppDraft {
    pub name: String,
    pub working_dir: PathBuf,
    pub kind: AppKind,
    pub command: String,
    pub url: Option<String>,
    pub env_vars: Vec<(String, String)>,
    pub auto_restart: bool,
    pub start_order: i32,
}

impl AppDraft {
    pub fn from_config(app: &AppConfig) -> Self {
        Self {
            name: app.name.clone(),
            working_dir: app.working_dir.clone(),
            kind: app.kind,
            command: app.command.clone(),
            url: app.url.clone(),
            env_vars: app.env_vars.clone(),
            auto_restart: app.auto_restart,
            start_order: app.start_order,
        }
    }

    pub fn into_config(self, id: Uuid) -> AppConfig {
        AppConfig {
            id,
            name: self.name,
            working_dir: self.working_dir,
            command: self.command,
            kind: self.kind,
            url: self.url,
            env_vars: self.env_vars,
            auto_restart: self.auto_restart,
            start_order: self.start_order,
        }
    }

    /// Meme nettoyage que les formulaires natifs : espaces retires, URL vide = pas d'URL,
    /// variables sans cle ignorees.
    pub fn normalize(&mut self) {
        self.name = self.name.trim().to_string();
        let dir = self.working_dir.to_string_lossy();
        if dir.trim() != dir {
            self.working_dir = PathBuf::from(dir.trim());
        }
        self.command = self.command.trim().to_string();
        self.url = self.url.take().map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        self.env_vars = std::mem::take(&mut self.env_vars)
            .into_iter()
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .filter(|(k, _)| !k.is_empty())
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_like_the_native_forms() {
        let mut draft = AppDraft {
            name: "  Api ".to_string(),
            working_dir: PathBuf::from(" /repo/api "),
            command: " run dev ".to_string(),
            url: Some("   ".to_string()),
            env_vars: vec![(" PORT ".to_string(), " 3000 ".to_string()), ("  ".to_string(), "x".to_string())],
            ..Default::default()
        };
        draft.normalize();
        assert_eq!(draft.name, "Api");
        assert_eq!(draft.working_dir, PathBuf::from("/repo/api"));
        assert_eq!(draft.command, "run dev");
        assert_eq!(draft.url, None);
        assert_eq!(draft.env_vars, vec![("PORT".to_string(), "3000".to_string())]);
    }

    #[test]
    fn config_round_trip_keeps_every_field() {
        let id = Uuid::new_v4();
        let app = AppConfig {
            id,
            name: "Api".to_string(),
            working_dir: PathBuf::from("/repo"),
            command: "start".to_string(),
            kind: AppKind::Npm,
            url: Some("http://localhost:3000".to_string()),
            env_vars: vec![("A".to_string(), "B".to_string())],
            auto_restart: true,
            start_order: 4,
        };
        assert_eq!(AppDraft::from_config(&app).into_config(id), app);
    }
}
