use serde::{Deserialize, Serialize};

const MAX_LINES: usize = 20;
const MAX_LINE_BYTES: usize = 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TerminalView {
    lines: Vec<Vec<u8>>,
    column: usize,
    escape: bool,
    csi: Vec<u8>,
    #[serde(default)]
    osc_escape: bool,
    pub reliable: bool,
}

impl Default for TerminalView {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            column: 0,
            escape: false,
            csi: Vec::new(),
            osc_escape: false,
            reliable: true,
        }
    }
}

impl TerminalView {
    pub fn feed(&mut self, bytes: &[u8]) {
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        for &byte in bytes {
            if self.escape {
                if self.csi.is_empty() {
                    match byte {
                        b'[' | b']' => self.csi.push(byte),
                        _ => {
                            self.escape = false;
                            self.reliable = false;
                        }
                    }
                    continue;
                }
                if self.csi[0] == b']' {
                    if byte == 7 || (self.osc_escape && byte == b'\\') {
                        self.escape = false;
                        self.csi.clear();
                        self.osc_escape = false;
                    } else {
                        self.osc_escape = byte == 0x1b;
                        if self.csi.len() < 4096 {
                            self.csi.push(byte);
                        } else {
                            self.escape = false;
                            self.reliable = false;
                        }
                    }
                    continue;
                }
                self.csi.push(byte);
                if self.csi.len() > 64 {
                    self.escape = false;
                    self.reliable = false;
                } else if self.csi.len() > 1 && (0x40..=0x7e).contains(&byte) {
                    self.apply_escape();
                }
                continue;
            }
            match byte {
                0x1b => {
                    self.escape = true;
                    self.csi.clear();
                    self.osc_escape = false;
                }
                b'\r' => self.column = 0,
                b'\n' => {
                    self.lines.push(Vec::new());
                    self.column = 0;
                    self.trim();
                }
                8 => {
                    self.column = self.column.saturating_sub(1);
                }
                0x20..=0x7e | 0x80..=0xff => {
                    let line = self.lines.last_mut().unwrap();
                    if self.column < line.len() {
                        line[self.column] = byte;
                    } else if line.len() < MAX_LINE_BYTES {
                        line.resize(self.column, b' ');
                        line.push(byte);
                    }
                    self.column = self.column.saturating_add(1);
                }
                b'\t' => self.column = (self.column / 8 + 1) * 8,
                _ => {}
            }
        }
    }
    fn apply_escape(&mut self) {
        self.escape = false;
        if self.csi.first() == Some(&b'[') {
            match *self.csi.last().unwrap_or(&0) {
                b'm' => {}
                b'K' => {
                    let line = self.lines.last_mut().unwrap();
                    line.truncate(self.column);
                }
                b'G' => {
                    let value = std::str::from_utf8(&self.csi[1..self.csi.len() - 1])
                        .ok()
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(1);
                    self.column = value.saturating_sub(1);
                }
                _ => self.reliable = false,
            }
        } else {
            self.reliable = false;
        }
    }
    fn trim(&mut self) {
        if self.lines.len() > MAX_LINES {
            self.lines.drain(..self.lines.len() - MAX_LINES);
        }
    }
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|v| String::from_utf8_lossy(v))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn carriage_return_overwrites_visible_line() {
        let mut v = TerminalView::default();
        v.feed(b"10%\r10%\r11%");
        assert_eq!(v.text(), "11%");
    }

    #[test]
    fn carriage_return_cursor_survives_incremental_feeds() {
        let mut v = TerminalView::default();
        v.feed(b"10%\r");
        v.feed(b"11%");
        assert_eq!(v.text(), "11%");
    }

    #[test]
    fn ansi_color_is_ignored_without_losing_text() {
        let mut v = TerminalView::default();
        v.feed(b"\x1b[31mred\x1b[0m");
        assert_eq!(v.text(), "red");
        assert!(v.reliable);
    }

    #[test]
    fn split_ansi_sequence_is_incremental() {
        let mut v = TerminalView::default();
        v.feed(b"\x1b[");
        v.feed(b"32mok");
        assert_eq!(v.text(), "ok");
        assert!(v.reliable);
    }

    #[test]
    fn osc_title_is_ignored() {
        let mut v = TerminalView::default();
        v.feed(b"before\x1b]0;title\x07after");
        assert_eq!(v.text(), "beforeafter");
        assert!(v.reliable);
    }
}
