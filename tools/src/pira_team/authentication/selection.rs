//! Non-secret setup selection; credentials remain in the native persistent home.
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

pub(crate) const REPAIR: &str = "rerun PIRA setup/install to repair Team authentication (setup_pira_tools.py --auth-only; --reauth for a cached login rejected upstream); ordinary Team launch never starts login";
const INVALID: &str =
    "invalid/private Team auth selection; rerun PIRA setup/install (values withheld)";

pub(crate) fn require_file(
    provider: &crate::authentication::provider::Config,
    has_api_key: bool,
    file: Option<&Path>,
) -> Result<(), String> {
    provider.require_supported_auth(has_api_key)?;
    if provider.requires_file_auth(has_api_key) && file.is_none() {
        return Err(format!(
            "Team-compatible persistent file login is missing; {REPAIR}"
        ));
    }
    Ok(())
}

pub(crate) fn load(home: &Path) -> Result<Option<PathBuf>, String> {
    let path = home.join("pira-team-auth.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(INVALID.into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 {
        return Err(INVALID.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(INVALID.into());
        }
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut text = String::new();
    options
        .open(path)
        .map_err(|_| INVALID)?
        .take(64 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(|_| INVALID)?;
    if text.len() > 64 * 1024 {
        return Err(INVALID.into());
    }
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| INVALID)?;
    if value.as_object().is_none_or(|fields| fields.len() != 2) || value["version"] != 1 {
        return Err(INVALID.into());
    }
    let home = value["home"]
        .as_str()
        .filter(|s| !s.contains('\0'))
        .ok_or(INVALID)?;
    if !Path::new(home).is_absolute() {
        return Err(INVALID.into());
    }
    Ok(Some(PathBuf::from(home)))
}

pub(crate) fn link(source: &Path, target: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(source, target);
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_file(source, target);
    result.map_err(|_| format!("cannot create refresh-safe Codex credential symlink; check filesystem support and Windows Developer Mode/symlink privilege, then {REPAIR}. Another login alone cannot fix missing symlink support; no copy or hard-link fallback was attempted"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_codex_file_guard_does_not_accept_stale_files_for_other_stores() {
        let file = std::env::temp_dir().join("synthetic-native-auth.json");
        for store in ["file", "keyring", "auto", "ephemeral"] {
            let config = crate::authentication::provider::Config::parse(&format!(
                "cli_auth_credentials_store={store:?}"
            ))
            .unwrap();
            assert!(require_file(&config, false, None).is_err());
            assert_eq!(
                require_file(&config, false, Some(&file)).is_ok(),
                store == "file"
            );
            assert!(require_file(&config, true, None).is_ok());
        }
        let unused = crate::authentication::provider::Config::parse("model_provider='local'\ncli_auth_credentials_store='auto'\n[model_providers.local]\nrequires_openai_auth=false").unwrap();
        assert!(require_file(&unused, false, None).is_ok());
    }
}
