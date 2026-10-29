mod native_boundary_tests {
    use super::*;
    use crate::native::Metadata;
    use std::{cell::RefCell, io};

    fn metadata(identity: &crate::native::Identity) -> Metadata {
        Metadata {
            pid: identity.pid,
            parent: identity.parent,
            birth: identity.birth,
            alive: identity.alive,
        }
    }

    fn backend_arguments(run: &Path) -> Vec<String> {
        let mut args = arguments(&["codex", "app-server", "--strict-config", "--stdio", "-c"]);
        args.push(format!("{MARKER}{}", json!(run)));
        args
    }

    fn inspect_fixture(
        fixture: &DiscoveryFixture,
        pid: u32,
    ) -> Result<(crate::native::Identity, Option<PathBuf>), String> {
        let (identity, marker) = fixture.inspect(pid)?;
        inspect_linux_using(
            pid,
            |_| Ok(metadata(&identity)),
            |_| {
                Ok(marker
                    .as_deref()
                    .map(backend_arguments)
                    .unwrap_or_else(|| arguments(&["sshd: fixture"])))
            },
            |_| {
                if marker.is_some() {
                    Ok(PathBuf::from("/fixture/codex"))
                } else {
                    Err(io::ErrorKind::PermissionDenied.into())
                }
            },
        )
    }

    #[test]
    fn protected_root_ancestors_are_traversed_not_trusted_as_boundary() {
        let mut fixture = DiscoveryFixture::new(12);
        for (_, marker) in &mut fixture.processes {
            *marker = None;
        }
        let visited = RefCell::new(Vec::new());
        let result = discover_using(32, |pid| {
            visited.borrow_mut().push(pid);
            inspect_fixture(&fixture, pid)
        });
        assert!(result.unwrap().is_none());
        assert_eq!(*visited.borrow(), [32, 24, 16, 12]);
    }

    #[test]
    fn protected_unmarked_ancestor_cannot_hide_marked_worker_above_it() {
        let mut fixture = DiscoveryFixture::new(12);
        fixture.processes[1].1 = None;
        fixture.processes[2].1 = Some(fixture.run.clone());
        let caller = discover_using(32, |pid| inspect_fixture(&fixture, pid))
            .unwrap()
            .unwrap();
        assert_eq!(caller.run, fixture.run.canonicalize().unwrap());
        assert_eq!(caller.runtime["backend_pid"], 12);
    }

    #[test]
    fn protected_wrapper_metadata_still_binds_exact_owner_birth_and_revision() {
        for owner in [24, 16, 12] {
            let fixture = DiscoveryFixture::new(owner);
            let caller = discover_using(32, |pid| inspect_fixture(&fixture, pid))
                .unwrap()
                .unwrap();
            assert_eq!(caller.runtime["backend_pid"], owner);

            let runtime_path = fixture.run.join("runtime.json");
            let valid = read_json(&runtime_path).unwrap();
            for field in ["backend_birth", "revision"] {
                let mut stale = valid.clone();
                stale[field] = json!(999);
                save_json(&runtime_path, &stale).unwrap();
                assert!(
                    discover_using(32, |pid| inspect_fixture(&fixture, pid)).is_err(),
                    "owner={owner}, field={field}"
                );
            }
        }
    }

    #[test]
    fn marked_backends_require_executable_identity_even_when_protected() {
        let fixture = DiscoveryFixture::new(12);
        for executable in [
            Err(io::ErrorKind::PermissionDenied),
            Err(io::ErrorKind::NotFound),
        ] {
            let result = inspect_linux_using(
                24,
                |_| Ok(metadata(&fixture.processes[1].0)),
                |_| Ok(backend_arguments(&fixture.run)),
                |_| executable.map_err(Into::into),
            );
            assert_eq!(result.unwrap_err(), "cannot verify process identity");
        }
    }

    #[test]
    fn protected_argument_or_stat_denials_never_become_root() {
        let fixture = DiscoveryFixture::new(12);
        for denied in ["stat", "argv"] {
            let result = discover_using(32, |pid| {
                if pid != 24 {
                    return inspect_fixture(&fixture, pid);
                }
                inspect_linux_using(
                    pid,
                    |_| {
                        if denied == "stat" {
                            Err(io::ErrorKind::PermissionDenied.into())
                        } else {
                            Ok(metadata(&fixture.processes[1].0))
                        }
                    },
                    |_| Err(io::ErrorKind::PermissionDenied.into()),
                    |_| panic!("no executable query after metadata/arguments denial"),
                )
            });
            assert!(result.is_err(), "{denied}");
        }
    }

    #[test]
    fn changing_or_disappearing_metadata_during_inspection_fails_closed() {
        let fixture = DiscoveryFixture::new(12);
        for changed in ["birth", "parent", "alive", "missing"] {
            let mut reads = 0;
            let result = inspect_linux_using(
                24,
                |_| {
                    reads += 1;
                    let mut value = metadata(&fixture.processes[1].0);
                    if reads == 2 {
                        match changed {
                            "birth" => value.birth += 1,
                            "parent" => value.parent = 1,
                            "alive" => value.alive = false,
                            "missing" => return Err(io::ErrorKind::NotFound.into()),
                            _ => unreachable!(),
                        }
                    }
                    Ok(value)
                },
                |_| Ok(arguments(&["sshd: fixture"])),
                |_| panic!("unmarked process needs no executable access"),
            );
            assert!(result.is_err(), "{changed}");
        }
    }

    #[test]
    fn backend_marker_still_requires_supported_executable_and_valid_json() {
        let fixture = DiscoveryFixture::new(12);
        let mut malformed = backend_arguments(&fixture.run);
        *malformed.last_mut().unwrap() = format!("{MARKER}not-json");
        for name in ["codex", "python3", "sshd"] {
            let result = inspect_linux_using(
                24,
                |_| Ok(metadata(&fixture.processes[1].0)),
                |_| Ok(malformed.clone()),
                |_| Ok(PathBuf::from(name)),
            );
            if name == "sshd" {
                assert!(result.unwrap().1.is_none());
            } else {
                assert_eq!(result.unwrap_err(), "invalid Team context marker");
            }
        }
        let forged = arguments(&[
            "pira_team",
            "--task",
            "app-server --strict-config --stdio shell_environment_policy.set.PIRA_TEAM_CONTEXT=\"bad\"",
        ]);
        assert!(
            inspect_linux_using(
                24,
                |_| Ok(metadata(&fixture.processes[1].0)),
                |_| Ok(forged),
                |_| panic!("task text is not a backend marker"),
            )
            .unwrap()
            .1
            .is_none()
        );
    }
}
