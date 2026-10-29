//! Incremental splitter for a stream of top-level JSON values: NDJSON,
//! concatenated JSON and pretty-printed JSON all work. Values are returned as
//! raw bytes (lossless) and minified onto a single line when needed.

/// Feed bytes with [`ValueSplitter::push`]; complete values come out of
/// [`ValueSplitter::next_value`].
#[derive(Debug, Default)]
pub struct ValueSplitter {
    buf: Vec<u8>,
    /// Scan position inside `buf`.
    pos: usize,
    /// Start of the value being scanned, if one has begun.
    start: Option<usize>,
    depth: u32,
    in_string: bool,
    escaped: bool,
    /// Scalar top-level value (number/literal/string) being scanned.
    scalar: bool,
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\n' | b'\r' | b'\t') || b == 0xEF || b == 0xBB || b == 0xBF
}

impl ValueSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    fn compact(&mut self) {
        let keep_from = self.start.unwrap_or(self.pos);
        if keep_from > 0 {
            self.buf.drain(..keep_from);
            self.pos -= keep_from;
            if let Some(s) = self.start.as_mut() {
                *s = 0;
            }
        }
    }

    /// Next complete value, if any. With `eof`, a trailing scalar without a
    /// terminator is returned too, and an unterminated value is an error.
    pub fn next_value(&mut self, eof: bool) -> Result<Option<Vec<u8>>, String> {
        loop {
            if self.pos >= self.buf.len() {
                if eof {
                    if let Some(start) = self.start {
                        if self.scalar && !self.in_string {
                            let v = self.buf[start..].to_vec();
                            self.reset_value();
                            self.buf.clear();
                            self.pos = 0;
                            return Ok(Some(v));
                        }
                        return Err("unexpected end of input inside a JSON value".to_string());
                    }
                }
                self.compact();
                return Ok(None);
            }
            let b = self.buf[self.pos];
            match self.start {
                None => {
                    if is_ws(b) || b == b',' {
                        self.pos += 1;
                        continue;
                    }
                    self.start = Some(self.pos);
                    match b {
                        b'{' | b'[' => {
                            self.depth = 1;
                            self.scalar = false;
                        }
                        b'"' => {
                            self.scalar = true;
                            self.in_string = true;
                        }
                        _ => self.scalar = true,
                    }
                    self.pos += 1;
                }
                Some(start) => {
                    if self.in_string {
                        if self.escaped {
                            self.escaped = false;
                        } else if b == b'\\' {
                            self.escaped = true;
                        } else if b == b'"' {
                            self.in_string = false;
                        }
                        self.pos += 1;
                        continue;
                    }
                    if self.scalar {
                        if is_ws(b) || b == b',' || b == b'{' || b == b'[' {
                            let v = self.buf[start..self.pos].to_vec();
                            self.reset_value();
                            return Ok(Some(v));
                        }
                        if b == b'"' {
                            self.in_string = true;
                        }
                        self.pos += 1;
                        continue;
                    }
                    match b {
                        b'"' => self.in_string = true,
                        b'{' | b'[' => self.depth += 1,
                        b'}' | b']' => {
                            self.depth -= 1;
                            if self.depth == 0 {
                                self.pos += 1;
                                let v = self.buf[start..self.pos].to_vec();
                                self.reset_value();
                                return Ok(Some(v));
                            }
                        }
                        _ => {}
                    }
                    self.pos += 1;
                }
            }
        }
    }

    fn reset_value(&mut self) {
        self.start = None;
        self.depth = 0;
        self.in_string = false;
        self.escaped = false;
        self.scalar = false;
    }
}

/// Remove insignificant whitespace (outside strings) so a value fits on one
/// line. Returns the input unchanged when it has no line breaks.
pub fn single_line(value: Vec<u8>) -> Vec<u8> {
    if memchr::memchr2(b'\n', b'\r', &value).is_none() {
        return value;
    }
    let mut out = Vec::with_capacity(value.len());
    let mut in_string = false;
    let mut escaped = false;
    for &b in &value {
        if in_string {
            out.push(b);
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b' ' | b'\n' | b'\r' | b'\t' => {}
            b'"' => {
                in_string = true;
                out.push(b);
            }
            _ => out.push(b),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(input: &[u8], chunk: usize) -> Vec<String> {
        let mut s = ValueSplitter::new();
        let mut out = vec![];
        for c in input.chunks(chunk.max(1)) {
            s.push(c);
            while let Some(v) = s.next_value(false).unwrap() {
                out.push(String::from_utf8(single_line(v)).unwrap());
            }
        }
        while let Some(v) = s.next_value(true).unwrap() {
            out.push(String::from_utf8(single_line(v)).unwrap());
        }
        out
    }

    #[test]
    fn ndjson_concatenated_and_pretty() {
        let input = b"{\"a\":1}\n{\"b\":\"x}\\\"{\"}{\"c\":[1,{\"d\":2}]}\n{\n  \"e\": \"keep  spaces\",\n  \"f\": 1\n}\n";
        for chunk in [1, 3, 7, 1000] {
            assert_eq!(
                all(input, chunk),
                vec![
                    r#"{"a":1}"#,
                    r#"{"b":"x}\"{"}"#,
                    r#"{"c":[1,{"d":2}]}"#,
                    r#"{"e":"keep  spaces","f":1}"#
                ],
                "chunk {chunk}"
            );
        }
    }

    #[test]
    fn crlf_and_bom_and_big_ints() {
        let input = b"\xEF\xBB\xBF{\"n\":123456789012345678901234567890}\r\n{\"m\":-0.5e10}\r\n";
        assert_eq!(
            all(input, 4),
            vec![
                r#"{"n":123456789012345678901234567890}"#,
                r#"{"m":-0.5e10}"#
            ]
        );
    }

    #[test]
    fn top_level_scalars_and_strings() {
        assert_eq!(
            all(b"\"{\\\"x\\\":1}\"\n42", 2),
            vec![r#""{\"x\":1}""#, "42"]
        );
    }

    #[test]
    fn unterminated_value_is_an_error() {
        let mut s = ValueSplitter::new();
        s.push(b"{\"a\":");
        assert!(s.next_value(false).unwrap().is_none());
        assert!(s.next_value(true).is_err());
    }
}
