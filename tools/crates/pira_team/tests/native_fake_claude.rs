//! Keep the native Claude image/marker around the deterministic interpreter peer.
use std::process::Command;

fn main() {
    let mut command = Command::new(std::env::var_os("TEAM_CLAUDE_PYTHON").unwrap());
    command
        .arg("-B")
        .arg(std::env::var_os("TEAM_CLAUDE_PEER").unwrap());
    for arg in std::env::args_os().skip(1) {
        // Only this native owner advertises the Team marker, not its interpreter.
        if arg == "--input-format" {
            command.arg("--fixture-input-format");
        } else {
            command.arg(arg);
        }
    }
    std::process::exit(command.status().unwrap().code().unwrap_or(1));
}
