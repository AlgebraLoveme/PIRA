use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::util;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    Stdout,
    Stderr,
}

impl fmt::Display for StreamKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineMeta {
    #[serde(rename = "line")]
    pub line: usize,
    pub stream: StreamKind,
    pub offset: u64,
    #[serde(rename = "length")]
    pub length: u64,
    #[serde(default)]
    pub score: i64,
    #[serde(default)]
    pub flags: u32,
}

pub mod line_flag {
    pub const FAILURE: u32 = 1 << 0;
    pub const SUCCESS: u32 = 1 << 1;
    pub const SUCCESSFUL_CHECK: u32 = 1 << 2;
    pub const DIFF: u32 = 1 << 3;
    pub const ERROR: u32 = 1 << 4;
    pub const FAILED_TEST: u32 = 1 << 5;
    pub const WARNING: u32 = 1 << 6;
    pub const NOTE: u32 = 1 << 7;
    pub const KEYWORD: u32 = 1 << 8;
    pub const PATH: u32 = 1 << 9;
    pub const METRIC: u32 = 1 << 10;
    pub const MARKER: u32 = 1 << 11;
    pub const STACK: u32 = 1 << 12;
    pub const STDERR: u32 = 1 << 13;
    pub const POSITION: u32 = 1 << 14;
    pub const NONZERO_TAIL: u32 = 1 << 15;
    pub const NUMERIC_ANOMALY: u32 = 1 << 16;
    pub const INFORMATIVE: u32 = 1 << 17;

    pub const HIGH_CONFIDENCE: u32 =
        FAILURE | SUCCESS | ERROR | FAILED_TEST | WARNING | NUMERIC_ANOMALY;
    pub const STABLE_TEMPLATE: u32 = FAILURE
        | SUCCESS
        | SUCCESSFUL_CHECK
        | DIFF
        | ERROR
        | FAILED_TEST
        | WARNING
        | NOTE
        | KEYWORD
        | PATH
        | METRIC
        | MARKER
        | STACK
        | STDERR
        | NONZERO_TAIL
        | NUMERIC_ANOMALY;
}

impl LineMeta {
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

#[derive(Debug)]
pub struct TempSpool {
    pub path: PathBuf,
    pub length: u64,
    pub observed_length: u64,
    pub sha256: [u8; 32],
    pub binary: bool,
    pub non_utf8: bool,
}

impl Drop for TempSpool {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Debug)]
pub struct CaptureResult {
    pub stdout: TempSpool,
    pub stderr: TempSpool,
    pub timeline: Vec<LineMeta>,
    pub total_lines: usize,
    pub stdout_lines: usize,
    pub stderr_lines: usize,
    pub timeline_truncated: bool,
    pub retention_truncated: bool,
    pub exit_code: i32,
    pub start_ms: u128,
    pub end_ms: u128,
    pub duration_ms: u128,
    pub cwd: String,
    pub live_id: Option<String>,
    pub live_store_dir: Option<PathBuf>,
}

impl Drop for CaptureResult {
    fn drop(&mut self) {
        if let (Some(store), Some(id)) = (&self.live_store_dir, &self.live_id) {
            crate::storage::remove_live_checkpoint(store, id);
        }
    }
}

impl CaptureResult {
    pub fn total_bytes(&self) -> u64 {
        self.stdout.length.saturating_add(self.stderr.length)
    }

    pub fn observed_bytes(&self) -> u64 {
        self.stdout
            .observed_length
            .saturating_add(self.stderr.observed_length)
    }

    pub fn readers(&self) -> Result<StreamReaders, String> {
        StreamReaders::from_paths(
            &self.stdout.path,
            0,
            self.stdout.length,
            &self.stderr.path,
            0,
            self.stderr.length,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(default = "default_compat_version")]
    pub compat_version: u32,
    #[serde(default)]
    pub tool_version: String,
    #[serde(default)]
    pub command_argv: Vec<String>,
    #[serde(default)]
    pub original_command_argv: Vec<String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub start_unix_ms: u128,
    #[serde(default)]
    pub end_unix_ms: u128,
    #[serde(default)]
    pub duration_ms: u128,
    #[serde(default)]
    pub exit_code: i32,
    #[serde(default)]
    pub stdout_bytes: u64,
    #[serde(default)]
    pub stderr_bytes: u64,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub observed_stdout_bytes: u64,
    #[serde(default)]
    pub observed_stderr_bytes: u64,
    #[serde(default)]
    pub observed_total_bytes: u64,
    #[serde(default)]
    pub retention_truncated: bool,
    #[serde(default)]
    pub stdout_lines: usize,
    #[serde(default)]
    pub stderr_lines: usize,
    #[serde(default)]
    pub total_lines: usize,
    #[serde(default)]
    pub detected_paths: Vec<String>,
    #[serde(default)]
    pub binary_stdout: bool,
    #[serde(default)]
    pub binary_stderr: bool,
    #[serde(default)]
    pub non_utf8_stdout: bool,
    #[serde(default)]
    pub non_utf8_stderr: bool,
    #[serde(default)]
    pub line_timeline: Vec<LineMeta>,
    #[serde(default)]
    pub suggested_keywords: Vec<String>,
    #[serde(default)]
    pub store_dir: String,
    #[serde(default)]
    pub store_path: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub result_id: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub workspace_hash: String,
    #[serde(default)]
    pub stdout_sha256: String,
    #[serde(default)]
    pub stderr_sha256: String,
    #[serde(default)]
    pub timeline_truncated: bool,
}

fn default_compat_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListedEntry {
    pub id: String,
    pub filename: String,
    pub timestamp: String,
    pub start_ms: u128,
    pub exit: i32,
    pub bytes: u64,
    pub lines: usize,
    pub command: String,
    pub path: PathBuf,
    pub workspace_hash: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub running: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl ListedEntry {
    pub fn from_metadata(metadata: &Metadata, path: PathBuf) -> Self {
        Self {
            id: metadata.result_id.clone(),
            filename: metadata.filename.clone(),
            timestamp: metadata.created_at.clone(),
            start_ms: metadata.start_unix_ms,
            exit: metadata.exit_code,
            bytes: metadata.total_bytes,
            lines: metadata.total_lines,
            command: util::redacted_argv_display(&metadata.command_argv),
            path,
            workspace_hash: metadata.workspace_hash.clone(),
            running: false,
        }
    }
}

#[derive(Debug)]
pub struct StreamReaders {
    stdout: SectionReader,
    stderr: SectionReader,
}

#[derive(Debug, Clone)]
pub struct BlockDescriptor {
    pub codec: u8,
    pub logical_offset: u64,
    pub uncompressed_length: u64,
    pub stored_length: u64,
    pub payload_offset: u64,
    pub content_sha256: Option<[u8; 32]>,
}

#[derive(Debug)]
enum SectionReader {
    Raw {
        reader: BufReader<File>,
        base: u64,
        length: u64,
    },
    Blocks {
        file: File,
        base: u64,
        length: u64,
        blocks: Vec<BlockDescriptor>,
        cache: Option<(usize, Vec<u8>)>,
    },
}

impl StreamReaders {
    pub fn from_paths(
        stdout_path: &std::path::Path,
        stdout_base: u64,
        stdout_length: u64,
        stderr_path: &std::path::Path,
        stderr_base: u64,
        stderr_length: u64,
    ) -> Result<Self, String> {
        Ok(Self {
            stdout: SectionReader::Raw {
                reader: BufReader::new(File::open(stdout_path).map_err(|error| error.to_string())?),
                base: stdout_base,
                length: stdout_length,
            },
            stderr: SectionReader::Raw {
                reader: BufReader::new(File::open(stderr_path).map_err(|error| error.to_string())?),
                base: stderr_base,
                length: stderr_length,
            },
        })
    }

    pub fn from_blocks(
        path: &std::path::Path,
        stdout_base: u64,
        stdout_length: u64,
        stdout_blocks: Vec<BlockDescriptor>,
        stderr_base: u64,
        stderr_length: u64,
        stderr_blocks: Vec<BlockDescriptor>,
    ) -> Result<Self, String> {
        Ok(Self {
            stdout: SectionReader::Blocks {
                file: File::open(path).map_err(|e| e.to_string())?,
                base: stdout_base,
                length: stdout_length,
                blocks: stdout_blocks,
                cache: None,
            },
            stderr: SectionReader::Blocks {
                file: File::open(path).map_err(|e| e.to_string())?,
                base: stderr_base,
                length: stderr_length,
                blocks: stderr_blocks,
                cache: None,
            },
        })
    }

    pub fn read_display_line(&mut self, line: &LineMeta) -> Result<String, String> {
        let bytes = self.read_bounded(line, util::MAX_DISPLAY_READ_BYTES)?;
        Ok(util::sanitize_terminal(&String::from_utf8_lossy(&bytes)))
    }

    pub fn read_security_line(&mut self, line: &LineMeta) -> Result<String, String> {
        let bytes = self.read_bounded(line, util::MAX_DISPLAY_READ_BYTES)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn read_search_line(&mut self, line: &LineMeta) -> Result<String, String> {
        let bytes = self.read_bounded(line, util::MAX_SEARCH_LINE_BYTES)?;
        Ok(util::sanitize_terminal(&String::from_utf8_lossy(&bytes)))
    }

    fn read_bounded(&mut self, line: &LineMeta, maximum: u64) -> Result<Vec<u8>, String> {
        let reader = self.parts_mut(line.stream);
        let section_length = reader.length();
        validate_line(line, section_length)?;
        if line.length <= maximum {
            return reader.read_range(line.offset, line.length);
        }
        let prefix_length = maximum / 2;
        let suffix_length = maximum - prefix_length;
        let mut bytes = reader.read_range(line.offset, prefix_length)?;
        bytes.extend_from_slice(b" ... [line read truncated] ... ");
        bytes.extend_from_slice(
            &reader.read_range(line.offset + line.length - suffix_length, suffix_length)?,
        );
        Ok(bytes)
    }

    pub fn copy_line<W: Write>(&mut self, line: &LineMeta, output: &mut W) -> Result<(), String> {
        let reader = self.parts_mut(line.stream);
        let section_length = reader.length();
        validate_line(line, section_length)?;
        reader.copy_range(line.offset, line.length, output)
    }

    pub fn copy_section<W: Write>(
        &mut self,
        stream: StreamKind,
        output: &mut W,
    ) -> Result<(), String> {
        let reader = self.parts_mut(stream);
        let length = reader.length();
        reader.copy_range(0, length, output)
    }

    fn parts_mut(&mut self, stream: StreamKind) -> &mut SectionReader {
        match stream {
            StreamKind::Stdout => &mut self.stdout,
            StreamKind::Stderr => &mut self.stderr,
        }
    }
}

impl SectionReader {
    fn length(&self) -> u64 {
        match self {
            Self::Raw { length, .. } | Self::Blocks { length, .. } => *length,
        }
    }
    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, String> {
        let size = usize::try_from(length).map_err(|_| "range too large")?;
        let mut out = Vec::with_capacity(size);
        self.copy_range(offset, length, &mut out)?;
        Ok(out)
    }
    fn copy_range<W: Write>(
        &mut self,
        offset: u64,
        length: u64,
        out: &mut W,
    ) -> Result<(), String> {
        if offset.checked_add(length).is_none_or(|e| e > self.length()) {
            return Err("stream range exceeds section".into());
        }
        match self {
            Self::Raw { reader, base, .. } => {
                reader
                    .seek(SeekFrom::Start(*base + offset))
                    .map_err(|e| e.to_string())?;
                let mut limited = reader.take(length);
                std::io::copy(&mut limited, out).map_err(util::io_error)?;
                Ok(())
            }
            Self::Blocks {
                file,
                base,
                blocks,
                cache,
                ..
            } => {
                let end = offset + length;
                for (index, b) in blocks.iter().enumerate() {
                    let bend = b.logical_offset + b.uncompressed_length;
                    if bend <= offset || b.logical_offset >= end {
                        continue;
                    }
                    if cache.as_ref().is_none_or(|(i, _)| *i != index) {
                        file.seek(SeekFrom::Start(*base + b.payload_offset))
                            .map_err(|e| e.to_string())?;
                        let mut stored = vec![
                            0;
                            usize::try_from(b.stored_length)
                                .map_err(|_| "block too large")?
                        ];
                        file.read_exact(&mut stored).map_err(|e| e.to_string())?;
                        let decoded = match b.codec {
                            0 => stored,
                            1 => lz4_flex::block::decompress(
                                &stored,
                                usize::try_from(b.uncompressed_length)
                                    .map_err(|_| "block too large")?,
                            )
                            .map_err(|e| format!("lz4 decode: {e}"))?,
                            _ => return Err("unsupported block codec".into()),
                        };
                        if b.content_sha256.is_some_and(|expected| {
                            <[u8; 32]>::from(sha2::Sha256::digest(&decoded)) != expected
                        }) {
                            return Err("corrupt result: block checksum mismatch".into());
                        }
                        *cache = Some((index, decoded));
                    }
                    let data = &cache.as_ref().unwrap().1;
                    let from = offset.max(b.logical_offset) - b.logical_offset;
                    let to = end.min(bend) - b.logical_offset;
                    out.write_all(&data[from as usize..to as usize])
                        .map_err(|e| e.to_string())?;
                }
                Ok(())
            }
        }
    }
}

fn validate_line(line: &LineMeta, section_length: u64) -> Result<(), String> {
    if line
        .offset
        .checked_add(line.length)
        .is_none_or(|end| end > section_length)
    {
        return Err(format!("invalid timeline offset at L{}", line.line));
    }
    Ok(())
}
