//! A tiny deterministic JSON writer (no maps, no float formatting surprises).

pub struct J {
    pub b: Vec<u8>,
    /// Per open container: has it got a member already (so the next needs ',')?
    stack: Vec<bool>,
    after_key: bool,
}

impl Default for J {
    fn default() -> Self {
        Self::with_capacity(1024)
    }
}

impl J {
    pub fn with_capacity(n: usize) -> Self {
        Self {
            b: Vec::with_capacity(n),
            stack: Vec::with_capacity(16),
            after_key: false,
        }
    }
    pub fn clear(&mut self) {
        self.b.clear();
        self.stack.clear();
        self.after_key = false;
    }
    fn sep(&mut self) {
        if self.after_key {
            self.after_key = false;
            return;
        }
        if let Some(n) = self.stack.last_mut() {
            if *n {
                self.b.push(b',');
            } else {
                *n = true;
            }
        }
    }
    pub fn obj(&mut self) -> &mut Self {
        self.sep();
        self.b.push(b'{');
        self.stack.push(false);
        self
    }
    pub fn arr(&mut self) -> &mut Self {
        self.sep();
        self.b.push(b'[');
        self.stack.push(false);
        self
    }
    pub fn end_obj(&mut self) -> &mut Self {
        self.stack.pop();
        self.b.push(b'}');
        self
    }
    pub fn end_arr(&mut self) -> &mut Self {
        self.stack.pop();
        self.b.push(b']');
        self
    }
    pub fn key(&mut self, k: &str) -> &mut Self {
        self.sep();
        esc(&mut self.b, k);
        self.b.push(b':');
        self.after_key = true;
        self
    }
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.sep();
        esc(&mut self.b, s);
        self
    }
    /// Pre-encoded JSON text (trusted, from this crate only).
    pub fn raw(&mut self, s: &str) -> &mut Self {
        self.sep();
        self.b.extend_from_slice(s.as_bytes());
        self
    }
    pub fn u(&mut self, v: u64) -> &mut Self {
        self.sep();
        push_u(&mut self.b, v);
        self
    }
    pub fn i(&mut self, v: i64) -> &mut Self {
        self.sep();
        if v < 0 {
            self.b.push(b'-');
        }
        push_u(&mut self.b, v.unsigned_abs());
        self
    }
    /// Fixed-point decimal `v / 10^scale`, e.g. (12345, 2) -> 123.45.
    pub fn fixed(&mut self, v: i64, scale: u32) -> &mut Self {
        self.sep();
        if v < 0 {
            self.b.push(b'-');
        }
        let a = v.unsigned_abs();
        let p = 10u64.pow(scale);
        push_u(&mut self.b, a / p);
        if scale > 0 {
            self.b.push(b'.');
            let f = a % p;
            let mut d = p / 10;
            while d > 0 {
                self.b.push(b'0' + ((f / d) % 10) as u8);
                d /= 10;
            }
        }
        self
    }
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.sep();
        self.b.extend_from_slice(if v { b"true" } else { b"false" });
        self
    }
    pub fn null(&mut self) -> &mut Self {
        self.sep();
        self.b.extend_from_slice(b"null");
        self
    }
    pub fn kstr(&mut self, k: &str, v: &str) -> &mut Self {
        self.key(k).str(v)
    }
    pub fn ku(&mut self, k: &str, v: u64) -> &mut Self {
        self.key(k).u(v)
    }
}

pub fn push_u(out: &mut Vec<u8>, mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out.extend_from_slice(&buf[i..]);
}

pub fn esc(out: &mut Vec<u8>, s: &str) {
    out.push(b'"');
    let bytes = s.as_bytes();
    // Fast path (vectorisable): nothing to escape.
    if !bytes.iter().any(|&c| c < 0x20 || c == b'"' || c == b'\\') {
        out.extend_from_slice(bytes);
        out.push(b'"');
        return;
    }
    let mut start = 0;
    for (i, &c) in bytes.iter().enumerate() {
        let rep: Option<&[u8]> = match c {
            b'"' => Some(b"\\\""),
            b'\\' => Some(b"\\\\"),
            b'\n' => Some(b"\\n"),
            b'\r' => Some(b"\\r"),
            b'\t' => Some(b"\\t"),
            0x08 => Some(b"\\b"),
            0x0c => Some(b"\\f"),
            c if c < 0x20 => None,
            _ => continue,
        };
        out.extend_from_slice(&bytes[start..i]);
        match rep {
            Some(r) => out.extend_from_slice(r),
            None => out.extend_from_slice(format!("\\u{c:04x}").as_bytes()),
        }
        start = i + 1;
    }
    out.extend_from_slice(&bytes[start..]);
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn writes_valid_json() {
        let mut j = J::default();
        j.obj()
            .kstr("a", "x\"y\n\u{1}")
            .key("b")
            .arr()
            .u(1)
            .i(-2)
            .fixed(-12345, 2)
            .fixed(5, 3)
            .end_arr()
            .key("c")
            .obj()
            .end_obj()
            .key("d")
            .null()
            .end_obj();
        let s = String::from_utf8(j.b.clone()).unwrap();
        assert_eq!(
            s,
            r#"{"a":"x\"y\n\u0001","b":[1,-2,-123.45,0.005],"c":{},"d":null}"#
        );
        let _: serde_json::Value = serde_json::from_str(&s).unwrap();
    }
}
