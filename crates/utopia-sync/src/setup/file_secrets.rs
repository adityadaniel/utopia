use std::collections::BTreeMap;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::Secrets;

type Table = BTreeMap<String, BTreeMap<String, String>>;

pub struct FileSecrets {
    path: PathBuf,
}

impl FileSecrets {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn in_config_dir() -> Self {
        let dir = if let Some(d) = std::env::var_os("UTOPIA_CONFIG_DIR") {
            PathBuf::from(d)
        } else if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
            PathBuf::from(x).join("utopia")
        } else {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config/utopia")
        };
        Self::new(dir.join("credentials.json"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Result<Table, String> {
        let unreadable = || "Could not read the sync credentials file.".to_string();
        let mut file = match std::fs::File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Table::new()),
            Err(_) => return Err(unreadable()),
        };
        let meta = file.metadata().map_err(|_| unreadable())?;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "The sync credentials file {} can be read by other users; run `chmod 600` on it.",
                self.path.display()
            ));
        }
        let mut text = String::new();
        std::io::Read::read_to_string(&mut file, &mut text).map_err(|_| unreadable())?;
        serde_json::from_str(&text)
            .map_err(|_| "The sync credentials file is not valid JSON.".to_string())
    }

    fn write(&self, table: &Table) -> Result<(), String> {
        let failed = || "Could not save the sync credentials file.".to_string();
        let dir = self.path.parent().ok_or_else(failed)?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|_| failed())?;
        let dir_mode = std::fs::metadata(dir)
            .map_err(|_| failed())?
            .permissions()
            .mode();
        if dir_mode & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(dir_mode & 0o700))
                .map_err(|_| failed())?;
        }
        let text = serde_json::to_string_pretty(table).map_err(|_| failed())?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|_| failed())?;
        std::io::Write::write_all(&mut tmp, text.as_bytes()).map_err(|_| failed())?;
        tmp.as_file().sync_all().map_err(|_| failed())?;
        tmp.persist(&self.path).map_err(|_| failed())?;
        std::fs::File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(|_| failed())
    }
}

impl Secrets for FileSecrets {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, String> {
        Ok(self
            .read()?
            .get(service)
            .and_then(|accounts| accounts.get(account))
            .cloned())
    }

    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), String> {
        let mut table = self.read()?;
        table
            .entry(service.to_string())
            .or_default()
            .insert(account.to_string(), value.to_string());
        self.write(&table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn an_existing_config_directory_is_made_private() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::create_dir(&config).unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(config.join("other.json"), "{}").unwrap();
        FileSecrets::new(config.join("credentials.json"))
            .set("utopia-sync", "notes", "tok")
            .unwrap();
        assert_eq!(mode(&config), 0o700);
        assert!(config.join("other.json").is_file());
    }

    #[test]
    fn credentials_round_trip_in_a_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let secrets = FileSecrets::new(config.join("credentials.json"));
        assert_eq!(secrets.get("utopia-sync", "notes").unwrap(), None);
        secrets.set("utopia-sync", "notes", "tok-1").unwrap();
        secrets
            .set("utopia-sync-passphrase", "notes", "pass")
            .unwrap();
        secrets.set("utopia-sync", "work", "tok-2").unwrap();
        secrets.set("utopia-sync", "notes", "tok-3").unwrap();
        assert_eq!(
            secrets.get("utopia-sync", "notes").unwrap().as_deref(),
            Some("tok-3")
        );
        assert_eq!(
            secrets.get("utopia-sync", "work").unwrap().as_deref(),
            Some("tok-2")
        );
        assert_eq!(
            secrets
                .get("utopia-sync-passphrase", "notes")
                .unwrap()
                .as_deref(),
            Some("pass")
        );
        assert_eq!(secrets.get("utopia-sync-passphrase", "work").unwrap(), None);
        assert_eq!(mode(secrets.path()), 0o600);
        assert_eq!(mode(&config), 0o700);
        assert_eq!(std::fs::read_dir(&config).unwrap().count(), 1);
    }

    #[test]
    fn a_readable_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let secrets = FileSecrets::new(path.clone());
        secrets.set("utopia-sync", "notes", "secret-token").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = secrets.get("utopia-sync", "notes").unwrap_err();
        assert!(err.contains("chmod 600"), "{err}");
        assert!(!err.contains("secret-token"), "{err}");
        assert!(secrets.set("utopia-sync", "notes", "other").is_err());
    }

    #[test]
    fn a_damaged_file_is_an_error_not_an_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(&path, "{not json").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let secrets = FileSecrets::new(path);
        assert!(secrets.get("utopia-sync", "notes").is_err());
        assert!(
            secrets.set("utopia-sync", "notes", "x").is_err(),
            "a write never replaces a file it could not read"
        );
    }
}
