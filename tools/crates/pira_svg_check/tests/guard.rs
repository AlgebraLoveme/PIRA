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
fn closed_panel_boundary_crossing_text_edge_warns() {
    assert!(warning_codes("boundary_through_text.svg").contains(&"stroke-intrusion".to_string()));
}

#[test]
fn open_line_crossing_text_edge_warns() {
    assert!(warning_codes("line_at_text_edge.svg").contains(&"stroke-intrusion".to_string()));
}

#[test]
fn padded_stroked_panel_does_not_warn() {
    assert!(!warning_codes("padded_stroked_panel.svg").contains(&"stroke-intrusion".to_string()));
}

#[test]
fn opaque_backing_hides_line() {
    let source = std::fs::read_to_string(fixture("backed_label.svg")).unwrap();
    for family in ["sans-serif", "monospace"] {
        let source = source.replace("sans-serif", family);
        assert!(
            !analyze_svg(&source)
                .warnings
                .iter()
                .any(|w| w.code == "stroke-intrusion"),
            "{family}"
        );
        // Removing only the occluder must restore real stroke detection. This
        // also prevents a missing-font/no-glyph run from passing vacuously.
        let exposed = source
            .lines()
            .filter(|line| !line.contains("id=\"label-backing\""))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            analyze_svg(&exposed)
                .warnings
                .iter()
                .any(|w| w.code == "stroke-intrusion"),
            "{family}"
        );
    }
}

#[test]
fn low_contrast_warns() {
    assert!(warning_codes("low_contrast.svg").contains(&"low-contrast".to_string()));
}

#[test]
fn faint_text_is_not_silently_skipped() {
    assert!(warning_codes("faint_text.svg").contains(&"low-contrast".to_string()));
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

// Fixtures live under TMPDIR when supplied by the test runner.
fn analyze_svg(source: &str) -> pira_svg_check::Report {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "pira-svg-{}-{}.svg",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, source).unwrap();
    let result = analyze_file(&path, &Config::default());
    std::fs::remove_file(path).unwrap();
    result.expect("SVG should analyze")
}

#[test]
fn stylesheet_loss_is_attributed_without_inactive_rule_noise() {
    for (rule, target, width, clipped, absent) in [
        ("text {clip-path:url(#cut)}", "", 55, true, false),
        ("text {clip-path:url(#cut)}", "", 1, true, true),
        ("g {mask:url(#shade)}", "", 55, true, false),
        ("g {mask:url(#shade)!important}", "", 55, true, false),
        ("g {clip-path:url(#cut)!important}", "", 55, true, false),
        ("g {mask:url(#shade)!important}", "", 1, true, true),
        ("g {mask:url(#shade)!important}", "", 300, false, false),
        (".unused {clip-path:url(#cut)}", "", 1, false, false),
        ("text {clip-path:url(#cut)}", "", 300, false, false),
        ("text {display:none}", "", 1, false, true),
        ("", "clip-path='url(#cut)'", 1, true, true),
    ] {
        let report = analyze_svg(&format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='300' height='80'><defs><clipPath id='cut'><rect width='{width}' height='80'/></clipPath><mask id='shade' maskUnits='userSpaceOnUse' x='0' y='0' width='300' height='80'><rect width='{width}' height='80' fill='white'/></mask></defs><style>{rule}</style><g><text id='label' x='20' y='45' font-size='24' {target}>Visible label</text></g></svg>"
        ));
        for code in ["text-clipped", "text-not-rendered"] {
            let expected = if code == "text-clipped" {
                clipped
            } else {
                absent
            };
            assert_eq!(
                report.warnings.iter().any(|w| w.code == code),
                expected,
                "{rule} {target} {width}: {report:?}"
            );
        }
        assert!(
            report
                .warnings
                .iter()
                .all(|w| w.text_element.as_deref() == Some("<text#label>"))
        );
    }
}

#[test]
fn inline_clipping_declarations_are_not_inert_strings_or_comments() {
    for (style, expected) in [
        ("font-family:'clip-path:url(#cut)';fill:black", false),
        ("/* mask:url(#cut) */fill:black", false),
        ("clip-path:none;clip-path:url(#cut)", true),
        ("clip-path/**/:url(#cut)", true),
        ("clip-path&#9;:url(#cut)", true),
    ] {
        let report = analyze_svg(&format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='300' height='80'><defs><clipPath id='cut'><rect width='55' height='80'/></clipPath></defs><text x='20' y='45' font-size='24' style=\"{style}\">Visible label</text></svg>"
        ));
        assert_eq!(
            report.warnings.iter().any(|w| w.code == "text-clipped"),
            expected,
            "{style}: {report:?}"
        );
    }
}

#[test]
fn foreign_metadata_cannot_shadow_identity_membership_or_rendering() {
    for attrs in [
        "m:id='wrong' m:data-pira-svg-check-key='wrong' m:mask='metadata' m:style='fill:none'",
        "m:fill='none' m:opacity='0' m:clip-path='metadata'",
    ] {
        let report = analyze_svg(&format!(
            "<svg xmlns='http://www.w3.org/2000/svg' xmlns:m='urn:metadata' width='300' height='80'><text {attrs} id='label' x='20' y='45' font-size='24' fill='#ddd'>Visible label</text></svg>"
        ));
        assert_eq!(report.warnings.len(), 1, "{report:?}");
        assert_eq!(report.warnings[0].code, "low-contrast");
        assert_eq!(
            report.warnings[0].text_element.as_deref(),
            Some("<text#label>")
        );
    }
}

#[test]
fn content_derived_canvas_survives_variant_removal() {
    let report = analyze_svg(
        "<svg xmlns='http://www.w3.org/2000/svg'><rect x='0' y='0' width='300' height='80' fill='white'/><text id='label' x='20' y='45' font-size='24' fill='#ddd'>Visible label</text><path d='M20 36H150' stroke='black'/></svg>",
    );
    assert!(
        report.warnings.iter().any(|w| w.code == "low-contrast"),
        "{report:?}"
    );
    assert!(
        report.warnings.iter().any(|w| w.code == "stroke-intrusion"),
        "{report:?}"
    );
}

#[cfg(unix)]
#[test]
fn native_font_directory_path_is_accepted() {
    use std::os::unix::ffi::OsStringExt;
    let path = std::env::temp_dir().join(std::ffi::OsString::from_vec(
        format!("pira-svg-font-{}-", std::process::id())
            .into_bytes()
            .into_iter()
            .chain([255])
            .collect(),
    ));
    // macOS filesystems reject invalid UTF-8 names. Still exercise native argv
    // handling there, like any nonexistent optional font directory; other Unix
    // platforms additionally exercise an existing directory with that name.
    #[cfg(not(target_os = "macos"))]
    std::fs::create_dir(&path).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_pira_svg_check"))
        .arg("--font-dir")
        .arg(&path)
        .arg(fixture("clear.svg"))
        .output()
        .unwrap();
    #[cfg(not(target_os = "macos"))]
    std::fs::remove_dir(&path).unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("clear"));
}

#[test]
fn namespace_metadata_does_not_change_visible_text() {
    for (extra, attrs) in [
        ("", "xml:style='fill:none'"),
        (
            "",
            "xmlns:l='http://www.w3.org/1999/xlink' l:style='fill:none' l:fill='none'",
        ),
        ("", "xmlns:m='urn:meta' m:href='https://example.invalid'"),
        ("<m:text xmlns:m='urn:meta'>metadata</m:text>", ""),
        (
            "<m:container xmlns:m='urn:meta'><text>metadata</text></m:container>",
            "",
        ),
        ("<style xmlns='urn:meta'>text {fill:none}</style>", ""),
        (
            "<m:style xmlns:m='urn:meta'><![CDATA[text {fill:url(https://example.invalid)}]]></m:style>",
            "",
        ),
    ] {
        let source = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='300' height='80'>{extra}<text id='label' x='20' y='45' font-size='24' {attrs}>Visible label</text></svg>"
        );
        let report = analyze_svg(&source);
        assert!(report.warnings.is_empty(), "{source}: {report:?}");
    }
}

#[test]
fn prefixed_svg_and_alternate_xlink_keep_rendering() {
    let report = analyze_svg(
        "<s:svg xmlns:s='http://www.w3.org/2000/svg' xmlns:l='http://www.w3.org/1999/xlink' width='300' height='80'><s:defs><s:rect id='back' width='300' height='80' fill='white'/></s:defs><s:use l:href='#back'/><s:text id='label' x='20' y='45' font-size='24' xml:space='preserve'>Visible label</s:text></s:svg>",
    );
    assert!(report.warnings.is_empty(), "{report:?}");
}

#[test]
fn important_author_declarations_cannot_defeat_text_variants() {
    for (rules, attrs, expected) in [
        ("text {display:inline!important}", "", None),
        ("* {display:inline!important}", "", None),
        ("", "style='display:inline!important'", None),
        (
            "text {stroke:black!important;stroke-width:5}",
            "fill='none'",
            Some("text-not-rendered"),
        ),
        (
            "",
            "fill='none' style='stroke:black!important;stroke-width:5'",
            Some("text-not-rendered"),
        ),
        (
            "g {fill:#ddd} text {fill:inherit}",
            "",
            Some("low-contrast"),
        ),
    ] {
        let source = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='300' height='80'><style>{rules}</style><g><text id='label' x='20' y='45' font-size='24' {attrs}>Visible label</text></g></svg>"
        );
        let report = analyze_svg(&source);
        let codes: Vec<_> = report
            .warnings
            .iter()
            .map(|warning| warning.code.as_str())
            .collect();
        assert_eq!(
            codes,
            expected.into_iter().collect::<Vec<_>>(),
            "{source}: {report:?}"
        );
    }
}

#[test]
fn referenced_image_under_foreign_parent_cannot_read_external_png() {
    let directory = std::env::temp_dir().join(format!("pira-svg-resource-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let image = directory.join("harmless-red.png");
    let mut pixels = resvg::tiny_skia::Pixmap::new(10, 10).unwrap();
    pixels.fill(resvg::tiny_skia::Color::from_rgba8(255, 0, 0, 255));
    pixels.save_png(&image).unwrap();
    let input = directory.join("input.svg");
    let image_href = quick_xml::escape::escape(image.to_str().unwrap());
    for (container, attribute) in [
        ("g", "href"),
        ("m:container", "href"),
        ("m:container", "l:href"),
    ] {
        let source = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' xmlns:m='urn:metadata' xmlns:l='http://www.w3.org/1999/xlink' width='100' height='50'><{container}><image id='external' {attribute}=\"{image_href}\" width='10' height='10'/></{container}><use href='#external'/><text x='20' y='30' font-size='14'>OK</text></svg>"
        );
        std::fs::write(&input, source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_pira_svg_check"))
            .arg("--json")
            .arg(&input)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{container} {attribute}");
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("external SVG resource"));
    }
    std::fs::remove_file(input).unwrap();
    std::fs::remove_file(image).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn resolved_instances_match_inline_text_fill_and_stroke() {
    let wrap = |body: &str| {
        format!("<svg xmlns='http://www.w3.org/2000/svg' width='320' height='160'>{body}</svg>")
    };
    let codes = |source: &str| {
        let mut codes: Vec<_> = analyze_svg(source)
            .warnings
            .into_iter()
            .map(|w| w.code)
            .collect();
        codes.sort();
        codes
    };
    let label = "<text x='20' y='45' font-size='24' fill='#ddd'>Visible label</text>";
    let inline = wrap(&format!(
        "<g transform='translate(5 5)'>{label}</g><g transform='translate(5 75)'>{label}</g>"
    ));
    let referenced = wrap(&format!(
        "<defs><g id='label'>{label}</g><g id='nested'><use href='#label'/></g></defs><use href='#nested' transform='translate(5 5)'/><use href='#nested' transform='translate(5 75)'/>"
    ));
    assert_eq!(codes(&inline), vec!["low-contrast", "low-contrast"]);
    assert_eq!(codes(&inline), codes(&referenced));
    let report = analyze_svg(&referenced);
    assert_ne!(
        report.warnings[0].text_element,
        report.warnings[1].text_element
    );

    let inherited = label.replace("fill='#ddd'", "fill='inherit'");
    let independent = wrap(&format!(
        "<defs><g id='label'>{inherited}</g></defs><use href='#label' fill='#ddd'/><use href='#label' fill='black' y='75'/>"
    ));
    assert_eq!(codes(&independent), vec!["low-contrast"]);

    let black = label.replace("#ddd", "black");
    for (paint, stroked) in [
        ("fill='black'", false),
        ("fill='none' stroke='black' stroke-width='2'", true),
    ] {
        let shape = format!("<rect id='shape' x='0' y='30' width='260' height='3' {paint}/>");
        let inline = wrap(&format!("{shape}{black}"));
        let referenced = wrap(&format!("<defs>{shape}</defs><use href='#shape'/>{black}"));
        assert_eq!(codes(&inline), codes(&referenced), "{paint}");
        assert_eq!(
            codes(&referenced).iter().any(|c| c == "stroke-intrusion"),
            stroked
        );
    }
}

#[test]
fn masks_keep_scene_backed_dependencies_and_only_warn_on_loss() {
    for scene_backed in [false, true] {
        for width in [55, 300] {
            let shape = format!("<rect id='back' width='{width}' height='80' fill='white'/>");
            let source = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='300' height='80'>{}<defs>{}<mask id='m' maskUnits='userSpaceOnUse' x='0' y='0' width='300' height='80'><use href='#back'/></mask></defs><text x='20' y='45' font-size='24' mask='url(#m)'>Visible label</text></svg>",
                if scene_backed { &shape } else { "" },
                if scene_backed { "" } else { &shape }
            );
            let report = analyze_svg(&source);
            assert_eq!(
                report.warnings.iter().any(|w| w.code == "text-clipped"),
                width == 55,
                "{report:?}"
            );
            assert!(
                !report
                    .warnings
                    .iter()
                    .any(|w| w.code == "text-not-rendered"),
                "{report:?}"
            );
        }
    }
}

#[test]
fn clipping_loss_is_independent_of_declaration_location_and_references() {
    for referenced in [false, true] {
        for (attrs, rules, width, loss) in [
            ("clip-path='url(#c)'", "", 300, false),
            ("clip-path='url(#c)'", "g {clip-path:none}", 1, false),
            ("style='clip-path:url(#c);clip-path:none'", "", 1, false),
            ("style='clip-path:url(#c)'", "", 55, true),
            ("", "g {clip-path:url(#c)!important}", 55, true),
        ] {
            let text = "<text x='20' y='45' font-size='24'>Visible label</text>";
            let body = if referenced {
                format!("<defs><g id='label'>{text}</g></defs><use href='#label'/>")
            } else {
                text.to_string()
            };
            let source = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='300' height='80'><defs><clipPath id='c'><rect width='{width}' height='80'/></clipPath></defs><style>{rules}</style><g {attrs}>{body}</g></svg>"
            );
            let report = analyze_svg(&source);
            assert_eq!(
                report.warnings.iter().any(|w| w.code == "text-clipped"),
                loss,
                "{source}: {report:?}"
            );
        }
    }
}

#[test]
fn isolated_fonts_require_inventory_and_report_missing_selection_and_glyphs() {
    use resvg::usvg::fontdb::Database;
    let directory = std::env::temp_dir().join(format!("pira-svg-fonts-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("input.svg");
    let svg = |family: &str, text: &str| {
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='300' height='80'><text x='20' y='45' font-size='24' font-family='{family}'>{text}</text></svg>"
        )
    };
    let config = Config {
        isolated_fonts: true,
        font_dirs: vec![directory.clone()],
        ..Config::default()
    };
    std::fs::write(&path, svg("MissingPiraFont", "Hello")).unwrap();
    assert!(
        analyze_file(&path, &config)
            .unwrap_err()
            .to_string()
            .contains("missing font family")
    );
    // Copy one real font into the isolated set; production never loads system fonts
    // in this mode. Like the existing rendering tests, this test needs host fonts.
    let mut database = Database::new();
    database.load_system_fonts();
    let face = database
        .faces()
        .next()
        .expect("render tests require a system font");
    let family = face.families[0].0.clone();
    let bytes = database
        .with_face_data(face.id, |data, _| data.to_vec())
        .unwrap();
    std::fs::write(directory.join("font.ttf"), bytes).unwrap();
    let mut isolated_database = Database::new();
    isolated_database.load_fonts_dir(&directory);
    let host_only = database
        .faces()
        .flat_map(|face| &face.families)
        .find(|(name, _)| {
            !isolated_database
                .faces()
                .any(|face| face.families.iter().any(|(isolated, _)| isolated == name))
        })
        .expect("system inventory must contain another font family")
        .0
        .clone();
    std::fs::write(&path, svg(&quick_xml::escape::escape(&host_only), "Hello")).unwrap();
    assert!(analyze_file(&path, &Config::default()).is_ok());
    assert!(
        analyze_file(&path, &config)
            .unwrap_err()
            .to_string()
            .contains("missing font family")
    );
    let family = quick_xml::escape::escape(&family);
    std::fs::write(&path, svg(&family, "Hello")).unwrap();
    assert!(analyze_file(&path, &config).is_ok());
    std::fs::write(&path, svg(&family, "Hello &#x10FFFF;")).unwrap();
    assert!(
        analyze_file(&path, &config)
            .unwrap_err()
            .to_string()
            .contains("missing glyph U+10FFFF")
    );
    std::fs::write(&path, svg("MissingPiraFont", "Hello")).unwrap();
    assert!(
        analyze_file(&path, &config)
            .unwrap_err()
            .to_string()
            .contains("missing font family")
    );
    let result = Command::new(env!("CARGO_BIN_EXE_pira_svg_check"))
        .args(["--isolated-fonts"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("--font-dir"));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn reference_serialization_loss_fails_visibly() {
    let path = std::env::temp_dir().join(format!("pira-svg-precision-{}.svg", std::process::id()));
    // The renderer can paint this geometry, but the SVG writer's decimal
    // precision collapses it. Returning no-semantic-text would hide that loss.
    std::fs::write(&path, "<svg xmlns='http://www.w3.org/2000/svg' width='300' height='100' viewBox='0 0 0.000000003 0.000000001'><defs><rect id='r' width='0.000000001' height='0.000000001'/></defs><use href='#r'/></svg>").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_pira_svg_check"))
        .arg(&path)
        .output()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("unsupported scene analysis"));
}

#[test]
fn object_bounds_effects_preserve_sibling_geometry_before_isolation() {
    for mask in [false, true] {
        for (width, loss) in [(0.5, false), (0.1, true)] {
            for extra in ["", "<defs><g id='empty'/></defs><use href='#empty'/>"] {
                let (definition, effect) = if mask {
                    (
                        format!(
                            "<mask id='c' maskUnits='objectBoundingBox' maskContentUnits='objectBoundingBox' x='0' y='0' width='1' height='1'><rect width='{width}' height='1' fill='white'/></mask>"
                        ),
                        "mask",
                    )
                } else {
                    (
                        format!(
                            "<clipPath id='c' clipPathUnits='objectBoundingBox'><rect width='{width}' height='1'/></clipPath>"
                        ),
                        "clip-path",
                    )
                };
                let source = format!(
                    "<svg xmlns='http://www.w3.org/2000/svg' width='300' height='120'><defs>{definition}</defs><g transform='translate(5 5)'><g {effect}='url(#c)'><text x='20' y='50' font-size='24'>Hello</text><rect x='200' width='50' height='80'/></g></g>{extra}</svg>"
                );
                let report = analyze_svg(&source);
                let codes: Vec<_> = report
                    .warnings
                    .iter()
                    .map(|warning| warning.code.as_str())
                    .collect();
                assert_eq!(
                    codes,
                    if loss { vec!["text-clipped"] } else { vec![] },
                    "{source}: {report:?}"
                );
            }
        }
    }
}

// These two real fonts reproduce usvg's glyph-count-changing fallback. Keep
// this platform fixture explicit rather than silently substituting other fonts.
#[cfg(target_os = "macos")]
#[test]
fn isolated_final_shaping_rejects_unresolved_complex_clusters() {
    let directory = std::env::temp_dir().join(format!("pira-svg-shaping-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    for name in ["Georgia.ttf", "Arial Unicode.ttf"] {
        std::fs::copy(
            Path::new("/System/Library/Fonts/Supplemental").join(name),
            directory.join(name),
        )
        .expect(
            "mixed-script regression requires macOS Georgia and Arial Unicode supplemental fonts",
        );
    }
    let path = directory.join("input.svg");
    let config = Config {
        isolated_fonts: true,
        font_dirs: vec![directory.clone()],
        ..Config::default()
    };
    for content in ["لا", "क्ष", "ffi 汉"] {
        let svg = |text: &str| {
            format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='300' height='100'><text x='20' y='50' font-size='24' font-family='Georgia'>{text}</text></svg>"
            )
        };
        std::fs::write(&path, svg(content)).unwrap();
        assert!(analyze_file(&path, &config).is_ok(), "covered {content}");
        std::fs::write(&path, svg(&format!("{content} &#x10FFFF;"))).unwrap();
        let error = analyze_file(&path, &config).unwrap_err().to_string();
        assert!(
            error.contains("isolated fonts:")
                && error.contains("glyph")
                && error.contains("--font-dir"),
            "{content}: {error}"
        );
    }
    // Text in paint/dependency subtrees must be validated too, not only labels.
    std::fs::write(&path, "<svg xmlns='http://www.w3.org/2000/svg' width='300' height='100'><defs><pattern id='p' patternUnits='userSpaceOnUse' width='300' height='100'><text x='20' y='50' font-size='24' font-family='Georgia'>لا &#x10FFFF;</text></pattern></defs><rect width='300' height='100' fill='url(#p)'/></svg>").unwrap();
    assert!(
        analyze_file(&path, &config)
            .unwrap_err()
            .to_string()
            .contains("unresolved shaped glyph")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn fragment_images_never_fall_back_to_filesystem_paths() {
    let directory = std::env::temp_dir().join(format!("pira-svg-fragment-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    std::fs::create_dir(directory.join("#anchor")).unwrap();
    let backing = "<svg xmlns='http://www.w3.org/2000/svg' width='300' height='80'><rect width='300' height='80' fill='black'/></svg>";
    for name in ["#back.svg", "ordinary.svg"] {
        std::fs::write(directory.join(name), backing).unwrap();
    }
    // POSIX filenames can also masquerade as malformed data URLs.
    #[cfg(unix)]
    std::fs::write(directory.join("data:back.svg"), backing).unwrap();
    let input = directory.join("input.svg");
    let hrefs = [
        "#back.svg",
        "#anchor/../ordinary.svg",
        #[cfg(unix)]
        "data:back.svg",
    ];
    for href in hrefs {
        for resource in [
            format!("<image href='{href}' width='300' height='80'/>"),
            format!(
                "<defs><filter id='f' x='0' y='0' width='300' height='80' filterUnits='userSpaceOnUse'><feImage href='{href}'/></filter></defs><rect width='300' height='80' filter='url(#f)'/>"
            ),
        ] {
            let source = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='300' height='80'>{resource}<text x='20' y='45' font-family='sans-serif' font-size='24'>Visible label</text></svg>"
            );
            std::fs::write(&input, &source).unwrap();
            let output = Command::new(env!("CARGO_BIN_EXE_pira_svg_check"))
                .current_dir(&directory)
                .arg("--json")
                .arg(&input)
                .output()
                .unwrap();
            assert!(output.status.success(), "{href}: {:?}", output);
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            // Black glyphs on the white canvas must be measurable and clear, not
            // hidden against the black filesystem image or absent due to no font.
            assert_eq!(
                report["warnings"],
                serde_json::json!([]),
                "{resource}: {report}"
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn mixed_opacity_spans_keep_geometric_contrast_coverage() {
    for family in ["sans-serif", "monospace"] {
        for (paint, low) in [
            ("fill-opacity='.1'", true),
            ("fill-opacity='.5'", true),
            ("style='fill-opacity:.1!important'", true),
            ("fill='#e5e5e5'", true),
            ("fill='black'", false),
        ] {
            for referenced in [false, true] {
                let text = format!(
                    "<text x='20' y='50' font-family='{family}' font-size='28'>Solid <tspan {paint}>Faint label</tspan></text>"
                );
                let body = if referenced {
                    format!("<defs><g id='label'>{text}</g></defs><use href='#label'/>")
                } else {
                    text
                };
                let report = analyze_svg(&format!(
                    "<svg xmlns='http://www.w3.org/2000/svg' width='500' height='100'>{body}</svg>"
                ));
                assert_eq!(
                    report.warnings.iter().any(|w| w.code == "low-contrast"),
                    low,
                    "{family} {paint} {referenced}: {report:?}"
                );
                assert!(
                    !report
                        .warnings
                        .iter()
                        .any(|w| w.code == "text-not-rendered"),
                    "{report:?}"
                );
                if !low {
                    assert!(report.warnings.is_empty(), "{report:?}");
                }
            }
        }
    }
}

#[test]
fn excessive_svg_nesting_is_a_handled_analysis_error() {
    let directory = std::env::temp_dir().join(format!("pira-svg-depth-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let input = directory.join("input.svg");
    let nested = |groups: usize| {
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='500' height='100'>{}<rect width='10' height='10'/>{}</svg>",
            "<g>".repeat(groups),
            "</g>".repeat(groups)
        )
    };
    let image = |source: &str, mime: &str| {
        let encoded: String = source.bytes().map(|byte| format!("%{byte:02X}")).collect();
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='500' height='100'><image href='data:{mime},{encoded}' width='500' height='100'/></svg>"
        )
    };
    let mut cases = vec![
        (nested(62), false),
        (nested(63), true),
        (nested(15_000), true),
    ];
    for mime in ["image/svg+xml", "text/plain"] {
        cases.push((image(&nested(15_000), mime), true));
        cases.push((image(&nested(60), mime), false));
        cases.push((image(&nested(61), mime), true));
    }
    let mut recursive = nested(0);
    for layers in 1..=5 {
        recursive = image(&recursive, "image/svg+xml");
        if layers >= 4 {
            cases.push((recursive.clone(), layers == 5));
        }
    }
    for (source, rejected) in cases {
        std::fs::write(&input, source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_pira_svg_check"))
            .arg("--json")
            .arg(&input)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(if rejected { 2 } else { 0 }),
            "{:?}",
            output
        );
        if rejected {
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("nesting limit"),
                "{:?}",
                output
            );
            assert!(output.stdout.is_empty());
        } else {
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(report["warnings"][0]["code"], "no-semantic-text");
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn mixed_opacity_coverage_preserves_mask_loss_and_occlusion() {
    for width in [180, 500] {
        let report = analyze_svg(&format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='500' height='100'><defs><mask id='m' maskUnits='userSpaceOnUse' x='0' y='0' width='500' height='100'><rect width='{width}' height='100' fill='white'/></mask></defs><text x='20' y='50' font-family='sans-serif' font-size='28' mask='url(#m)'>Solid<tspan x='180' fill-opacity='.1'>Faint label</tspan></text></svg>"
        ));
        assert_eq!(
            report.warnings.iter().any(|w| w.code == "text-clipped"),
            width == 180,
            "{report:?}"
        );
        assert_eq!(
            report.warnings.iter().any(|w| w.code == "low-contrast"),
            width == 500,
            "{report:?}"
        );
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| w.code == "text-not-rendered"),
            "{report:?}"
        );
    }
}
