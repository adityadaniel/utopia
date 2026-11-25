#![cfg(target_os = "linux")]

use utopia_cli::paths;
use utopia_sync::setup::file_secrets::FileSecrets;

fn same_dir() {
    assert_eq!(
        FileSecrets::in_config_dir().path().parent(),
        Some(paths::config_dir().as_path())
    );
}

#[test]
fn adr_0014_the_credentials_file_lives_in_the_app_config_dir() {
    std::env::set_var("HOME", "/home/someone");
    std::env::remove_var("XDG_CONFIG_HOME");
    std::env::remove_var("UTOPIA_CONFIG_DIR");
    same_dir();
    std::env::set_var("XDG_CONFIG_HOME", "/xdg");
    same_dir();
    std::env::set_var("UTOPIA_CONFIG_DIR", "/explicit");
    same_dir();
}
