//! Own backend termination by an isolated process group or a Windows job handle.
use std::process::{Child, Command};

#[derive(Clone)]
pub struct Scope {
    pub pid: u32,
    #[cfg(unix)]
    live: std::sync::Arc<std::sync::Mutex<bool>>,
    #[cfg(windows)]
    job: std::sync::Arc<crate::native::windows::Job>,
}
impl Scope {
    pub fn terminate(&self) {
        #[cfg(unix)]
        {
            let live = self.live.lock().unwrap();
            if *live && let Ok(pid) = i32::try_from(self.pid) {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
        #[cfg(windows)]
        self.job.terminate();
    }
    /// Check exit without releasing Unix leader identity before scope cleanup.
    pub fn try_wait(&self, child: &mut Child) -> std::io::Result<Option<std::process::ExitStatus>> {
        #[cfg(unix)]
        {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            if unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.pid,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if info.si_signo == 0 {
                return Ok(None);
            }
        }
        #[cfg(windows)]
        if child.try_wait()?.is_none() {
            return Ok(None);
        }
        self.wait(child).map(Some)
    }

    pub fn wait(&self, child: &mut Child) -> std::io::Result<std::process::ExitStatus> {
        #[cfg(unix)]
        {
            // WNOWAIT leaves the leader PID reserved while the group is stopped.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            loop {
                if unsafe {
                    libc::waitid(
                        libc::P_PID,
                        self.pid,
                        &mut info,
                        libc::WEXITED | libc::WNOWAIT,
                    )
                } == 0
                {
                    break;
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    *self.live.lock().unwrap() = false;
                    return Err(error);
                }
            }
            self.terminate();
            *self.live.lock().unwrap() = false;
            child.wait()
        }
        #[cfg(windows)]
        {
            let status = child.wait();
            self.terminate();
            status
        }
    }
    pub fn job_name(&self) -> Option<&str> {
        #[cfg(windows)]
        {
            Some(&self.job.name)
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

pub fn spawn(command: &mut Command) -> Result<(Child, Scope), String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Assign the suspended process before it can create descendants.
        command.creation_flags(
            crate::native::windows::CREATE_SUSPENDED
                | crate::native::windows::CREATE_NEW_PROCESS_GROUP,
        );
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("launch owned process: {e}"))?;
    #[cfg(windows)]
    let job = match crate::native::windows::Job::assign(&child) {
        Ok(job) => std::sync::Arc::new(job),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("isolate native backend: {error}"));
        }
    };
    #[cfg(not(windows))]
    let _ = &mut child;
    let scope = Scope {
        pid: child.id(),
        #[cfg(unix)]
        live: std::sync::Arc::new(std::sync::Mutex::new(true)),
        #[cfg(windows)]
        job,
    };
    Ok((child, scope))
}

pub fn isolate_launcher(
    args: &[String],
    runtime: &serde_json::Value,
) -> Result<Option<i32>, String> {
    #[cfg(unix)]
    {
        let _ = (args, runtime);
        // An inherited-group launcher can leave in place. A waiting proxy here
        // would retain the coordinator backend's group after its leader exits,
        // correctly preventing ownerless settlement while the child runs.
        if unsafe { libc::getpgrp() != libc::getpid() } {
            if unsafe { libc::setsid() } == -1 {
                return Err("cannot isolate child launcher".into());
            }
            return Ok(None);
        }
        // The CLI calls here after verified caller discovery, but before starting
        // threads or acquiring run ownership. Fork retains that private verified
        // binding; no environment assertion/re-exec can manufacture it. Do not
        // move this past threaded startup: the child continues Rust execution.
        // A group leader cannot escape a command manager that retains
        // this PID as its kill target/PGID (Codex pipe commands already lead a
        // session). Leave only a waiting proxy in that scope, as on Windows.
        let child = unsafe { libc::fork() };
        if child == -1 {
            return Err(format!(
                "cannot fork child launcher: {}",
                std::io::Error::last_os_error()
            ));
        }
        if child == 0 {
            if unsafe { libc::setsid() } == -1 {
                return Err(format!(
                    "cannot isolate child launcher: {}",
                    std::io::Error::last_os_error()
                ));
            }
            return Ok(None);
        }
        let mut forwarded = false;
        loop {
            let mut status = 0;
            let waited = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
            if waited == child {
                return Ok(Some(if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status)
                } else {
                    128 + libc::WTERMSIG(status)
                }));
            }
            if waited == -1 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(format!("cannot wait for child launcher: {error}"));
                }
            }
            // Preserve ordinary SIGINT/SIGTERM cancellation of the waiting CLI.
            // This is our unreaped fork child, never a PID from persisted state.
            if !forwarded && crate::CANCELLED.load(std::sync::atomic::Ordering::SeqCst) {
                unsafe {
                    libc::kill(child, libc::SIGTERM);
                }
                forwarded = true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    #[cfg(windows)]
    {
        let name = runtime["backend_job"].as_str().ok_or(
            "coordinator lacks an owned backend job; interrupt/resume it with current Team",
        )?;
        if !crate::native::windows::in_job(name)
            .map_err(|_| "cannot verify launcher job membership")?
        {
            return Ok(None);
        }
        use std::os::windows::process::CommandExt;
        let status =
            Command::new(std::env::current_exe().map_err(|_| "cannot locate Team executable")?)
                .args(args)
                .creation_flags(
                    crate::native::windows::CREATE_BREAKAWAY_FROM_JOB
                        | crate::native::windows::CREATE_NEW_PROCESS_GROUP,
                )
                .status()
                .map_err(|_| "cannot break child launcher away from coordinator job")?;
        Ok(Some(status.code().unwrap_or(1)))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::io::{BufRead, Write};
    #[test]
    fn windows_breakaway_helper() {
        if std::env::var_os("TEAM_TEST_JOB_HELPER").is_none() {
            return;
        }
        let name = std::env::var("TEAM_TEST_JOB_NAME").unwrap_or_else(|_| {
            let mut value = String::new();
            std::io::stdin().read_line(&mut value).unwrap();
            value.trim().to_owned()
        });
        // Only this isolated test subprocess changes its environment.
        unsafe {
            std::env::set_var("TEAM_TEST_JOB_NAME", &name);
        }
        let args: Vec<_> = std::env::args().skip(1).collect();
        if let Some(code) =
            isolate_launcher(&args, &serde_json::json!({"backend_job":name})).unwrap()
        {
            assert_eq!(code, 0);
            return;
        }
        println!("OUTSIDE {}", std::process::id());
        std::io::stdout().flush().unwrap();
        let mut value = String::new();
        std::io::stdin().read_line(&mut value).unwrap();
        assert_eq!(value.trim(), "stop");
        println!("EXITED");
        std::io::stdout().flush().unwrap();
    }
    #[test]
    fn windows_job_stops_owned_proxy_but_not_breakaway_launcher() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "process::tests::windows_breakaway_helper",
                "--nocapture",
            ])
            .env("TEAM_TEST_JOB_HELPER", "1")
            .env_remove("TEAM_TEST_JOB_NAME")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        let (mut child, scope) = spawn(&mut command).unwrap();
        let mut input = child.stdin.take().unwrap();
        writeln!(input, "{}", scope.job_name().unwrap()).unwrap();
        let output = child.stdout.take().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(output).lines() {
                if send.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        loop {
            let line = receive
                .recv_timeout(std::time::Duration::from_secs(8))
                .unwrap();
            if line.contains("OUTSIDE ") {
                break;
            }
        }
        let job_name = scope.job_name().unwrap().to_owned();
        let session = crate::native::windows::session_id(child.id()).unwrap();
        assert!(crate::native::windows::job_empty(&job_name, session.wrapping_add(1)).is_err());
        assert!(!crate::native::windows::job_empty(&job_name, session).unwrap());
        scope.terminate();
        assert!(!scope.wait(&mut child).unwrap().success());
        // The breakaway launcher is still alive but no longer belongs to this job.
        assert!(crate::native::windows::job_empty(&job_name, session).unwrap());
        drop(scope);
        assert!(crate::native::windows::job_empty(&job_name, session).unwrap());
        writeln!(input, "stop").unwrap();
        drop(input);
        loop {
            let line = receive
                .recv_timeout(std::time::Duration::from_secs(8))
                .unwrap();
            if line.contains("EXITED") {
                break;
            }
        }
        reader.join().unwrap();
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };

    // Only a fresh test subprocess calls this primitive. No Team CLI identity,
    // context, admission or model is synthesized by these process-lifetime tests.
    #[test]
    fn launcher_helper() {
        let Some(path) = std::env::var_os("TEAM_TEST_LAUNCHER_FIXTURE") else {
            return;
        };
        let path = PathBuf::from(path);
        unsafe {
            libc::signal(
                libc::SIGTERM,
                crate::cancel_worker as *const () as libc::sighandler_t,
            );
            libc::signal(
                libc::SIGINT,
                crate::cancel_worker as *const () as libc::sighandler_t,
            );
        }
        let initial_pid = std::process::id();
        let initial_group = unsafe { libc::getpgrp() };
        if let Some(code) = isolate_launcher(&[], &serde_json::Value::Null).unwrap() {
            std::process::exit(code);
        }
        let owner = fs::File::create(path.join("run.lock")).unwrap();
        owner.lock().unwrap();
        let member = path.file_name().unwrap().to_str().unwrap();
        crate::lifecycle::save_json(
            &path.join("tree.json"),
            &serde_json::json!({"version":1,"epoch":0,"blocked":false,
                "members":{member:{"revision":1,"active":true,"blocked":false}}}),
        )
        .unwrap();
        let watcher = crate::subteam::Watcher::start(
            &path,
            &serde_json::json!({"parent_run_id":null,"subtree_epoch":0}),
        )
        .unwrap();
        let (mut backend, scope) = spawn(Command::new("/bin/sleep").arg("10")).unwrap();
        let ready = serde_json::json!({"initial_pid":initial_pid,"initial_group":initial_group,
            "launcher":std::process::id(),
            "group":unsafe {libc::getpgrp()},"session":unsafe {libc::getsid(0)},
            "parent":unsafe {libc::getppid()},"backend":backend.id()});
        crate::lifecycle::save_json(&path.join("ready.json"), &ready).unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        while !path.join("cancel").exists()
            && !crate::CANCELLED.load(Ordering::SeqCst)
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let cancelled = path.join("cancel").exists() || crate::CANCELLED.load(Ordering::SeqCst);
        scope.terminate();
        assert!(!scope.wait(&mut backend).unwrap().success());
        crate::lifecycle::save_json(
            &path.join("terminal.json"),
            &serde_json::json!({"cancelled":cancelled,"backend_stopped":true}),
        )
        .unwrap();
        drop(watcher);
        drop(owner);
        std::process::exit(23);
    }

    struct Helper {
        path: PathBuf,
        child: Child,
        scope: Scope,
    }
    impl Helper {
        fn start() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "team-launcher-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "process::unix_tests::launcher_helper",
                    "--nocapture",
                ])
                .env("TEAM_TEST_LAUNCHER_FIXTURE", &path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            // Same initial session/group ownership as Codex's pipe command manager.
            let (child, scope) = spawn(&mut command).unwrap();
            Self { path, child, scope }
        }
        fn wait_file(&self, name: &str) -> bool {
            let deadline = Instant::now() + Duration::from_secs(4);
            while !self.path.join(name).exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            self.path.join(name).exists()
        }
        fn ready(&self) -> serde_json::Value {
            assert!(
                self.wait_file("ready.json"),
                "helper did not become ready: {}",
                self.path.display()
            );
            crate::lifecycle::read_json(&self.path.join("ready.json")).unwrap()
        }
    }
    impl Drop for Helper {
        fn drop(&mut self) {
            let _ = fs::write(self.path.join("cancel"), "cancel");
            self.scope.terminate();
            let _ = self.scope.wait(&mut self.child);
        }
    }

    #[test]
    fn unix_launcher_survives_command_group_cleanup_and_finalizes_owned_backend() {
        let mut helper = Helper::start();
        let ready = helper.ready();
        // A sibling/unrelated scope must neither die with this command nor be
        // touched by the isolated launcher's own backend cleanup.
        let mut sibling = Helper::start();
        let sibling_ready = sibling.ready();
        helper.scope.terminate(); // Codex kills its cached, still-owned command PGID.
        assert!(!helper.scope.wait(&mut helper.child).unwrap().success());
        let owner = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(helper.path.join("run.lock"))
            .unwrap();
        let owner_survived = matches!(owner.try_lock(), Err(std::fs::TryLockError::WouldBlock));
        let tree_path = helper.path.join("tree.json");
        let mut tree = crate::lifecycle::read_json(&tree_path).unwrap();
        tree["blocked"] = serde_json::json!(true);
        crate::lifecycle::save_json(&tree_path, &tree).unwrap();
        let finalized = helper.wait_file("terminal.json");
        eprintln!(
            "tracked command={}, ready={ready}, owner_survived={owner_survived}, finalized={finalized}",
            helper.child.id()
        );
        assert!(
            owner_survived && finalized,
            "command cleanup killed the child owner before lifecycle finalization"
        );
        let terminal = crate::lifecycle::read_json(&helper.path.join("terminal.json")).unwrap();
        assert_eq!(
            terminal,
            serde_json::json!({"cancelled":true,"backend_stopped":true})
        );
        assert_ne!(ready["launcher"], helper.child.id());
        assert_eq!(ready["group"], ready["launcher"]);
        assert_eq!(ready["session"], ready["launcher"]);
        assert_eq!(
            unsafe { libc::kill(-(ready["backend"].as_i64().unwrap() as i32), 0) },
            -1
        );
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            match owner.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(error) => panic!("owner did not finalize: {error}"),
            }
        }
        assert!(!sibling.path.join("terminal.json").exists());
        assert_eq!(
            unsafe { libc::kill(sibling_ready["launcher"].as_i64().unwrap() as i32, 0) },
            0
        );
        assert_eq!(
            unsafe { libc::kill(sibling_ready["backend"].as_i64().unwrap() as i32, 0) },
            0
        );
        fs::write(sibling.path.join("cancel"), "own cancellation").unwrap();
        assert_eq!(
            sibling.scope.wait(&mut sibling.child).unwrap().code(),
            Some(23)
        );
    }

    // A native parent stands in for the backend group, without Team identity or
    // authentication. Its child inherits that group exactly as legacy launchers do.
    #[test]
    fn inherited_group_parent_helper() {
        let Some(path) = std::env::var_os("TEAM_TEST_INHERITED_GROUP_FIXTURE") else {
            return;
        };
        let path = PathBuf::from(path);
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "process::unix_tests::launcher_helper",
                "--nocapture",
            ])
            .env("TEAM_TEST_LAUNCHER_FIXTURE", &path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        crate::lifecycle::save_json(
            &path.join("parent.json"),
            &serde_json::json!({
            "pid":std::process::id(),"group":unsafe {libc::getpgrp()},"child":child.id()}),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        while !path.join("exit-parent").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        // Deliberately do not wait for or stop the legitimate child, matching a
        // target-only backend exit. The test later closes the child's own gate.
        std::process::exit(0);
    }

    #[test]
    fn unix_inherited_group_launcher_does_not_retain_parent_backend_scope() {
        struct Parent {
            path: PathBuf,
            child: Child,
        }
        impl Drop for Parent {
            fn drop(&mut self) {
                let _ = fs::write(self.path.join("cancel"), "own test cleanup");
                let _ = fs::write(self.path.join("exit-parent"), "exit");
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
        let path = std::env::temp_dir().join(format!(
            "team-inherited-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "process::unix_tests::inherited_group_parent_helper",
                "--nocapture",
            ])
            .env("TEAM_TEST_INHERITED_GROUP_FIXTURE", &path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let mut parent = Parent {
            path,
            child: command.spawn().unwrap(),
        };
        let wait_file = |name: &str| {
            let deadline = Instant::now() + Duration::from_secs(4);
            while !parent.path.join(name).exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(parent.path.join(name).exists(), "missing {name}");
        };
        wait_file("ready.json");
        wait_file("parent.json");
        let ready = crate::lifecycle::read_json(&parent.path.join("ready.json")).unwrap();
        let identity = crate::lifecycle::read_json(&parent.path.join("parent.json")).unwrap();
        assert_eq!(identity["group"], parent.child.id());
        assert_eq!(ready["initial_group"], identity["group"]);
        assert_eq!(ready["initial_pid"], identity["child"]);
        assert_ne!(ready["initial_pid"], ready["initial_group"]);
        fs::write(parent.path.join("exit-parent"), "target-only parent exit").unwrap();
        // Reap only the parent. Scope::wait would itself kill the whole original
        // group and mask precisely the proxy-retention defect being checked.
        assert!(parent.child.wait().unwrap().success());
        let group_exists = unsafe { libc::kill(-(parent.child.id() as i32), 0) } == 0;
        let group_error = std::io::Error::last_os_error();
        eprintln!("inherited parent={identity}, ready={ready}, parent_group_exists={group_exists}");
        assert!(
            !group_exists,
            "waiting launcher proxy kept the dead backend group nonempty"
        );
        assert_eq!(group_error.raw_os_error(), Some(libc::ESRCH));
        assert_eq!(ready["launcher"], ready["initial_pid"]);
        assert_eq!(ready["group"], ready["launcher"]);
        assert_eq!(ready["session"], ready["launcher"]);
        assert_eq!(
            unsafe { libc::kill(ready["launcher"].as_i64().unwrap() as i32, 0) },
            0
        );
        assert_eq!(
            unsafe { libc::kill(ready["backend"].as_i64().unwrap() as i32, 0) },
            0
        );
        let owner = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(parent.path.join("run.lock"))
            .unwrap();
        assert!(matches!(
            owner.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        assert!(!parent.path.join("terminal.json").exists());
        let tree_path = parent.path.join("tree.json");
        let mut tree = crate::lifecycle::read_json(&tree_path).unwrap();
        tree["blocked"] = serde_json::json!(true);
        crate::lifecycle::save_json(&tree_path, &tree).unwrap();
        wait_file("terminal.json");
        assert_eq!(
            crate::lifecycle::read_json(&parent.path.join("terminal.json")).unwrap(),
            serde_json::json!({"cancelled":true,"backend_stopped":true})
        );
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            match owner.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(error) => panic!("owner did not finalize: {error}"),
            }
        }
        assert_eq!(
            unsafe { libc::kill(-(ready["backend"].as_i64().unwrap() as i32), 0) },
            -1
        );
    }

    #[test]
    fn unix_launcher_proxy_preserves_cancellation_and_exit_status() {
        let mut helper = Helper::start();
        helper.ready();
        assert_eq!(
            unsafe { libc::kill(helper.child.id() as i32, libc::SIGTERM) },
            0
        );
        assert_eq!(
            helper.scope.wait(&mut helper.child).unwrap().code(),
            Some(23)
        );
        let terminal = crate::lifecycle::read_json(&helper.path.join("terminal.json")).unwrap();
        assert_eq!(
            terminal,
            serde_json::json!({"cancelled":true,"backend_stopped":true})
        );
    }
}
