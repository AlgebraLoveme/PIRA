use std::path::{Path, PathBuf};
use std::process::Command;

use pira_svg_check::{Config, analyze_file};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn warning_codes(name: &str) -> Vec<String> {
    analyze_file(&fixture(name), &Config::default())
        .expect("fixture should analyze")
        .warnings
        .into_iter()
        .map(|warning| warning.code)
        .collect()
}

#[test]
fn clear_text_has_no_warnings() {
    assert!(warning_codes("clear.svg").is_empty());
}

#[test]
fn line_through_word_warns_even_when_text_is_topmost() {
    assert!(warning_codes("line_through.svg").contains(&"stroke-intrusion".to_string()));
}

#[test]
fn opaque_backing_hides_line() {
    assert!(!warning_codes("backed_label.svg").contains(&"stroke-intrusion".to_string()));
}

#[test]
fn low_contrast_warns() {
    assert!(warning_codes("low_contrast.svg").contains(&"low-contrast".to_string()));
}

#[test]
fn clip_path_warns() {
    assert!(warning_codes("clipped_text.svg").contains(&"text-clipped".to_string()));
}

#[test]
fn cli_returns_success_for_warnings() {
    let output = Command::new(env!("CARGO_BIN_EXE_pira_svg_check"))
        .arg(fixture("line_through.svg"))
        .output()
        .expect("CLI should run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("stroke-intrusion"));
}
