use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

pub(crate) struct StoreLocation {
    pub path: PathBuf,
    legacy: Vec<PathBuf>,
}

pub(crate) fn configured(option: Option<&PathBuf>) -> Result<StoreLocation, String> {
    let os = if cfg!(all(unix, not(target_os = "macos"))) {
        "unix"
    } else {
        std::env::consts::OS
    };
    select(option, os, |name| std::env::var_os(name))
}

fn select(
    option: Option<&PathBuf>,
    os: &str,
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<StoreLocation, String> {
    // Explicit overrides retain their existing interpretation, including relative paths.
    if let Some(path) = option
        .cloned()
        .or_else(|| env("PIRA_CTX_STORE_DIR").map(PathBuf::from))
    {
        return Ok(StoreLocation {
            path,
            legacy: vec![],
        });
    }
    let base = |name| {
        env(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let home = base("HOME");
    let mut legacy = Vec::new();
    let path = match os {
        "windows" => base("LOCALAPPDATA").map(|root| root.join("PIRA").join("ctx")),
        "macos" => home.map(|root| {
            legacy.push(root.join("Library").join("Caches").join("PIRA").join("ctx"));
            root.join("Library")
                .join("Application Support")
                .join("PIRA")
                .join("ctx")
        }),
        // The previous Unix default used XDG_CACHE_HOME even when relative or empty.
        // Retain that exact discovery path; also check the standard HOME cache location.
        "unix" | "linux" => {
            if let Some(root) = env("XDG_CACHE_HOME") {
                legacy.push(PathBuf::from(root).join("pira").join("ctx"));
            }
            if let Some(root) = &home {
                let old = root.join(".cache").join("pira").join("ctx");
                if !legacy.contains(&old) {
                    legacy.push(old);
                }
            }
            // XDG base-directory settings must be absolute; empty/relative means unset.
            base("XDG_DATA_HOME")
                .filter(|root| root.is_absolute())
                .or_else(|| home.map(|root| root.join(".local").join("share")))
                .map(|root| root.join("pira").join("ctx"))
        }
        _ => None,
    }
    .ok_or_else(|| {
        "cannot determine a per-user pira_ctx store; set PIRA_CTX_STORE_DIR or --store-dir"
            .to_string()
    })?;
    legacy.retain(|old| old != &path);
    Ok(StoreLocation { path, legacy })
}

pub(crate) fn warn_legacy_default(option: Option<&PathBuf>) {
    // Selection errors retain the command's existing error path; this is discovery only.
    let Ok(location) = configured(option) else {
        return;
    };
    for old in location.legacy {
        let detail = match fs::read_dir(&old) {
            Ok(mut entries) => match entries.next() {
                None => continue,
                Some(Ok(_)) => "contains existing entries".to_string(),
                Some(Err(error)) => format!("could not be fully inspected: {error}"),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => format!("could not be inspected: {error}"),
        };
        crate::util::diagnostic_line(&format!(
            "pira_ctx: warning: legacy default store {} {detail}; selected persistent store {} does not merge it. Use --store-dir with the legacy path to access it, or run the source-preserving store migration tool. Default selection has not moved or changed legacy records.",
            old.display(),
            location.path.display()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choose(os: &str, values: &[(&str, OsString)]) -> StoreLocation {
        select(None, os, |key| {
            values
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.clone())
        })
        .unwrap()
    }

    #[test]
    fn all_platform_defaults_and_xdg_boundaries() {
        let root = std::env::temp_dir().join("ctx-default-test");
        let home = root.join("home");
        let data = root.join("data");
        let cache = root.join("cache");
        let values = [
            ("HOME", home.clone().into_os_string()),
            ("LOCALAPPDATA", data.clone().into_os_string()),
            ("XDG_DATA_HOME", data.clone().into_os_string()),
            ("XDG_CACHE_HOME", cache.clone().into_os_string()),
        ];
        let mac = choose("macos", &values);
        assert_eq!(mac.path, home.join("Library/Application Support/PIRA/ctx"));
        assert_eq!(mac.legacy, vec![home.join("Library/Caches/PIRA/ctx")]);
        let windows = choose("windows", &values);
        assert_eq!(windows.path, data.join("PIRA/ctx"));
        assert!(windows.legacy.is_empty());
        let linux = choose("linux", &values);
        assert_eq!(linux.path, data.join("pira/ctx"));
        assert_eq!(
            linux.legacy,
            vec![cache.join("pira/ctx"), home.join(".cache/pira/ctx")]
        );
        for setting in [
            None,
            Some(OsString::new()),
            Some(OsString::from("relative")),
        ] {
            let mut values = vec![("HOME", home.clone().into_os_string())];
            if let Some(setting) = setting {
                values.push(("XDG_DATA_HOME", setting));
            }
            assert_eq!(
                choose("linux", &values).path,
                home.join(".local/share/pira/ctx")
            );
        }
        for os in ["macos", "linux", "windows"] {
            assert!(select(None, os, |_| None).is_err());
            let override_path = PathBuf::from("explicit");
            let chosen = select(Some(&override_path), os, |_| {
                panic!("CLI override must not consult environment")
            })
            .unwrap();
            assert_eq!(chosen.path, override_path);
            assert!(chosen.legacy.is_empty());
            let chosen = choose(os, &[("PIRA_CTX_STORE_DIR", "environment".into())]);
            assert_eq!(chosen.path, PathBuf::from("environment"));
            assert!(chosen.legacy.is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_environment_units_are_not_replaced() {
        use std::os::unix::ffi::OsStringExt;
        let home = OsString::from_vec(b"/native-\xff".to_vec());
        let chosen = choose("macos", &[("HOME", home.clone())]);
        assert_eq!(
            chosen.path,
            PathBuf::from(home).join("Library/Application Support/PIRA/ctx")
        );
        let path = OsString::from_vec(b"/override-\xff".to_vec());
        let chosen = choose("linux", &[("PIRA_CTX_STORE_DIR", path.clone())]);
        assert_eq!(chosen.path, PathBuf::from(path));
    }
}
