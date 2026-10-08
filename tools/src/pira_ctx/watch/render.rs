use serde::{Deserialize, Serialize};

const ROWS: u16 = 20;
const COLS: u16 = 80;
const MAX_REPLAY_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
enum Engine {
    #[serde(rename = "vt100-0.16.2-20x80-v1")]
    Vt100,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Failure {
    UnsupportedInput,
    Resize,
    ReplayLimit,
    CellLimit,
    #[serde(rename = "backend_failure")]
    Backend,
}

/// A fixed-viewport text-cell view, reconstructed only from its complete bounded input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "SavedView")]
pub struct TerminalView {
    engine: Engine,
    transcript: Vec<u8>,
    pub reliable: bool,
    failure: Option<Failure>,
    #[serde(skip)]
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedView {
    engine: Engine,
    transcript: Vec<u8>,
    reliable: bool,
    failure: Option<Failure>,
}

impl TryFrom<SavedView> for TerminalView {
    type Error = String;
    fn try_from(saved: SavedView) -> Result<Self, Self::Error> {
        if saved.transcript.len() > MAX_REPLAY_BYTES {
            return Err(
                "terminal replay exceeds 32 KiB; no clipped-state reconstruction is allowed".into(),
            );
        }
        let mut view = Self::default();
        view.feed(&saved.transcript);
        view.engine = saved.engine;
        view.reliable &= saved.reliable && saved.failure.is_none();
        view.failure = view.failure.or(saved.failure);
        Ok(view)
    }
}

impl Default for TerminalView {
    fn default() -> Self {
        Self {
            engine: Engine::Vt100,
            transcript: Vec::new(),
            reliable: true,
            failure: None,
            text: String::new(),
        }
    }
}

impl TerminalView {
    pub fn feed(&mut self, bytes: &[u8]) {
        // Once a prefix is lost, no later tail or reset sequence proves continuity.
        if !self.reliable {
            return;
        }
        if bytes.len() > MAX_REPLAY_BYTES.saturating_sub(self.transcript.len()) {
            self.reliable = false;
            self.failure = Some(Failure::ReplayLimit);
            return;
        }
        let previous_length = self.transcript.len();
        self.transcript.extend_from_slice(bytes);
        let rendered = std::panic::catch_unwind(|| render(&self.transcript));
        match rendered {
            Ok((text, failure)) => {
                self.text = text;
                self.failure = failure;
                self.reliable = failure.is_none();
            }
            Err(_) => {
                // Retain the last replayable prefix and its view, not panic-inducing bytes.
                self.transcript.truncate(previous_length);
                self.reliable = false;
                self.failure = Some(Failure::Backend);
            }
        }
    }

    pub fn text(&self) -> String {
        self.text.clone()
    }

    pub fn reason(&self) -> Option<&'static str> {
        if self.reliable {
            return None;
        }
        Some(match self.failure {
            Some(Failure::UnsupportedInput) => "unsupported terminal input",
            Some(Failure::Resize) => "terminal resize is unsupported (fixed 20x80 viewport)",
            Some(Failure::ReplayLimit) => "terminal replay exceeds 32 KiB; view frozen",
            Some(Failure::CellLimit) => {
                "terminal cell exceeds supported combining-content capacity"
            }
            Some(Failure::Backend) => "terminal backend failed; view frozen",
            None => "terminal input is clipped or incomplete; view frozen",
        })
    }
}

#[derive(Default)]
struct Callbacks(Option<Failure>);
impl vt100::Callbacks for Callbacks {
    fn resize(&mut self, _: &mut vt100::Screen, _: (u16, u16)) {
        // PIRA: never call set_size; upstream issue 28 reports wide-cell resize panics.
        self.0 = Some(Failure::Resize);
    }
    fn unhandled_char(&mut self, _: &mut vt100::Screen, _: char) {
        self.0 = Some(Failure::UnsupportedInput);
    }
    fn unhandled_control(&mut self, _: &mut vt100::Screen, _: u8) {
        self.0 = Some(Failure::UnsupportedInput);
    }
    fn unhandled_escape(&mut self, _: &mut vt100::Screen, _: Option<u8>, _: Option<u8>, _: u8) {
        self.0 = Some(Failure::UnsupportedInput);
    }
    fn unhandled_csi(
        &mut self,
        _: &mut vt100::Screen,
        _: Option<u8>,
        _: Option<u8>,
        _: &[&[u16]],
        _: char,
    ) {
        self.0 = Some(Failure::UnsupportedInput);
    }
    fn unhandled_osc(&mut self, _: &mut vt100::Screen, _: &[&[u8]]) {
        self.0 = Some(Failure::UnsupportedInput);
    }
}

fn render(bytes: &[u8]) -> (String, Option<Failure>) {
    let mut parser = vt100::Parser::new_with_callbacks(ROWS, COLS, 0, Callbacks::default());
    let mut failure = unsupported_encoding(bytes).then_some(Failure::UnsupportedInput);
    for byte in bytes {
        parser.process(std::slice::from_ref(byte));
        // vt100::Cell::append silently stops at 18 bytes. Fail conservatively before
        // that ceiling can silently discard another combining character. Inspect the
        // cells at/behind the cursor, including a wrapped wide-character continuation.
        let screen = parser.screen();
        let (row, col) = screen.cursor_position();
        let candidates = [
            (row, col),
            (row, col.saturating_sub(1)),
            (row, col.saturating_sub(2)),
            (row.saturating_sub(1), COLS - 1),
            (row.saturating_sub(1), COLS - 2),
        ];
        if candidates.into_iter().any(|(row, col)| {
            screen
                .cell(row, col)
                .is_some_and(|cell| cell.contents().len() >= 18)
        }) {
            failure = Some(Failure::CellLimit);
        }
        failure = failure.or(parser.callbacks().0);
    }
    // contents() joins soft-wrapped rows; watch compares the physical viewport.
    let rows = parser.screen().rows(0, COLS).collect::<Vec<_>>().join("\n");
    (rows.trim_end_matches('\n').to_owned(), failure)
}

fn unsupported_encoding(bytes: &[u8]) -> bool {
    // This is a rejection guard, not a renderer: vt100/vte silently ignore SI/SO,
    // DCS/SOS/PM/APC and overflowing CSI parameters/intermediates. Reject these
    // encodings, even inside another control payload, rather than imply support.
    if bytes.iter().any(|byte| matches!(byte, 14 | 15))
        || bytes
            .windows(2)
            .any(|pair| pair[0] == 0x1b && matches!(pair[1], b'P' | b'X' | b'^' | b'_'))
    {
        return true;
    }
    static CSI: std::sync::LazyLock<regex::bytes::Regex> =
        std::sync::LazyLock::new(|| regex::bytes::Regex::new(r"\x1b\[([0-?]*)([ -/]*)").unwrap());
    CSI.captures_iter(bytes).any(|capture| {
        let params = &capture[1];
        params.len() > 64
            || !capture[2].is_empty()
            || params
                .iter()
                .filter(|byte| matches!(byte, b';' | b':'))
                .count()
                >= 16
            || params.split(|byte| !byte.is_ascii_digit()).any(|number| {
                !number.is_empty()
                    && (number.len() > 5
                        || std::str::from_utf8(number).unwrap().parse::<u32>().unwrap()
                            > u16::MAX as u32)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(bytes: &[u8]) -> TerminalView {
        let mut view = TerminalView::default();
        view.feed(bytes);
        view
    }

    #[test]
    fn unicode_cells_combining_and_wide_overwrite() {
        for (input, expected) in [
            ("é\u{8}X", "X"),
            ("e\u{301}Z", "e\u{301}Z"),
            ("界X\rY", "Y X"),
        ] {
            let rendered = view(input.as_bytes());
            assert_eq!(rendered.text(), expected);
            assert!(rendered.reliable, "{input}");
        }
    }

    #[test]
    fn cursor_erase_and_native_line_feed_are_engine_semantics() {
        for (input, expected) in [
            ("10%\r11%", "11%"),
            ("abc\x1b[2G\x1b[KX", "aX"),
            ("abc\x1b[2G\x1b[1KX", " Xc"),
            ("abc\x1b[2G\x1b[2KX", " X"),
            ("abc\nX", "abc\n   X"),
            ("abc\r\nX", "abc\nX"),
            ("a\r\nb\x1b[1;1HX", "X\nb"),
            ("A\tB", "A       B"),
        ] {
            let rendered = view(input.as_bytes());
            assert_eq!(rendered.text(), expected, "{input:?}");
            assert!(rendered.reliable, "{input:?}");
        }
    }

    #[test]
    fn viewport_wrap_scroll_and_saved_cursor_are_text_cells() {
        let wrapped = view(format!("{}XY", "a".repeat(79)).as_bytes());
        assert_eq!(wrapped.text(), format!("{}X\nY", "a".repeat(79)));
        assert!(wrapped.reliable);
        let scrolled = view(
            (0..22)
                .map(|i| format!("{i}\r\n"))
                .collect::<String>()
                .as_bytes(),
        );
        assert_eq!(
            scrolled.text(),
            (3..22)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert!(scrolled.reliable);
        let saved = view(b"ab\x1b7\rX\x1b8Z");
        assert_eq!(saved.text(), "XbZ");
        assert!(saved.reliable);
    }

    #[test]
    fn exact_replay_restores_partial_utf8_escape_and_alternate_screen_state() {
        let input = "é\x1b[31m界\x1b[0m\x1b[?1049halternate\x1b[?1049l\u{8}X".as_bytes();
        let expected = view(input);
        assert!(expected.reliable);
        for split in 0..=input.len() {
            let first = view(&input[..split]);
            let encoded = serde_json::to_vec(&first).unwrap();
            let mut resumed: TerminalView = serde_json::from_slice(&encoded).unwrap();
            resumed.feed(&input[split..]);
            assert_eq!(resumed.text(), expected.text(), "split {split}");
            assert!(resumed.reliable, "split {split}");
        }
    }

    #[test]
    fn unsupported_protocols_resize_and_capacity_are_not_reliable() {
        for bytes in [
            &b"\xe7\x95\x8c\x1b[8;25;90t"[..],
            b"\x1bPqignored\x1b\\",
            b"\x1b[99999999G",
            b"\x0eabc",
            b"\x1b]777;unknown\x07",
            b"\xff",
            b"\x1b[3K",
        ] {
            let rendered = view(bytes);
            assert!(!rendered.reliable, "{bytes:?}");
            assert!(rendered.reason().is_some());
        }
        let rendered = view(format!("e{}", "\u{301}".repeat(12)).as_bytes());
        assert!(!rendered.reliable);
        assert!(view(b"\x1b]0;title\x07\x1b[31mred\x1b[0m").reliable);
    }

    #[test]
    fn replay_ceiling_and_legacy_state_never_reconstruct_from_a_tail() {
        let mut rendered = view(b"kept");
        rendered.feed(&vec![b'x'; MAX_REPLAY_BYTES]);
        assert!(!rendered.reliable);
        assert_eq!(rendered.text(), "kept");
        let mut resumed: TerminalView =
            serde_json::from_slice(&serde_json::to_vec(&rendered).unwrap()).unwrap();
        resumed.feed(b"\x1bcnew");
        assert!(!resumed.reliable);
        assert_eq!(resumed.text(), "kept");
        assert!(
            serde_json::from_str::<TerminalView>(
                r#"{"lines":[],"column":0,"escape":false,"csi":[],"reliable":true}"#
            )
            .is_err()
        );
    }
}
