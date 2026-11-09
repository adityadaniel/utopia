use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
struct MemorySecrets(Mutex<BTreeMap<(String, String), String>>);
impl Secrets for MemorySecrets {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, String> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&(service.into(), account.into()))
            .cloned())
    }
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .insert((service.into(), account.into()), value.into());
        Ok(())
    }
}

#[test]
fn ac_12_5_settings_round_trip_and_degrade_without_secret_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sync.json");
    let prefs = Preferences {
        mode: Mode::Relay,
        url: "https://relay.example".into(),
        ..Default::default()
    };
    prefs.save(&path).unwrap();
    assert_eq!(Preferences::load(&path), (prefs, None));
    for invalid in [
        r#"{"version":99}"#,
        r#"{"version":1,"mode":"future"}"#,
        "secret-token-invalid-json",
    ] {
        std::fs::write(&path, invalid).unwrap();
        let (prefs, warning) = Preferences::load(&path);
        assert_eq!(prefs.mode, Mode::Off);
        let warning = warning.unwrap();
        assert!(!warning.contains(invalid));
    }
}

#[test]
fn ac_15_6_secrets_stay_in_keychain_and_rekey_is_refused() {
    let secrets = MemorySecrets::default();
    save_secrets(&secrets, "g", "token-secret", "pass-secret").unwrap();
    assert_eq!(
        secrets.get(TOKEN_SERVICE, "g").unwrap().as_deref(),
        Some("token-secret")
    );
    assert!(save_secrets(&secrets, "g", "replacement-token", "different-pass").is_err());
    assert_eq!(
        secrets.get(TOKEN_SERVICE, "g").unwrap().as_deref(),
        Some("token-secret")
    );
    save_secrets(&secrets, "g", "replacement-token", "").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sync.json");
    Preferences::default().save(&file).unwrap();
    let disk = std::fs::read_to_string(file).unwrap();
    for secret in ["token-secret", "pass-secret", "replacement-token"] {
        assert!(!disk.contains(secret));
    }
}

#[test]
fn ac_12_7_environment_overrides_saved_transport_without_persisting_secrets() {
    let saved = Preferences {
        mode: Mode::Folder,
        folder: Some("/tmp/shared".into()),
        ..Default::default()
    };
    let env = Environment::read(|key| match key {
        "UTOPIA_SYNC_URL" => Some("https://relay.example".into()),
        "UTOPIA_SYNC_TOKEN" => Some("env-secret".into()),
        _ => None,
    });
    let effective = env.apply(&saved);
    assert_eq!(effective.mode, Mode::Relay);
    assert_eq!(saved.mode, Mode::Folder);
    assert_eq!(env.names, vec!["UTOPIA_SYNC_URL", "UTOPIA_SYNC_TOKEN"]);
    assert!(!serde_json::to_string(&effective)
        .unwrap()
        .contains("env-secret"));
}

#[test]
fn ac_15_6_relay_urls_cannot_hide_credentials() {
    for url in [
        "https://user:secret@example.com",
        "https://example.com?token=secret",
        "https://example.com/#secret",
        "file:///tmp/relay",
    ] {
        assert!(Preferences {
            mode: Mode::Relay,
            url: url.into(),
            ..Default::default()
        }
        .validate()
        .is_err());
    }
}

/// §12.6: copying a graph or losing a keychain item must not permit a silent re-key.
#[test]
fn ac_12_6_encrypted_graph_requires_the_original_passphrase_even_without_keychain() {
    let marker = verify_passphrase(Some("original"), "g", None)
        .unwrap()
        .unwrap();
    assert!(!marker.contains("original"));
    assert!(verify_passphrase(Some("replacement"), "g", Some(&marker)).is_err());
    assert!(verify_passphrase(None, "g", Some(&marker)).is_err());
    assert_eq!(
        verify_passphrase(Some("original"), "g", Some(&marker)).unwrap(),
        Some(marker)
    );
}

#[test]
fn ac_12_5_atomic_settings_replacement_leaves_no_secret_or_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sync.json");
    Preferences::default().save(&file).unwrap();
    let prefs = Preferences {
        mode: Mode::Relay,
        url: "https://example.com".into(),
        ..Default::default()
    };
    prefs.save(&file).unwrap();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    let invalid = Preferences {
        url: "https://secret:token@example.com".into(),
        ..prefs.clone()
    };
    assert!(invalid.save(&file).is_err());
    assert_eq!(Preferences::load(&file).0, prefs);
}
