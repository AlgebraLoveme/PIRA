//! Test-only executable adapter: preserve native backend identity around Python.
use std::process::Command;
fn main() {
    let python = std::env::var_os("TEAM_FIXTURE_PYTHON").expect("fixture Python");
    let script = std::env::var_os("TEAM_FIXTURE_CODEX").expect("fixture protocol peer");
    let mut command = Command::new(python);
    command.arg("-B").arg(script);
    for arg in std::env::args_os().skip(1) {
        // Windows retains the executable wrapper as backend owner. Its Python
        // child must not also advertise that owner's app-server marker.
        if cfg!(windows) && arg == "app-server" {
            command.arg("native-fixture-app-server");
        } else {
            command.arg(arg);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        panic!("fixture exec failed: {}", command.exec());
    }
    #[cfg(windows)]
    std::process::exit(
        command
            .status()
            .expect("fixture backend")
            .code()
            .unwrap_or(1),
    );
}
