//! Local sync preferences (§12.5) and secret storage (§15.6). Never serialize credentials.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const TOKEN_SERVICE: &str = "utopia-sync";
pub const PASSPHRASE_SERVICE: &str = "utopia-sync-passphrase";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    #[default]
    Off,
    Relay,
    Nearby,
    Folder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub version: u32,
    pub mode: Mode,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub folder: Option<PathBuf>,
    #[serde(default)]
    pub host: Option<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            mode: Mode::Off,
            url: String::new(),
            folder: None,
            host: None,
        }
    }
}
impl Preferences {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("Unsupported sync settings version.".into());
        }
        // Reject credentials even in an inactive URL field: this whole object goes to disk.
        if !self.url.is_empty() {
            let url = url::Url::parse(&self.url)
                .map_err(|_| "Enter an http:// or https:// relay URL.")?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err("Use a relay URL without credentials, a query, or a fragment. Enter the token in its secret field.".into());
            }
        }
        match self.mode {
            Mode::Relay if self.url.is_empty() => return Err("Enter a relay URL.".into()),
            Mode::Folder if self.folder.as_ref().is_none_or(|p| !p.is_absolute()) => {
                return Err("Choose an absolute shared folder path.".into())
            }
            _ => {}
        }
        if let Some(bind) = &self.host {
            if bind.parse::<std::net::SocketAddr>().is_err() {
                return Err("Enter a hosting address such as 0.0.0.0:8787.".into());
            }
        }
        Ok(())
    }
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Ok(bytes) => match serde_json::from_slice::<Self>(&bytes) {
                Ok(p) if p.validate().is_ok() => (p, None),
                _ => (Self::default(), Some("Sync is Off: saved sync settings are malformed or unsupported. Review and Apply to replace them.".into())),
            },
            Err(_) => (Self::default(), Some("Sync is Off: saved sync settings could not be read.".into())),
        }
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        use std::io::Write;
        self.validate()?;
        let parent = path.parent().ok_or("Invalid settings path.")?;
        std::fs::create_dir_all(parent)
            .map_err(|_| "Could not create the sync settings directory.")?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| "Could not stage sync settings.")?;
        let bytes =
            serde_json::to_vec_pretty(self).map_err(|_| "Could not encode sync settings.")?;
        tmp.write_all(&bytes)
            .and_then(|_| tmp.as_file().sync_all())
            .map_err(|_| "Could not write sync settings.")?;
        tmp.persist(path)
            .map_err(|_| "Could not replace sync settings.")?;
        Ok(())
    }
}

/// Errors are deliberately fixed messages: OS diagnostics must never echo credentials.
pub trait Secrets: Send + Sync {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, String>;
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), String>;
}
pub struct Keychain {
    prefix: Option<String>,
}
impl Keychain {
    pub fn new(prefix: Option<String>) -> Self {
        Self { prefix }
    }
    fn service(&self, name: &str) -> String {
        self.prefix
            .as_ref()
            .map_or_else(|| name.into(), |prefix| format!("{prefix}/{name}"))
    }
}
impl Secrets for Keychain {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, String> {
        #[cfg(target_os = "macos")]
        {
            match security_framework::passwords::get_generic_password(
                &self.service(service),
                account,
            ) {
                Ok(bytes) => String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| "Keychain credential is not valid text.".into()),
                Err(e) if e.code() == -25300 => Ok(None),
                Err(_) => Err("Could not read the sync credential from Keychain.".into()),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (service, account);
            Ok(None)
        }
    }
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            security_framework::passwords::set_generic_password(
                &self.service(service),
                account,
                value.as_bytes(),
            )
            .map_err(|_| "Could not save the sync credential in Keychain.".into())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (service, account, value);
            Err("Secret storage requires macOS Keychain.".into())
        }
    }
}

/// §12.6: conservatively freeze a saved E2E passphrase on first setup, before a push can
/// escape. Blank fields retain credentials; switching Off never deletes the encryption key.
pub fn save_secrets(
    secrets: &dyn Secrets,
    graph: &str,
    token: &str,
    pass: &str,
) -> Result<(), String> {
    if !pass.is_empty() {
        if let Some(old) = secrets.get(PASSPHRASE_SERVICE, graph)? {
            if old != pass {
                return Err("This graph already has an E2E passphrase. Changing it would make existing encrypted updates unreadable; re-keying is not supported.".into());
            }
        }
        secrets.set(PASSPHRASE_SERVICE, graph, pass)?;
    }
    if !token.is_empty() {
        secrets.set(TOKEN_SERVICE, graph, token)?;
    }
    Ok(())
}

#[derive(Clone, Default)]
pub struct Environment {
    pub names: Vec<&'static str>,
    pub url: Option<String>,
    pub folder: Option<String>,
    pub host: Option<String>,
    pub graph: Option<String>,
    pub token: Option<String>,
    pub passphrase: Option<String>,
}
impl Environment {
    pub fn read(mut read: impl FnMut(&str) -> Option<String>) -> Self {
        let mut names = Vec::new();
        let mut get = |key| {
            let v = read(key).filter(|v| !v.trim().is_empty());
            if v.is_some() {
                names.push(key);
            }
            v
        };
        let url = get("UTOPIA_SYNC_URL");
        let folder = get("UTOPIA_SYNC_FOLDER");
        let host = get("UTOPIA_LAN_SERVE");
        let graph = get("UTOPIA_SYNC_GRAPH");
        let token = get("UTOPIA_SYNC_TOKEN");
        let passphrase = get("UTOPIA_SYNC_PASSPHRASE");
        Self {
            names,
            url,
            folder,
            host,
            graph,
            token,
            passphrase,
        }
    }
    pub fn apply(&self, saved: &Preferences) -> Preferences {
        let mut p = saved.clone();
        if let Some(host) = &self.host {
            p.mode = Mode::Nearby;
            p.host = Some(host.clone());
        } else if let Some(url) = &self.url {
            p.mode = if url == "lan" {
                Mode::Nearby
            } else {
                Mode::Relay
            };
            p.url = if url == "lan" {
                String::new()
            } else {
                url.clone()
            };
            p.host = None;
        } else if let Some(folder) = &self.folder {
            p.mode = Mode::Folder;
            p.folder = Some(folder.into());
            p.host = None;
        }
        p
    }
}

#[cfg(test)]
mod tests;

/// A non-secret, encrypted fixed marker protects the graph even if its Keychain item is
/// lost or the graph is copied (§12.6). No passphrase, derived key or token is serialized.
/// Like a page payload, the marker can only be opened by the original graph key.
pub fn verify_passphrase(
    pass: Option<&str>,
    graph: &str,
    marker: Option<&str>,
) -> Result<Option<String>, String> {
    let refusal = "This graph has used E2E encryption. Enter its original passphrase; changing or removing it would make synced data unreadable. Re-keying is not supported.";
    let Some(pass) = pass else {
        return if marker.is_some() {
            Err(refusal.into())
        } else {
            Ok(None)
        };
    };
    let key =
        crate::e2e::GraphKey::derive(pass, graph).map_err(|_| "Could not derive the graph key.")?;
    let page = utopia_core::PageId("sync-key-check".into());
    let plain = b"utopia-sync-key-check-v1";
    if let Some(marker) = marker {
        let bytes: Vec<u8> = serde_json::from_str(marker).map_err(|_| refusal)?;
        if key.open(graph, &page, &bytes).map_err(|_| refusal)? != plain {
            return Err(refusal.into());
        }
        Ok(Some(marker.into()))
    } else {
        let bytes = key
            .seal(graph, &page, plain)
            .map_err(|_| "Could not protect the graph key marker.")?;
        serde_json::to_string(&bytes)
            .map(Some)
            .map_err(|_| "Could not encode the key marker.".into())
    }
}
