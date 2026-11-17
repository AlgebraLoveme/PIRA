//! Disposable per-run workspaces are not part of retained Team records.
use crate::{Options, lifecycle, profile, storage};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub(super) fn prepare(options: &Options, run: &Path) -> Result<PathBuf, String> {
    let mut records = vec![run.parent().ok_or("missing Team store")?.to_owned()];
    for root in [
        storage::configured_root("PIRA_TEAM_DIR", "team").ok(),
        storage::platform_team_root().ok(),
        std::env::var_os("PIRA_TEAM_CONTEXT")
            .map(PathBuf::from)
            .and_then(|run| run.parent().map(Path::to_owned)),
    ]
    .into_iter()
    .flatten()
    {
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()
                .map_err(|e| e.to_string())?
                .join(root)
        };
        records.push(
            root.canonicalize()
                .map(Ok)
                .unwrap_or_else(|_| profile::physical(&root))?,
        );
    }
    let mut bases = vec![std::env::temp_dir()];
    // An older Team worker may have inherited TMPDIR/TEMP pointing into records.
    #[cfg(unix)]
    bases.push(PathBuf::from("/tmp"));
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        bases.push(PathBuf::from(local).join("Temp"));
    }
    prepare_in(options, run, &records, &bases)
}

fn prepare_in(
    options: &Options,
    run: &Path,
    records: &[PathBuf],
    bases: &[PathBuf],
) -> Result<PathBuf, String> {
    let mut hash = DefaultHasher::new();
    run.hash(&mut hash);
    // PIRA: stable for a given launcher and temp base; a platform cleanup, changed
    // temp base or launcher hash implementation may recreate disposable state.
    let name = format!(
        "pira-team-{:016x}-{}",
        hash.finish(),
        run.file_name().ok_or("missing run ID")?.to_string_lossy()
    );
    let mut failure = String::from("no existing platform temp directory outside Team records");
    for base in bases {
        let Ok(base) = profile::physical(base) else {
            continue;
        };
        if !base.is_dir() || !outside_records(&base, records) {
            continue;
        }
        if base.to_str().is_none() {
            failure = "worker temporary paths must be UTF-8 for native shell settings".into();
            continue;
        }
        let path = base.join(&name);
        if let Err(error) = options.execution.require_temp_writable(&path) {
            failure = error;
            continue;
        }
        // Refuse replaceable non-sticky ancestors, as for managed run storage.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            for ancestor in base.ancestors() {
                let meta = std::fs::metadata(ancestor).map_err(|e| e.to_string())?;
                if (meta.uid() != unsafe { libc::geteuid() } && meta.uid() != 0)
                    || (meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0)
                {
                    return Err(
                        "unsafe worker temp ancestor; configure a private platform temp directory"
                            .into(),
                    );
                }
            }
        }
        match crate::private_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("create worker temporary workspace: {error}")),
        }
        lifecycle::private_path(&path, true)?;
        let physical = path.canonicalize().map_err(|e| e.to_string())?;
        if !outside_records(&physical, records) {
            return Err("worker temp resolves inside Team records".into());
        }
        return Ok(physical);
    }
    Err(format!(
        "{failure}; set the platform temp environment outside Team records"
    ))
}

fn outside_records(path: &Path, records: &[PathBuf]) -> bool {
    !records.iter().any(|root| path.starts_with(root))
        && !path.ancestors().any(|ancestor| {
            ancestor.join("manifest.json").is_file() && ancestor.join("run.lock").is_file()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut random = [0u8; 8];
            getrandom::fill(&mut random).unwrap();
            let root = std::env::temp_dir()
                .join(format!("team-temp-test-{:x}", u64::from_ne_bytes(random)));
            crate::private_dir(&root).unwrap();
            Self(root.canonicalize().unwrap())
        }
        fn options(&self) -> Options {
            let mut options = crate::tests::options(&["fixture"]).unwrap();
            options.execution = profile::Execution::from_context(&json!({
                "approval_policy":"never", "cwd":self.0,
                "sandbox_policy":{"type":"workspace-write","writable_roots":[],
                    "network_access":false,"exclude_tmpdir_env_var":true,"exclude_slash_tmp":true}
            }))
            .unwrap();
            options
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn outside_workspace_is_private_stable_and_recreated_without_touching_old_scratch() {
        let fixture = Fixture::new();
        let options = fixture.options();
        let store = fixture.0.join("records");
        let run = store.join("run");
        fs::create_dir_all(run.join("scratch")).unwrap();
        fs::write(run.join("scratch/keep"), "retained").unwrap();
        let bases = [store.clone(), run.join("scratch"), fixture.0.clone()];
        let path = prepare_in(&options, &run, std::slice::from_ref(&store), &bases).unwrap();
        assert_eq!(path.parent(), Some(fixture.0.as_path()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        fs::write(path.join("payload"), "temporary").unwrap();
        assert_eq!(
            prepare_in(&options, &run, std::slice::from_ref(&store), &bases).unwrap(),
            path
        );
        assert_eq!(
            fs::read_to_string(path.join("payload")).unwrap(),
            "temporary"
        );
        fs::remove_dir_all(&path).unwrap();
        assert_eq!(
            prepare_in(&options, &run, std::slice::from_ref(&store), &bases).unwrap(),
            path
        );
        assert!(path.is_dir());
        assert_eq!(
            fs::read_to_string(run.join("scratch/keep")).unwrap(),
            "retained"
        );
        let other_run = fixture.0.join("other-records/run");
        assert_ne!(
            prepare_in(
                &options,
                &other_run,
                &[store],
                std::slice::from_ref(&fixture.0)
            )
            .unwrap(),
            path
        );
    }

    #[test]
    fn permissions_and_record_boundaries_fail_before_allocation() {
        let fixture = Fixture::new();
        let other = Fixture::new();
        let options = fixture.options();
        let run = fixture.0.join("records/run");
        let error = prepare_in(&options, &run, &[], std::slice::from_ref(&other.0)).unwrap_err();
        assert!(error.contains("no extra write root"), "{error}");
        assert_eq!(fs::read_dir(&other.0).unwrap().count(), 0);
        assert!(
            prepare_in(
                &options,
                &run,
                std::slice::from_ref(&fixture.0),
                std::slice::from_ref(&fixture.0)
            )
            .is_err()
        );
        let retained = fixture.0.join("unconfigured-records/run");
        fs::create_dir_all(&retained).unwrap();
        fs::write(retained.join("manifest.json"), "{}").unwrap();
        fs::write(retained.join("run.lock"), "").unwrap();
        assert!(prepare_in(&options, &run, &[], &[retained]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn aliases_into_records_and_replaced_workspace_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let options = fixture.options();
        let store = fixture.0.join("records");
        fs::create_dir(&store).unwrap();
        let run = store.join("run");
        let alias = fixture.0.join("alias");
        symlink(&store, &alias).unwrap();
        assert!(prepare_in(&options, &run, std::slice::from_ref(&store), &[alias]).is_err());
        let path = prepare_in(
            &options,
            &run,
            std::slice::from_ref(&store),
            std::slice::from_ref(&fixture.0),
        )
        .unwrap();
        fs::remove_dir(&path).unwrap();
        symlink(&store, &path).unwrap();
        assert!(prepare_in(&options, &run, &[store], std::slice::from_ref(&fixture.0)).is_err());
    }
}
