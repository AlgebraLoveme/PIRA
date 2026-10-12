//! Explicit configured build/cache grants, separate from environment inheritance.
use std::ffi::OsString;
use std::path::PathBuf;

const CACHE_KEYS: &[&str] = &[
    "CARGO_HOME",
    "CARGO_TARGET_DIR",
    "PIP_CACHE_DIR",
    "UV_CACHE_DIR",
    "PYTHONPYCACHEPREFIX",
    "MYPY_CACHE_DIR",
    "npm_config_cache",
    "NPM_CONFIG_CACHE",
    "YARN_CACHE_FOLDER",
    "GOCACHE",
    "GOMODCACHE",
    "GRADLE_USER_HOME",
    "CCACHE_DIR",
    "SCCACHE_DIR",
    "XDG_CACHE_HOME",
];

pub fn configured() -> Result<Vec<PathBuf>, String> {
    resolve(|key| std::env::var_os(key))
}

fn resolve(get: impl Fn(&str) -> Option<OsString>) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::new();
    let directory = |value: OsString| -> Result<PathBuf, String> {
        let path = PathBuf::from(value);
        if !path.is_absolute() || !path.is_dir() {
            return Err("expected an existing absolute directory".into());
        }
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        if path.parent().is_none() {
            return Err("filesystem root is not a build/cache directory".into());
        }
        Ok(path)
    };
    for key in CACHE_KEYS {
        // PIRA: preserve the ambient Cargo contract: unusable configured roots grant nothing.
        if let Some(path) = get(key).and_then(|value| directory(value).ok())
            && !roots.contains(&path)
        {
            roots.push(path);
        }
    }
    if let Some(value) = get("PIRA_TEAM_BUILD_ROOTS") {
        let value = value
            .to_str()
            .ok_or("PIRA_TEAM_BUILD_ROOTS must be UTF-8 JSON")?;
        let values: Vec<String> = serde_json::from_str(value).map_err(
            |_| "PIRA_TEAM_BUILD_ROOTS must be a JSON array of absolute directory strings",
        )?;
        for value in values {
            let path =
                directory(value.into()).map_err(|e| format!("PIRA_TEAM_BUILD_ROOTS: {e}"))?;
            if !roots.contains(&path) {
                roots.push(path);
            }
        }
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn aliases_grant_the_physical_directory_once() {
        let root = std::env::temp_dir().join(format!("team-root-alias-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let alias = root.join("alias");
        let target = root.join("target");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let extra = serde_json::to_string(&[&alias, &target]).unwrap();
        let roots =
            resolve(|k| (k == "PIRA_TEAM_BUILD_ROOTS").then(|| extra.clone().into())).unwrap();
        assert_eq!(roots, [target.canonicalize().unwrap()]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configured_grants_are_deliberate_and_explicit_errors_are_visible() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let value = root.as_os_str().to_owned();
        for key in [
            "CARGO_TARGET_DIR",
            "UV_CACHE_DIR",
            "npm_config_cache",
            "GOCACHE",
            "GRADLE_USER_HOME",
            "CCACHE_DIR",
        ] {
            assert_eq!(
                resolve(|k| (k == key).then(|| value.clone())).unwrap(),
                std::slice::from_ref(&root)
            );
        }
        assert!(
            resolve(|k| (k == "UNKNOWN_OUTPUT_DIR").then(|| value.clone()))
                .unwrap()
                .is_empty()
        );
        for value in ["relative", "/nonexistent/pira-team-cache"] {
            assert!(
                resolve(|k| (k == "CARGO_HOME").then(|| value.into()))
                    .unwrap()
                    .is_empty()
            );
        }
        let explicit = serde_json::to_string(&[&root, &root]).unwrap();
        assert_eq!(
            resolve(|k| match k {
                "CARGO_HOME" => Some(value.clone()),
                "PIRA_TEAM_BUILD_ROOTS" => Some(explicit.clone().into()),
                _ => None,
            })
            .unwrap(),
            [root]
        );
        for value in [
            "",
            "{}",
            "[1]",
            "[\"relative\"]",
            "[\"/nonexistent/pira-team-cache\"]",
            "[\"/\"]",
        ] {
            assert!(resolve(|k| (k == "PIRA_TEAM_BUILD_ROOTS").then(|| value.into())).is_err());
        }
    }
}
