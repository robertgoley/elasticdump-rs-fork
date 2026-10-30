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
    /// Largest value accepted, in bytes (`None` = unlimited). Buffering
    /// stops as soon as a value grows past it, so a giant value cannot
    /// exhaust memory.
    max_value: Option<usize>,
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\n' | b'\r' | b'\t') || b == 0xEF || b == 0xBB || b == 0xBF
}

impl ValueSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Refuse values larger than `max` bytes (see [`ValueSplitter::next_value`]).
    pub fn with_max_value(max: Option<usize>) -> Self {
        Self {
            max_value: max,
            ..Self::default()
        }
    }

    fn check_size(&self, len: usize) -> Result<(), String> {
        match self.max_value {
            Some(max) if len > max => Err(format!(
                "a single document is larger than {} (it was not read further)",
                human_size(max)
            )),
            _ => Ok(()),
        }
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
                            self.check_size(self.buf.len() - start)?;
                            let v = self.buf[start..].to_vec();
                            self.reset_value();
                            self.buf.clear();
                            self.pos = 0;
                            return Ok(Some(v));
                        }
                        return Err("unexpected end of input inside a JSON value".to_string());
                    }
                }
                if let Some(start) = self.start {
                    self.check_size(self.buf.len() - start)?;
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
                            self.check_size(self.pos - start)?;
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
                                self.check_size(self.pos - start)?;
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

fn human_size(n: usize) -> String {
    if n >= 1 << 20 && n % (1 << 20) == 0 {
        format!("{} MB", n >> 20)
    } else {
        format!("{n} bytes")
    }
}

/// Remove insignificant whitespace (outside strings) so a value fits on one
/// line. Returns the input unchanged when it has no line breaks.
///
/// Raw CR/LF bytes *inside* strings (invalid JSON, but a lenient parser may
/// let them through) are escaped as `\r`/`\n`, so the result never contains
/// a line break: one record is always exactly one NDJSON line (INJ-F1).
pub fn single_line(value: Vec<u8>) -> Vec<u8> {
    if memchr::memchr2(b'\n', b'\r', &value).is_none() {
        return value;
    }
    flatten(&value)
}

/// Borrowing variant of [`single_line`]: no copy in the common case.
pub fn single_line_slice(value: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if memchr::memchr2(b'\n', b'\r', value).is_none() {
        return std::borrow::Cow::Borrowed(value);
    }
    std::borrow::Cow::Owned(flatten(value))
}

fn flatten(value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len());
    let mut in_string = false;
    let mut escaped = false;
    for &b in value {
        if in_string {
            match b {
                b'\n' => out.extend_from_slice(b"\\n"),
                b'\r' => out.extend_from_slice(b"\\r"),
                _ => out.push(b),
            }
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
    fn values_over_the_size_cap_are_refused_without_buffering_them() {
        // a complete value just over the cap, inside one chunk
        let mut s = ValueSplitter::with_max_value(Some(16));
        s.push(b"{\"a\":1}\n{\"b\":\"0123456789abcdef\"}\n");
        assert_eq!(s.next_value(false).unwrap().unwrap(), b"{\"a\":1}");
        let e = s.next_value(false).unwrap_err();
        assert!(e.contains("larger than 16 bytes"), "{e}");
        // an unterminated giant value is refused once the buffer passes the cap
        let mut s = ValueSplitter::with_max_value(Some(1 << 20));
        s.push(b"{\"big\":\"");
        let chunk = vec![b'x'; 64 << 10];
        let mut refused = None;
        for i in 0..64 {
            s.push(&chunk);
            if let Err(e) = s.next_value(false) {
                refused = Some((i, e));
                break;
            }
        }
        let (i, e) = refused.expect("refused");
        assert!(i <= 16, "refused after {i} chunks");
        assert!(e.contains("larger than 1 MB"), "{e}");
        // a scalar at EOF and at the cap exactly is fine
        let mut s = ValueSplitter::with_max_value(Some(4));
        s.push(b"1234");
        assert_eq!(s.next_value(true).unwrap().unwrap(), b"1234");
    }

    #[test]
    fn unterminated_value_is_an_error() {
        let mut s = ValueSplitter::new();
        s.push(b"{\"a\":");
        assert!(s.next_value(false).unwrap().is_none());
        assert!(s.next_value(true).is_err());
    }

    #[test]
    fn single_line_never_leaves_a_line_break() {
        let pretty = b"{\"a\":\r\n\t{\"_index\":\"victim\",\n \"s\":\"x\ny\r\"}\n}".to_vec();
        let flat = single_line(pretty.clone());
        assert!(memchr::memchr2(b'\n', b'\r', &flat).is_none());
        assert_eq!(flat, br#"{"a":{"_index":"victim","s":"x\ny\r"}}"#.to_vec());
        assert_eq!(single_line_slice(&pretty).into_owned(), flat);
        assert!(matches!(
            single_line_slice(b"{\"a\": 1}"),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}
