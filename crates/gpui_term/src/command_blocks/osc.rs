#[derive(Debug, Default)]
pub struct OscStreamParser {
    pending_start_esc: bool,
    in_osc: bool,
    osc_id: u32,
    osc_id_digits: bool,
    osc_seen_semicolon: bool,
    osc_payload: Vec<u8>,
    osc_pending_st_esc: bool,
    osc_invalid: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OscEvent {
    Osc133(String),
}

/// Offset is the exclusive end position in the pushed `bytes` slice.
pub type OscCompletion = (usize, OscEvent);

impl OscStreamParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<OscEvent> {
        self.push_with_offsets(bytes)
            .into_iter()
            .map(|(_, ev)| ev)
            .collect()
    }

    pub fn push_with_offsets(&mut self, bytes: &[u8]) -> Vec<OscCompletion> {
        let mut out: Vec<OscCompletion> = Vec::new();

        let mut i = 0usize;
        while i < bytes.len() {
            let b = bytes[i];

            if !self.in_osc {
                if self.pending_start_esc {
                    self.pending_start_esc = false;
                    if b == b']' {
                        self.begin_osc();
                        i += 1;
                        continue;
                    }
                }

                if b == 0x1b {
                    self.pending_start_esc = i + 1 == bytes.len();
                    if !self.pending_start_esc && bytes[i + 1] == b']' {
                        self.begin_osc();
                        i += 2;
                        continue;
                    }
                }

                i += 1;
                continue;
            }

            // In OSC body.
            if self.osc_pending_st_esc {
                self.osc_pending_st_esc = false;
                if b == b'\\' {
                    self.finish_osc(&mut out, i + 1);
                    i += 1;
                    continue;
                }
                self.push_osc_byte(0x1b);
                self.push_osc_byte(b);
                i += 1;
                continue;
            }

            match b {
                0x07 => {
                    self.finish_osc(&mut out, i + 1);
                    i += 1;
                }
                0x1b => {
                    if i + 1 == bytes.len() {
                        self.osc_pending_st_esc = true;
                        i += 1;
                    } else if bytes[i + 1] == b'\\' {
                        self.finish_osc(&mut out, i + 2);
                        i += 2;
                    } else {
                        self.push_osc_byte(b);
                        i += 1;
                    }
                }
                _ => {
                    self.push_osc_byte(b);
                    i += 1;
                }
            }
        }

        out
    }

    fn begin_osc(&mut self) {
        self.in_osc = true;
        self.osc_id = 0;
        self.osc_id_digits = false;
        self.osc_seen_semicolon = false;
        self.osc_payload.clear();
        self.osc_pending_st_esc = false;
        self.osc_invalid = false;
    }

    fn push_osc_byte(&mut self, b: u8) {
        if self.osc_invalid {
            return;
        }

        if !self.osc_seen_semicolon {
            if b.is_ascii_digit() {
                self.osc_id_digits = true;
                self.osc_id = match self
                    .osc_id
                    .checked_mul(10)
                    .and_then(|v| v.checked_add(u32::from(b - b'0')))
                {
                    Some(v) => v,
                    None => {
                        self.osc_invalid = true;
                        return;
                    }
                };
                return;
            }

            if b == b';' && self.osc_id_digits {
                self.osc_seen_semicolon = true;
                return;
            }

            self.osc_invalid = true;
            return;
        }

        if self.osc_id == 133 {
            self.osc_payload.push(b);
        }
    }

    fn finish_osc(&mut self, out: &mut Vec<OscCompletion>, end_offset: usize) {
        if !self.osc_invalid && self.osc_seen_semicolon && self.osc_id == 133 {
            let payload = String::from_utf8_lossy(&self.osc_payload).to_string();
            out.push((end_offset, OscEvent::Osc133(payload)));
        }

        self.in_osc = false;
        self.osc_pending_st_esc = false;
        self.osc_payload.clear();
        self.osc_invalid = false;
        self.osc_id_digits = false;
        self.osc_seen_semicolon = false;
        self.osc_id = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc_stream_parser_reads_bel_terminated_sequences() {
        let mut p = OscStreamParser::new();
        let events = p.push(b"\x1b]133;C\x07");
        assert_eq!(events, vec![OscEvent::Osc133("C".to_string())]);
    }

    #[test]
    fn osc_stream_parser_reads_st_terminated_sequences() {
        let mut p = OscStreamParser::new();
        let events = p.push(b"\x1b]133;D\x1b\\");
        assert_eq!(events, vec![OscEvent::Osc133("D".to_string())]);
    }

    #[test]
    fn osc_stream_parser_handles_split_reads() {
        let mut p = OscStreamParser::new();
        assert_eq!(p.push(b"\x1b]133;C"), Vec::<OscEvent>::new());
        assert_eq!(p.push(b"\x07"), vec![OscEvent::Osc133("C".to_string())]);
    }

    #[test]
    fn osc_stream_parser_ignores_other_osc_ids() {
        let mut p = OscStreamParser::new();
        let events = p.push(b"\x1b]7;file://localhost/tmp\x07");
        assert_eq!(events, Vec::<OscEvent>::new());
    }

    #[test]
    fn osc_stream_parser_ignores_malformed_sequences() {
        let mut p = OscStreamParser::new();
        let events = p.push(b"\x1b]133C\x07");
        assert_eq!(events, Vec::<OscEvent>::new());
    }

    #[test]
    fn osc_stream_parser_keeps_trailing_esc() {
        let mut p = OscStreamParser::new();
        assert_eq!(p.push(b"\x1b"), Vec::<OscEvent>::new());
        assert_eq!(
            p.push(b"]133;A\x07"),
            vec![OscEvent::Osc133("A".to_string())]
        );
    }
}
