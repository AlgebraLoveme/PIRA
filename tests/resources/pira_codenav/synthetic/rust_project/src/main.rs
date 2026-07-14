use std::path::PathBuf;

use pira_codenav_rust_fixture::Parser;

fn main() {
    let parser = Parser::new(PathBuf::from("."));
    let _ = parser.parse("fixture");
}
