use crate::model::DecisionRecord;
use crate::util;
use std::fmt::Write;

const DOCUMENT_START: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="color-scheme" content="light dark">
  <meta name="referrer" content="no-referrer">
  <meta name="robots" content="noindex,nofollow">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'">
  <title>PIRA decision export</title>
  <style>
    :root {
      color-scheme: light dark;
      --page: #f4f6f8;
      --surface: #ffffff;
      --surface-soft: #f8fafc;
      --text: #172033;
      --muted: #5d687a;
      --line: #d8dee8;
      --accent: #2855d9;
      --accent-soft: #eaf0ff;
      --selected: #e9f8ef;
      --selected-line: #18864b;
      --warning: #8a4b08;
      --warning-soft: #fff4df;
      --shadow: 0 14px 38px rgb(28 39 60 / 9%);
      --radius: 16px;
    }
    @media (prefers-color-scheme: dark) {
      :root {
        --page: #0d121b;
        --surface: #151c27;
        --surface-soft: #1b2431;
        --text: #edf2f8;
        --muted: #aeb9c8;
        --line: #344154;
        --accent: #8cacf9;
        --accent-soft: #202f53;
        --selected: #163b2a;
        --selected-line: #62d394;
        --warning: #f3bd77;
        --warning-soft: #3c2b17;
        --shadow: 0 16px 44px rgb(0 0 0 / 28%);
      }
    }
    * { box-sizing: border-box; }
    html { scroll-behavior: smooth; }
    body {
      margin: 0;
      background: var(--page);
      color: var(--text);
      font: 16px/1.55 ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      text-rendering: optimizeLegibility;
    }
    a { color: var(--accent); text-underline-offset: .16em; }
    a:hover { text-decoration-thickness: .13em; }
    a:focus-visible, summary:focus-visible {
      outline: 3px solid var(--accent);
      outline-offset: 3px;
      border-radius: 4px;
    }
    code {
      font: .86em/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
      overflow-wrap: anywhere;
    }
    .shell { width: min(1120px, calc(100% - 32px)); margin: 0 auto; }
    .hero {
      padding: 64px 0 36px;
      background:
        radial-gradient(circle at 12% -10%, var(--accent-soft), transparent 36rem),
        var(--surface);
      border-bottom: 1px solid var(--line);
    }
    .eyebrow {
      margin: 0 0 8px;
      color: var(--accent);
      font-size: .78rem;
      font-weight: 750;
      letter-spacing: .12em;
      text-transform: uppercase;
    }
    h1 { margin: 0; font-size: clamp(2rem, 5vw, 3.5rem); line-height: 1.08; letter-spacing: -.035em; }
    .lead { max-width: 720px; margin: 16px 0 0; color: var(--muted); font-size: 1.08rem; }
    .metrics { display: flex; flex-wrap: wrap; gap: 10px; margin: 26px 0 0; padding: 0; list-style: none; }
    .metric {
      min-width: 150px;
      padding: 12px 15px;
      background: var(--surface-soft);
      border: 1px solid var(--line);
      border-radius: 12px;
    }
    .metric strong { display: block; font-size: 1.16rem; }
    .metric span { color: var(--muted); font-size: .82rem; }
    main { padding: 32px 0 64px; }
    .notice {
      margin: 0 0 22px;
      padding: 13px 16px;
      color: var(--warning);
      background: var(--warning-soft);
      border: 1px solid currentColor;
      border-radius: 12px;
    }
    .index {
      margin: 0 0 24px;
      padding: 0 20px;
      background: var(--surface);
      border: 1px solid var(--line);
      border-radius: var(--radius);
      box-shadow: var(--shadow);
    }
    .index > summary { padding: 17px 0; cursor: pointer; font-weight: 720; }
    .index ol { margin: 0; padding: 0 0 18px 1.4rem; }
    .index li { padding: 5px 0 5px 5px; }
    .index time { margin-left: 8px; color: var(--muted); font-size: .84rem; white-space: nowrap; }
    .cards { display: grid; gap: 18px; }
    .decision {
      overflow: hidden;
      background: var(--surface);
      border: 1px solid var(--line);
      border-radius: var(--radius);
      box-shadow: var(--shadow);
      scroll-margin-top: 20px;
    }
    .decision-details > summary {
      position: relative;
      display: block;
      padding: 22px 24px;
      padding-right: 56px;
      cursor: pointer;
      list-style: none;
    }
    .decision-details > summary::-webkit-details-marker { display: none; }
    .decision-details > summary::marker { content: ""; }
    .decision-details > summary::after {
      content: "⌄";
      position: absolute;
      top: 50%;
      right: 24px;
      color: var(--muted);
      font-size: 1.25rem;
      transform: translateY(-50%);
      transition: transform .16s ease;
    }
    .decision-details[open] > summary::after { transform: translateY(-50%) rotate(180deg); }
    .decision-title { min-width: 0; }
    .decision-title strong { display: block; font-size: 1.1rem; overflow-wrap: anywhere; }
    .meta { display: flex; flex-wrap: wrap; gap: 8px 14px; margin-top: 7px; color: var(--muted); font-size: .84rem; }
    .maker, .selected-badge {
      display: inline-flex;
      align-items: center;
      width: fit-content;
      padding: 2px 8px;
      border-radius: 999px;
      font-size: .74rem;
      font-weight: 750;
      letter-spacing: .04em;
      text-transform: uppercase;
    }
    .maker { color: var(--accent); background: var(--accent-soft); }
    .decision-body { padding: 0 24px 24px; border-top: 1px solid var(--line); }
    .section { padding-top: 21px; }
    .section h2 {
      margin: 0 0 8px;
      color: var(--muted);
      font-size: .78rem;
      letter-spacing: .1em;
      text-transform: uppercase;
    }
    .preserve { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; }
    .choices { display: grid; gap: 9px; margin: 0; padding: 0; list-style: none; counter-reset: choices; }
    .choice {
      counter-increment: choices;
      position: relative;
      padding: 12px 14px 12px 46px;
      background: var(--surface-soft);
      border: 1px solid var(--line);
      border-radius: 11px;
      white-space: pre-wrap;
      overflow-wrap: anywhere;
    }
    .choice::before {
      content: counter(choices);
      position: absolute;
      top: 11px;
      left: 14px;
      width: 22px;
      height: 22px;
      border: 1px solid var(--line);
      border-radius: 50%;
      color: var(--muted);
      font-size: .75rem;
      line-height: 20px;
      text-align: center;
    }
    .choice.selected { background: var(--selected); border-color: var(--selected-line); }
    .choice.selected::before { color: var(--selected-line); border-color: var(--selected-line); font-weight: 800; }
    .selected-badge { margin-left: 8px; color: var(--selected-line); border: 1px solid currentColor; vertical-align: .08em; }
    .record-footer {
      display: flex;
      flex-wrap: wrap;
      justify-content: space-between;
      gap: 8px 16px;
      margin-top: 22px;
      padding-top: 15px;
      color: var(--muted);
      border-top: 1px solid var(--line);
      font-size: .8rem;
    }
    .empty {
      padding: 48px 24px;
      color: var(--muted);
      background: var(--surface);
      border: 1px solid var(--line);
      border-radius: var(--radius);
      text-align: center;
    }
    .page-footer { padding: 0 0 40px; color: var(--muted); font-size: .82rem; text-align: center; }
    @media (max-width: 640px) {
      .shell { width: min(100% - 22px, 1120px); }
      .hero { padding: 42px 0 28px; }
      .metric { flex: 1 1 135px; min-width: 0; }
      main { padding-top: 22px; }
      .decision-details > summary { padding: 18px 48px 18px 18px; }
      .decision-details > summary::after { right: 18px; }
      .decision-body { padding: 0 18px 19px; }
      .index { padding: 0 17px; }
      .index time { display: block; margin: 2px 0 0; }
    }
    @media (prefers-reduced-motion: reduce) {
      html { scroll-behavior: auto; }
      .decision-details > summary::after { transition: none; }
    }
    @media print {
      :root { --page: #fff; --surface: #fff; --surface-soft: #fff; --text: #000; --muted: #444; --line: #bbb; }
      body { background: #fff; font-size: 10.5pt; }
      .shell { width: 100%; }
      .hero { padding: 0 0 20px; background: none; }
      main { padding: 16px 0; }
      .index, .decision { box-shadow: none; break-inside: avoid; }
      .decision-details > summary::after { display: none; }
      details > *:not(summary) { display: block !important; }
      .page-footer { padding: 0; }
      a { color: inherit; text-decoration: none; }
    }
  </style>
</head>
<body>
"#;

pub fn render(records: &[DecisionRecord], skipped_count: usize) -> Result<String, String> {
    let mut output = String::with_capacity(
        DOCUMENT_START
            .len()
            .saturating_add(records.len().saturating_mul(2_048)),
    );
    output.push_str(DOCUMENT_START);
    output.push_str("<header class=\"hero\"><div class=\"shell\">");
    output.push_str("<p class=\"eyebrow\">PIRA decision history</p>");
    output.push_str("<h1>Workspace decisions</h1>");
    output.push_str(
        "<p class=\"lead\">A portable, searchable record of concluded choices and their decisive context.</p>",
    );
    output.push_str("<ul class=\"metrics\" aria-label=\"Export summary\">");
    metric(
        &mut output,
        &records.len().to_string(),
        plural(records.len(), "decision", "decisions"),
    );
    if let Some(newest) = records.first() {
        let timestamp = util::format_rfc3339(newest.timestamp_ms)?;
        metric(&mut output, &display_timestamp(&timestamp), "newest record");
    }
    if let Some(oldest) = records.last().filter(|_| records.len() > 1) {
        let timestamp = util::format_rfc3339(oldest.timestamp_ms)?;
        metric(&mut output, &display_timestamp(&timestamp), "oldest record");
    }
    output.push_str("</ul></div></header><main class=\"shell\">");

    if skipped_count > 0 {
        let _ = write!(
            output,
            "<p class=\"notice\" role=\"status\">{} invalid {} skipped during export.</p>",
            skipped_count,
            plural(skipped_count, "record was", "records were")
        );
    }

    if records.is_empty() {
        output.push_str(
            "<section class=\"empty\"><h2>No decisions in this export</h2><p>The selected range contained no valid records.</p></section>",
        );
    } else {
        render_index(&mut output, records)?;
        output.push_str("<section class=\"cards\" aria-label=\"Decision records\">");
        for record in records {
            render_record(&mut output, record)?;
        }
        output.push_str("</section>");
    }
    output.push_str("</main><footer class=\"page-footer shell\">Exported by <code>pira_dec ");
    escape(&mut output, env!("CARGO_PKG_VERSION"));
    output.push_str("</code></footer></body></html>\n");
    Ok(output)
}

fn metric(output: &mut String, value: &str, label: &str) {
    output.push_str("<li class=\"metric\"><strong>");
    escape(output, value);
    output.push_str("</strong><span>");
    escape(output, label);
    output.push_str("</span></li>");
}

fn render_index(output: &mut String, records: &[DecisionRecord]) -> Result<(), String> {
    let _ = write!(
        output,
        "<details class=\"index\"><summary>Decision index · {} {}</summary><ol>",
        records.len(),
        plural(records.len(), "entry", "entries")
    );
    for record in records {
        let timestamp = util::format_rfc3339(record.timestamp_ms)?;
        output.push_str("<li><a href=\"#");
        escape(output, &record.id);
        output.push_str("\">");
        escape(
            output,
            &util::single_line_clip(record.selected_text()?, 120),
        );
        output.push_str("</a><time datetime=\"");
        escape(output, &timestamp);
        output.push_str("\">");
        escape(output, &display_timestamp(&timestamp));
        output.push_str("</time></li>");
    }
    output.push_str("</ol></details>");
    Ok(())
}

fn render_record(output: &mut String, record: &DecisionRecord) -> Result<(), String> {
    let timestamp = util::format_rfc3339(record.timestamp_ms)?;
    let selected = record.selected_text()?;
    output.push_str("<article class=\"decision\" id=\"");
    escape(output, &record.id);
    output.push_str(
        "\"><details class=\"decision-details\" open><summary><span class=\"decision-title\"><strong>",
    );
    escape(output, selected);
    output.push_str("</strong><span class=\"meta\"><time datetime=\"");
    escape(output, &timestamp);
    output.push_str("\">");
    escape(output, &display_timestamp(&timestamp));
    output.push_str("</time><span class=\"maker\">");
    escape(output, record.maker.as_str());
    output.push_str("</span></span></span></summary><div class=\"decision-body\">");

    output.push_str("<section class=\"section\"><h2>Context</h2><p class=\"preserve\">");
    escape(output, &record.context);
    output.push_str(
        "</p></section><section class=\"section\"><h2>Alternatives</h2><ol class=\"choices\">",
    );
    for (index, choice) in record.choices.iter().enumerate() {
        let selected_choice = index + 1 == record.decision as usize;
        output.push_str(if selected_choice {
            "<li class=\"choice selected\">"
        } else {
            "<li class=\"choice\">"
        });
        escape(output, choice);
        if selected_choice {
            output.push_str("<span class=\"selected-badge\">Selected</span>");
        }
        output.push_str("</li>");
    }
    output.push_str("</ol></section>");
    if record.supersedes.is_some() || !record.related.is_empty() {
        output.push_str("<section class=\"section\"><h2>Relationships</h2><ul>");
        if let Some(id) = &record.supersedes {
            output.push_str("<li>Supersedes <code>");
            escape(output, id);
            output.push_str("</code></li>");
        }
        for id in &record.related {
            output.push_str("<li>Related <code>");
            escape(output, id);
            output.push_str("</code></li>");
        }
        output.push_str("</ul></section>");
    }
    output.push_str("<footer class=\"record-footer\"><code>");
    escape(output, &record.id);
    output.push_str("</code><a href=\"#");
    escape(output, &record.id);
    output.push_str(
        "\" aria-label=\"Permanent link to this decision\">Link to decision</a></footer>",
    );
    output.push_str("</div></details></article>");
    Ok(())
}

fn escape(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

fn display_timestamp(rfc3339: &str) -> String {
    format!("{} {} UTC", &rfc3339[..10], &rfc3339[11..16])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Maker;

    fn record() -> DecisionRecord {
        DecisionRecord {
            id: "D-20260717-063012-0123456789abcdef".into(),
            timestamp_ms: 1_784_269_812_345,
            context: "<script>alert(\"context\")</script> & 'quoted'".into(),
            choices: vec![
                "Keep <b>plain</b>".into(),
                "</style><img src=x onerror=alert(1)>".into(),
            ],
            decision: 2,
            maker: Maker::Human,
            supersedes: None,
            related: Vec::new(),
        }
    }

    #[test]
    fn export_is_standalone_escaped_and_semantic() {
        let html = render(&[record()], 1).unwrap();
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("Content-Security-Policy"));
        assert!(html.contains("&lt;script&gt;alert(&quot;context&quot;)&lt;/script&gt;"));
        assert!(html.contains("&lt;/style&gt;&lt;img src=x onerror=alert(1)&gt;"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("<img"));
        assert!(!html.contains("http://"));
        assert!(!html.contains("https://"));
        assert!(html.contains("<details class=\"decision-details\" open>"));
        assert!(html.contains("selected-badge"));
        assert!(html.contains("1 invalid record was skipped"));
        assert!(html.contains("href=\"#D-20260717-063012-0123456789abcdef\""));
    }

    #[test]
    fn export_handles_empty_ranges() {
        let html = render(&[], 0).unwrap();
        assert!(html.contains("No decisions in this export"));
        assert!(html.contains("<strong>0</strong><span>decisions</span>"));
        assert!(!html.contains("class=\"cards\""));
    }

    #[test]
    fn display_timestamp_keeps_date_and_utc_minute() {
        assert_eq!(
            display_timestamp("2026-07-23T05:01:56.991Z"),
            "2026-07-23 05:01 UTC"
        );
    }
}
