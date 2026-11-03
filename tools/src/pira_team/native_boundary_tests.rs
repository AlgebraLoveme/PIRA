mod launch_context_tests {
    use super::*;
    use std::fs;
    struct Fixture {
        root: PathBuf,
        run: PathBuf,
        attempt: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "team-context-{}-{}",
                std::process::id(),
                crate::claude::uuid().unwrap()
            ));
            crate::private_dir(&root).unwrap();
            let run = root.join("run");
            crate::private_dir(&run).unwrap();
            let attempt = run.join("attempt-1");
            crate::private_dir(&attempt).unwrap();
            save_json(&run.join("manifest.json"), &json!({"run_id":"run","status":"running","revision":1,"thread_id":"worker-session","transport":"app-server","creator_main_session_id":"main-session","allow_subteam":false,"parent_run_id":null})).unwrap();
            save_json(&run.join("runtime.json"), &json!({"revision":1,"backend_pid":10,"backend_birth":100,"launcher_pid":11,"launcher_birth":90,"attempt_logs":attempt})).unwrap();
            Self { root, run, attempt }
        }
        fn caller(&self, attempt: &Path) -> Result<Caller, String> {
            supplied_caller(
                Some(self.run.clone().into_os_string()),
                Some("1".into()),
                Some(attempt.as_os_str().to_owned()),
                None,
            )?
            .ok_or("missing supplied caller".into())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
    #[test]
    fn unmarked_main_needs_no_process_ancestry() {
        assert!(supplied_caller(None, None, None, None).unwrap().is_none());
    }
    #[test]
    fn partial_or_malformed_launch_context_is_not_main() {
        assert!(supplied_caller(None, None, None, Some("handoff".into())).is_err());
        let f = Fixture::new();
        let run = Some(f.run.clone().into_os_string());
        let attempt = Some(f.attempt.clone().into_os_string());
        for input in [
            (run.clone(), None, attempt.clone()),
            (None, Some("1".into()), attempt.clone()),
            (run.clone(), Some("1".into()), None),
            (run, Some("0".into()), attempt),
        ] {
            assert!(supplied_caller(input.0, input.1, input.2, None).is_err());
        }
    }
    #[test]
    fn supplied_run_retains_session_and_role_without_pid_authority() {
        let f = Fixture::new();
        let caller = f.caller(&f.attempt).unwrap();
        assert_eq!(caller.run, f.run.canonicalize().unwrap());
        let mut manifest = read_json(&f.run.join("manifest.json")).unwrap();
        for transport in ["app-server", "claude-stream-json"] {
            manifest["transport"] = json!(transport);
            let expected = profile::native_caller_identity(&manifest).unwrap();
            assert!(profile::matches_native_session(
                &manifest,
                &json!({"caller_thread_id":expected})
            ));
            assert!(!profile::matches_native_session(
                &manifest,
                &json!({"caller_thread_id":"wrong-session"})
            ));
        }
    }
    #[test]
    fn resumed_session_accepts_new_pid_and_rejects_previous_attempt() {
        let f = Fixture::new();
        let old = f.caller(&f.attempt).unwrap();
        let attempt = f.run.join("attempt-2");
        crate::private_dir(&attempt).unwrap();
        let mut runtime = old.runtime;
        runtime["backend_pid"] = json!(20);
        runtime["backend_birth"] = json!(200);
        runtime["revision"] = json!(2);
        runtime["attempt_logs"] = json!(attempt);
        let mut manifest = read_json(&f.run.join("manifest.json")).unwrap();
        manifest["revision"] = json!(2);
        save_json(&f.run.join("runtime.json"), &runtime).unwrap();
        save_json(&f.run.join("manifest.json"), &manifest).unwrap();
        assert!(f.caller(&f.attempt).is_err());
        assert_eq!(f.caller(&attempt).unwrap().runtime["backend_pid"], 20);
        assert_eq!(manifest["creator_main_session_id"], "main-session");
        assert_eq!(manifest["thread_id"], "worker-session");
        assert_eq!(manifest["allow_subteam"], false);
    }
    #[test]
    fn inconsistent_runtime_or_finished_run_is_rejected() {
        let f = Fixture::new();
        let runtime = read_json(&f.run.join("runtime.json")).unwrap();
        for field in ["revision", "backend_birth", "launcher_pid", "attempt_logs"] {
            let mut invalid = runtime.clone();
            invalid[field] = Value::Null;
            save_json(&f.run.join("runtime.json"), &invalid).unwrap();
            assert!(f.caller(&f.attempt).is_err(), "{field}");
        }
        save_json(&f.run.join("runtime.json"), &runtime).unwrap();
        let mut manifest = read_json(&f.run.join("manifest.json")).unwrap();
        manifest["status"] = json!("completed");
        save_json(&f.run.join("manifest.json"), &manifest).unwrap();
        assert!(f.caller(&f.attempt).is_err());
    }
}
