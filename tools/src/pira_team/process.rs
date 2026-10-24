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
        if unsafe { libc::getpgrp() != libc::getpid() && libc::setsid() == -1 } {
            return Err("cannot isolate child launcher".into());
        }
        Ok(None)
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
        scope.terminate();
        assert!(!scope.wait(&mut child).unwrap().success());
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
