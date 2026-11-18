#[derive(Debug, Default)]
pub(crate) struct Decoder {
    buf: Vec<u8>,
    name: String,
    data: String,
    has_data: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Event {
    pub(crate) name: String,
    pub(crate) data: String,
}

impl Decoder {
    pub(crate) fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    pub(crate) fn next_event(&mut self) -> Option<Event> {
        while let Some((line_end, next)) = self.line_break() {
            let line = String::from_utf8_lossy(&self.buf[..line_end]).into_owned();
            self.buf.drain(..next);
            if line.is_empty() {
                if let Some(event) = self.dispatch() {
                    return Some(event);
                }
                continue;
            }
            self.field(&line);
        }
        None
    }

    fn line_break(&self) -> Option<(usize, usize)> {
        let at = self.buf.iter().position(|&b| b == b'\n' || b == b'\r')?;
        if self.buf[at] == b'\n' {
            return Some((at, at + 1));
        }
        match self.buf.get(at + 1) {
            Some(b'\n') => Some((at, at + 2)),
            Some(_) => Some((at, at + 1)),
            None => None,
        }
    }

    fn field(&mut self, line: &str) {
        if line.starts_with(':') {
            return;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => value.clone_into(&mut self.name),
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            _ => {}
        }
    }

    fn dispatch(&mut self) -> Option<Event> {
        let name = std::mem::take(&mut self.name);
        if !std::mem::take(&mut self.has_data) {
            return None;
        }
        Some(Event {
            name,
            data: std::mem::take(&mut self.data),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(chunks: &[&str]) -> Vec<Event> {
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        for chunk in chunks {
            decoder.feed(chunk.as_bytes());
            while let Some(event) = decoder.next_event() {
                events.push(event);
            }
        }
        events
    }

    fn event(name: &str, data: &str) -> Event {
        Event {
            name: name.to_owned(),
            data: data.to_owned(),
        }
    }

    #[test]
    fn named_event_is_decoded() {
        let events = decode(&["event: ping\ndata: {}\n\n"]);
        assert_eq!(events, [event("ping", "{}")]);
    }

    #[test]
    fn event_split_across_chunks_is_joined() {
        let events = decode(&["data: {\"a\"", ":1}\n", "\n"]);
        assert_eq!(events, [event("", "{\"a\":1}")]);
    }

    #[test]
    fn multiline_data_is_joined_with_newlines() {
        let events = decode(&["data: one\ndata: two\n\n"]);
        assert_eq!(events, [event("", "one\ntwo")]);
    }

    #[test]
    fn crlf_split_between_chunks_is_one_line_break() {
        let events = decode(&["data: x\r", "\n\r\n"]);
        assert_eq!(events, [event("", "x")]);
    }

    #[test]
    fn comments_and_empty_events_are_skipped() {
        let events = decode(&[": keepalive\n\nevent: a\n\ndata: b\n\n"]);
        assert_eq!(events, [event("", "b")]);
    }
}
