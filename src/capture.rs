//! Bounded, session-local terminal excerpts. No input log and no growing disk log.
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    path::Path,
    time::Duration,
};

pub const OUTPUT_LIMIT: usize = 4096;
pub const SNAPSHOT_LIMIT: usize = 24 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureRecord {
    pub command: String,
    pub cwd: String,
    pub status: i32,
    pub output: String,
    pub truncated: bool,
    pub omitted_reason: Option<String>,
}

#[derive(Default)]
pub struct CaptureBuffer {
    records: Vec<CaptureRecord>,
    active: Option<Active>,
}
struct Active {
    command: String,
    cwd: String,
    tail: VecDeque<u8>,
    line: VecDeque<u8>,
    ansi: Ansi,
    watch: Vec<u8>,
    secret: bool,
    interactive: bool,
    truncated: bool,
    cr: bool,
    stopped: bool,
}
#[derive(Default)]
enum Ansi {
    #[default]
    Text,
    Escape,
    Csi(Vec<u8>),
    String,
    StringEscape,
}
impl CaptureBuffer {
    pub fn begin(&mut self, command: &str, cwd: &str) {
        // A missing end event never turns an incomplete interval into a completed command.
        self.active = None;
        if command.len() > 8192
            || cwd.len() > 4096
            || !crate::context::suitable_history(command)
            || crate::context::contains_secret(cwd)
            || cwd.chars().any(char::is_control)
            || crate::context::forbidden_directory(Path::new(cwd))
        {
            return;
        }
        self.active = Some(Active {
            command: command.into(),
            cwd: cwd.into(),
            tail: VecDeque::with_capacity(OUTPUT_LIMIT),
            line: VecDeque::with_capacity(OUTPUT_LIMIT),
            ansi: Ansi::Text,
            watch: vec![],
            secret: false,
            interactive: false,
            truncated: false,
            cr: false,
            stopped: false,
        });
    }
    /// Record terminal output only. Input bytes are never stored.
    pub fn output(&mut self, bytes: &[u8]) {
        let Some(active) = &mut self.active else {
            return;
        };
        if active.stopped {
            return;
        }
        let mut plain = Vec::with_capacity(bytes.len().min(16384));
        for &byte in bytes {
            match &mut active.ansi {
                Ansi::Text => match byte {
                    0x1b => active.ansi = Ansi::Escape,
                    b'\r' => {
                        plain.push(b'\n');
                        active.cr = true;
                    }
                    b'\n' => {
                        plain.push(byte);
                        active.line.push_back(b'\n');
                        while let Some(byte) = active.line.pop_front() {
                            if active.tail.len() == OUTPUT_LIMIT {
                                active.tail.pop_front();
                                active.truncated = true;
                            }
                            active.tail.push_back(byte);
                        }
                        active.cr = false;
                    }
                    b'\t' | 0x20..=0xff if byte != 0x7f => {
                        plain.push(byte);
                        if active.cr {
                            active.line.clear();
                        }
                        if active.line.len() == OUTPUT_LIMIT {
                            active.line.pop_front();
                            active.truncated = true;
                        }
                        active.line.push_back(byte);
                        active.cr = false;
                    }
                    _ => {}
                },
                Ansi::Escape => {
                    active.ansi = match byte {
                        b'[' => Ansi::Csi(Vec::with_capacity(16)),
                        b']' | b'P' | b'X' | b'^' | b'_' => Ansi::String,
                        _ => Ansi::Text,
                    }
                }
                Ansi::Csi(params) => {
                    if (0x40..=0x7e).contains(&byte) {
                        if byte == b'h'
                            && [b"?1049".as_slice(), b"?1047", b"?47"].contains(&params.as_slice())
                        {
                            active.interactive = true;
                        }
                        active.ansi = Ansi::Text;
                    } else if params.len() < 32 {
                        params.push(byte);
                    }
                }
                Ansi::String => {
                    if byte == 7 {
                        active.ansi = Ansi::Text;
                    } else if byte == 0x1b {
                        active.ansi = Ansi::StringEscape;
                    }
                }
                Ansi::StringEscape => {
                    active.ansi = if byte == b'\\' {
                        Ansi::Text
                    } else {
                        Ansi::String
                    }
                }
            }
        }
        // Check before trimming: a credential header must remain sticky after leaving the tail.
        for chunk in plain.chunks(512) {
            active.watch.extend_from_slice(chunk);
            active.secret |=
                crate::context::contains_secret(&String::from_utf8_lossy(&active.watch));
            if active.watch.len() > 256 {
                active.watch.drain(..active.watch.len() - 256);
            }
        }
    }
    pub fn is_active(&self) -> bool {
        self.active.as_ref().is_some_and(|a| !a.stopped)
    }
    pub fn discard_active(&mut self) {
        self.active = None;
    }
    pub fn stop_output(&mut self) {
        if let Some(active) = &mut self.active {
            active.stopped = true;
        }
    }
    pub fn input(&mut self, bytes: &[u8]) {
        if bytes.iter().any(|b| !matches!(b, 3 | 4))
            && let Some(active) = &mut self.active
            && !active.stopped
        {
            active.interactive = true;
        }
    }
    pub fn mark_truncated(&mut self) {
        if let Some(active) = &mut self.active {
            active.truncated = true;
        }
    }
    pub fn finish(&mut self, status: i32) {
        let Some(active) = self.active.take() else {
            return;
        };
        if active.secret {
            return;
        }
        let mut truncated = active.truncated;
        let (output, omitted_reason) = if active.interactive {
            (
                String::new(),
                Some("Interactive input or full-screen view: output omitted.".into()),
            )
        } else {
            let mut bytes: Vec<_> = active.tail.into_iter().collect();
            if active.line.iter().any(|b| !b.is_ascii_whitespace()) {
                bytes.extend(active.line);
            }
            let mut output = String::from_utf8_lossy(&bytes).into_owned();
            if output.len() > OUTPUT_LIMIT {
                truncated = true;
                let mut start = output.len() - OUTPUT_LIMIT;
                while !output.is_char_boundary(start) {
                    start += 1;
                }
                output.drain(..start);
            }
            (output, None)
        };
        let record = CaptureRecord {
            command: active.command,
            cwd: active.cwd,
            status,
            output,
            truncated,
            omitted_reason,
        };
        if !safe_record(&record) {
            return;
        }
        self.records.push(record);
        while serde_json::to_vec(&self.records).map_or(true, |b| b.len() > SNAPSHOT_LIMIT) {
            self.records.remove(0);
        }
    }
    pub fn records(&self) -> &[CaptureRecord] {
        &self.records
    }
}

pub fn safe_record(record: &CaptureRecord) -> bool {
    record.command.len() <= 8192
        && record.cwd.len() <= 4096
        && record.output.len() <= OUTPUT_LIMIT
        && (0..=255).contains(&record.status)
        && crate::context::suitable_history(&record.command)
        && !crate::context::contains_secret(&record.cwd)
        && !crate::context::forbidden_directory(Path::new(&record.cwd))
        && !crate::context::contains_secret(&record.output)
        && !record.cwd.chars().any(char::is_control)
        && !record
            .output
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
        && record
            .omitted_reason
            .as_ref()
            .is_none_or(|s| s.len() <= 256 && !s.chars().any(char::is_control))
}

/// Fetch only when opening Amber. The socket and its parent live in a private session directory.
pub fn query(path: &Path) -> Result<Vec<CaptureRecord>, String> {
    use std::os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixStream,
    };
    let meta = std::fs::symlink_metadata(path).map_err(|_| "Output context is unavailable")?;
    if !meta.file_type().is_socket()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
    {
        return Err("Unsafe output context socket".into());
    }
    let mut stream = UnixStream::connect(path).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| e.to_string())?;
    stream.write_all(b"Q\0\0").map_err(|e| e.to_string())?;
    let mut data = vec![];
    stream
        .take((SNAPSHOT_LIMIT + 1) as u64)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() > SNAPSHOT_LIMIT {
        return Err("Output context exceeds the size limit".into());
    }
    let records: Vec<CaptureRecord> = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    if records.iter().any(|record| !safe_record(record)) {
        return Err("Invalid output context".into());
    }
    Ok(records)
}
