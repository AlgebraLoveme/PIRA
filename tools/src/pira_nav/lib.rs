mod cancellation;
mod cli;
mod command;
mod deps;
mod discovery;
mod document;
mod help;
mod language;
mod lsp;
mod lsp_options;
mod model;
mod parse;
mod safe_walk;
mod search;
mod security;
mod semantic;
mod structural;
mod util;

pub use cli::run;

/// CLI entry point with catchable Unix cancellation and owned LSP cleanup.
/// Embedders using `run` retain control of their own signal handlers.
pub fn run_cli(args: impl IntoIterator<Item = std::ffi::OsString>) -> i32 {
    if let Err(error) = cancellation::install() {
        use std::io::Write;
        let _ = writeln!(
            std::io::stderr().lock(),
            "error: {}",
            util::sanitize_metadata(&error)
        );
        return 3;
    }
    let code = run(args);
    cancellation::exit_code().unwrap_or(code)
}
