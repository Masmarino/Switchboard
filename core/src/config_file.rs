use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::app_config::AppConfigList;
use crate::config_error::ConfigError;
use crate::paths;

/// Empreinte bon marche du fichier (mtime + taille) pour detecter une ecriture externe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSignature {
    modified: Option<SystemTime>,
    len: u64,
}

/// Le fichier `apps.json` partage entre les apps natives et le CLI : ecriture atomique
/// (fichier temporaire + rename) et verrou exclusif autour de chaque lecture-modification-ecriture.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    path: PathBuf,
}

impl ConfigFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn in_dir(dir: &Path) -> Self {
        Self::new(dir.join(paths::CONFIG_FILE_NAME))
    }

    pub fn default_location() -> Result<Self, ConfigError> {
        paths::config_dir().map(|dir| Self::in_dir(&dir)).ok_or(ConfigError::NoConfigDir)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn dir(&self) -> &Path {
        self.path.parent().unwrap_or_else(|| Path::new("."))
    }

    /// Fichier absent = liste vide (premier lancement) ; illisible ou JSON invalide = erreur.
    pub fn load(&self) -> Result<AppConfigList, ConfigError> {
        let contents = match std::fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AppConfigList::default()),
            Err(source) => return Err(ConfigError::Io { path: self.path.clone(), source }),
        };
        AppConfigList::from_json(&contents)
            .map_err(|e| ConfigError::Invalid { path: self.path.clone(), message: e.to_string() })
    }

    pub fn signature(&self) -> Option<FileSignature> {
        let meta = std::fs::metadata(&self.path).ok()?;
        Some(FileSignature { modified: meta.modified().ok(), len: meta.len() })
    }

    pub fn save(&self, list: &AppConfigList) -> Result<(), ConfigError> {
        let io_err = |source| ConfigError::Io { path: self.path.clone(), source };
        std::fs::create_dir_all(self.dir()).map_err(io_err)?;
        let json = list.to_json().map_err(|e| io_err(std::io::Error::other(e)))?;
        let tmp = self.path.with_extension(format!("json.tmp-{}", std::process::id()));
        let write = || -> std::io::Result<()> {
            let mut file = create_private(&tmp)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&tmp, &self.path)
        };
        write().map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            io_err(e)
        })
    }

    /// Recharge depuis le disque sous verrou, applique `mutate`, puis sauvegarde si `mutate`
    /// reussit — deux ecrivains (app native + CLI) ne peuvent plus s'ecraser mutuellement.
    pub fn update<T, E: From<ConfigError>>(
        &self,
        mutate: impl FnOnce(&mut AppConfigList) -> Result<T, E>,
    ) -> Result<T, E> {
        let _lock = self.lock()?;
        let mut list = self.load()?;
        let result = mutate(&mut list)?;
        self.save(&list)?;
        Ok(result)
    }

    /// Garde une copie d'un fichier illisible avant de l'ecraser, pour ne jamais perdre
    /// une config editee a la main avec une faute de frappe.
    pub fn backup_invalid(&self) -> std::io::Result<PathBuf> {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let backup = self.path.with_extension(format!("json.invalid-{stamp}"));
        std::fs::copy(&self.path, &backup)?;
        Ok(backup)
    }

    /// Verrou exclusif (fichier `apps.json.lock`), relache quand le `File` retourne est detruit.
    pub fn lock(&self) -> Result<File, ConfigError> {
        let io_err = |source| ConfigError::Io { path: self.path.clone(), source };
        std::fs::create_dir_all(self.dir()).map_err(io_err)?;
        let lock_path = self.path.with_extension("json.lock");
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path).map_err(io_err)?;
        file.lock().map_err(io_err)?;
        Ok(file)
    }
}

/// Les variables d'env peuvent contenir des secrets : fichier lisible par l'utilisateur seul.
fn create_private(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::AppConfig;

    fn temp_file() -> (tempfile::TempDir, ConfigFile) {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = ConfigFile::in_dir(dir.path());
        (dir, file)
    }

    #[test]
    fn missing_file_loads_as_empty_list() {
        let (_dir, file) = temp_file();
        assert!(file.load().expect("load").apps.is_empty());
    }

    #[test]
    fn invalid_json_is_an_error_not_an_empty_list() {
        let (_dir, file) = temp_file();
        std::fs::write(file.path(), "{ not json").unwrap();
        assert!(matches!(file.load(), Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn save_then_load_round_trips() {
        let (_dir, file) = temp_file();
        let mut list = AppConfigList::default();
        list.add(AppConfig { name: "Api".to_string(), ..Default::default() });
        file.save(&list).expect("save");
        assert_eq!(file.load().expect("load"), list);
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, file) = temp_file();
        file.save(&AppConfigList::default()).expect("save");
        let mode = std::fs::metadata(file.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn update_reloads_from_disk_before_mutating() {
        let (_dir, file) = temp_file();
        let mut on_disk = AppConfigList::default();
        on_disk.add(AppConfig { name: "Written by someone else".to_string(), ..Default::default() });
        file.save(&on_disk).unwrap();

        file.update(|list| {
            list.add(AppConfig { name: "Mine".to_string(), ..Default::default() });
            Ok::<_, ConfigError>(())
        })
        .unwrap();

        let names: Vec<_> = file.load().unwrap().apps.into_iter().map(|a| a.name).collect();
        assert_eq!(names, vec!["Written by someone else", "Mine"]);
    }

    #[test]
    fn failed_update_leaves_the_file_untouched() {
        let (_dir, file) = temp_file();
        file.save(&AppConfigList::default()).unwrap();
        let result: Result<(), ConfigError> = file.update(|list| {
            list.add(AppConfig::default());
            Err(ConfigError::NoConfigDir)
        });
        assert!(result.is_err());
        assert!(file.load().unwrap().apps.is_empty());
    }

    #[test]
    fn backup_invalid_keeps_a_copy_of_the_broken_file() {
        let (_dir, file) = temp_file();
        std::fs::write(file.path(), "{ typo").unwrap();
        let backup = file.backup_invalid().unwrap();
        assert!(backup.to_string_lossy().contains(".json.invalid-"));
        assert_eq!(std::fs::read_to_string(backup).unwrap(), "{ typo");
    }

    #[test]
    fn signature_changes_when_the_file_is_rewritten() {
        let (_dir, file) = temp_file();
        assert_eq!(file.signature(), None);
        file.save(&AppConfigList::default()).unwrap();
        let first = file.signature();
        let mut list = AppConfigList::default();
        list.add(AppConfig { name: "Changed".to_string(), ..Default::default() });
        file.save(&list).unwrap();
        assert_ne!(first, file.signature());
    }
}
