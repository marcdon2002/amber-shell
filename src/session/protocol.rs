//! Private markers share the PTY stream, so command boundaries cannot overtake output.
const FRAME_LIMIT: usize = 48 * 1024;
#[derive(Debug, PartialEq)]
pub enum Event {
    Output(Vec<u8>),
    Begin(String, String),
    Stop,
    End(i32),
    Widget(bool),
}
pub struct Decoder {
    prefix: Vec<u8>,
    pending: Vec<u8>,
}
impl Decoder {
    pub fn new(token: &str) -> Self {
        Self {
            prefix: format!("\x1b]777;amber;{token};").into_bytes(),
            pending: vec![],
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.pending.extend_from_slice(bytes);
        let mut events = vec![];
        loop {
            let start = self
                .pending
                .windows(self.prefix.len())
                .position(|w| w == self.prefix);
            let Some(start) = start else {
                let hold = (1..self.prefix.len().min(self.pending.len() + 1))
                    .rev()
                    .find(|n| self.pending.ends_with(&self.prefix[..*n]))
                    .unwrap_or(0);
                let count = self.pending.len() - hold;
                if count > 0 {
                    events.push(Event::Output(self.pending.drain(..count).collect()));
                }
                break;
            };
            if start > 0 {
                events.push(Event::Output(self.pending.drain(..start).collect()));
            }
            let Some(end) = self.pending[self.prefix.len()..]
                .iter()
                .position(|b| *b == 7)
                .map(|i| i + self.prefix.len())
            else {
                if self.pending.len() > FRAME_LIMIT {
                    self.pending.clear();
                } // malformed private frame: fail closed
                break;
            };
            let payload = &self.pending[self.prefix.len()..end];
            let fields: Vec<_> = payload.split(|b| *b == 0).collect();
            match fields.as_slice() {
                [b"B", command, cwd] => {
                    if let (Some(command), Some(cwd)) = (decode(command), decode(cwd)) {
                        events.push(Event::Begin(command, cwd));
                    }
                }
                [b"S"] => events.push(Event::Stop),
                [b"E", status] => {
                    if let Ok(status) = String::from_utf8_lossy(status).parse::<i32>()
                        && (0..=255).contains(&status)
                    {
                        events.push(Event::End(status));
                    }
                }
                [b"W"] => events.push(Event::Widget(true)),
                [b"P"] => events.push(Event::Widget(false)),
                _ => {}
            }
            self.pending.drain(..=end);
        }
        events
    }
}
fn decode(bytes: &[u8]) -> Option<String> {
    let mut result = vec![];
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let pair = bytes.get(i + 1..i + 3)?;
            let value = std::str::from_utf8(pair)
                .ok()
                .and_then(|s| u8::from_str_radix(s, 16).ok())?;
            result.push(value);
            i += 3;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(result).ok()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markers_remain_ordered_across_all_chunk_boundaries() {
        let raw=b"prompt\x1b]777;amber;test;B\0printf '%25s%0A'\0/demo\x07first\x1b]777;amber;test;S\x07prompt marker\x1b]777;amber;test;E\x001\x07";
        for size in 1..raw.len() {
            let mut decoder = Decoder::new("test");
            let mut events = vec![];
            for chunk in raw.chunks(size) {
                events.extend(decoder.feed(chunk));
            }
            let begin = events
                .iter()
                .position(|e| matches!(e, Event::Begin(_, _)))
                .unwrap();
            assert_eq!(
                events[begin],
                Event::Begin("printf '%s\n'".into(), "/demo".into())
            );
            let stop = events
                .iter()
                .position(|e| matches!(e, Event::Stop))
                .unwrap();
            let end = events
                .iter()
                .position(|e| matches!(e, Event::End(1)))
                .unwrap();
            assert!(begin < stop && stop < end);
            let output: Vec<u8> = events
                .iter()
                .filter_map(|e| {
                    if let Event::Output(b) = e {
                        Some(b.as_slice())
                    } else {
                        None
                    }
                })
                .flatten()
                .copied()
                .collect();
            assert_eq!(output, b"promptfirstprompt marker");
        }
    }
}
