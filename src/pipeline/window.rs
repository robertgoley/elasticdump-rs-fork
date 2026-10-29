//! `--offset` / `--size` applied to a stream of NDJSON document lines.

/// Skips the first `skip` documents and passes at most `remaining` after that.
#[derive(Debug, Clone)]
pub struct DocWindow {
    skip: u64,
    remaining: Option<u64>,
}

impl DocWindow {
    pub fn new(offset: u64, size: i64) -> Self {
        Self {
            skip: offset,
            remaining: (size >= 0).then_some(size as u64),
        }
    }

    pub fn unbounded() -> Self {
        Self::new(0, -1)
    }

    pub fn is_passthrough(&self) -> bool {
        self.skip == 0 && self.remaining.is_none()
    }

    /// `true` once `--size` documents have been passed.
    pub fn is_exhausted(&self) -> bool {
        self.remaining == Some(0)
    }

    /// Narrow `buf` (complete `\n`-terminated lines) to the documents inside
    /// the window. Returns the slice and its document count.
    pub fn apply<'a>(&mut self, buf: &'a [u8], docs: u64) -> (&'a [u8], u64) {
        if self.is_passthrough() {
            return (buf, docs);
        }
        let mut start = 0usize;
        let mut docs_left = docs;
        while self.skip > 0 && start < buf.len() {
            match memchr::memchr(b'\n', &buf[start..]) {
                Some(pos) => {
                    start += pos + 1;
                    self.skip -= 1;
                    docs_left = docs_left.saturating_sub(1);
                }
                None => {
                    start = buf.len();
                    self.skip -= 1;
                    docs_left = 0;
                }
            }
        }
        let rest = &buf[start..];
        let Some(remaining) = self.remaining else {
            return (rest, docs_left);
        };
        if docs_left <= remaining {
            self.remaining = Some(remaining - docs_left);
            return (rest, docs_left);
        }
        let mut end = 0usize;
        for _ in 0..remaining {
            match memchr::memchr(b'\n', &rest[end..]) {
                Some(pos) => end += pos + 1,
                None => {
                    end = rest.len();
                    break;
                }
            }
        }
        self.remaining = Some(0);
        (&rest[..end], remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::DocWindow;

    #[test]
    fn passthrough() {
        let mut w = DocWindow::unbounded();
        assert_eq!(w.apply(b"a\nb\n", 2), (&b"a\nb\n"[..], 2));
        assert!(!w.is_exhausted());
    }

    #[test]
    fn offset_spans_batches() {
        let mut w = DocWindow::new(3, -1);
        assert_eq!(w.apply(b"a\nb\n", 2), (&b""[..], 0));
        assert_eq!(w.apply(b"c\nd\ne\n", 3), (&b"d\ne\n"[..], 2));
    }

    #[test]
    fn size_truncates_and_exhausts() {
        let mut w = DocWindow::new(1, 2);
        assert_eq!(w.apply(b"a\nb\n", 2), (&b"b\n"[..], 1));
        assert!(!w.is_exhausted());
        assert_eq!(w.apply(b"c\nd\n", 2), (&b"c\n"[..], 1));
        assert!(w.is_exhausted());
        assert_eq!(w.apply(b"e\n", 1), (&b""[..], 0));
    }

    #[test]
    fn size_zero_passes_nothing() {
        let mut w = DocWindow::new(0, 0);
        assert!(w.is_exhausted());
        assert_eq!(w.apply(b"a\n", 1), (&b""[..], 0));
    }
}
