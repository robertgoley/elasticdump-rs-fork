//! Converters (TST-DATA-005/010): external corpora -> elasticdump format
//! (`{"_index","_id","_source"}` per line) with deterministic ids
//! `_id = hex(SHA-256(dataset ‖ 0 ‖ file ‖ 0 ‖ line_no))[..20]`.
//!
//! Kinds:
//! * `rally`: one JSON source document per line (Rally/OSB corpora);
//! * `clf`: Common/Combined Log Format (NASA, SecRepo, Kaggle, flog, apache_logs);
//! * `loghub`: raw log lines (Loghub sets); a timestamp is extracted for the
//!   known formats, the raw line is always kept in `message`.

use crate::json::{J, esc};
use anyhow::{Result, bail};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::io::{BufRead, Write};

pub fn det_id(dataset: &str, file: &str, line_no: u64) -> String {
    let mut h = Sha256::new();
    h.update(dataset.as_bytes());
    h.update([0]);
    h.update(file.as_bytes());
    h.update([0]);
    h.update(line_no.to_string().as_bytes());
    let d = h.finalize();
    crate::text::hex(&d[..10])
}

#[derive(Default, Debug)]
pub struct Stats {
    pub lines: u64,
    pub written: u64,
    pub skipped: u64,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `10/Oct/2000:13:55:36 -0700` -> RFC 3339 with offset.
fn clf_time(t: &str) -> Option<String> {
    // dd/Mon/yyyy:HH:MM:SS zone
    let (d, rest) = t.split_once('/')?;
    let (mon, rest) = rest.split_once('/')?;
    let (y, rest) = rest.split_once(':')?;
    let (hms, zone) = rest.split_once(' ').unwrap_or((rest, "+0000"));
    let m = MONTHS.iter().position(|x| *x == mon)? + 1;
    if zone.len() != 5 {
        return None;
    }
    Some(format!(
        "{y}-{m:02}-{:0>2}T{hms}{}:{}",
        d,
        &zone[..3],
        &zone[3..]
    ))
}

pub struct Converter {
    clf: Regex,
    loghub: Vec<(Regex, &'static str)>,
}

impl Default for Converter {
    fn default() -> Self {
        Self::new()
    }
}

impl Converter {
    pub fn new() -> Self {
        let clf = Regex::new(r#"^(\S+) (\S+) (\S+) \[([^\]]+)\] "([^"]*)" (\d{3}|-) (\d+|-)(?: "([^"]*)" "([^"]*)")?"#).unwrap();
        let loghub = vec![
            // HDFS: 081109 203615 148 INFO dfs.DataNode$PacketResponder: ...
            (Regex::new(r"^(\d{2})(\d{2})(\d{2}) (\d{2})(\d{2})(\d{2}) \d+ (\w+) ([^:]+): ").unwrap(), "hdfs"),
            // Spark/Hadoop-ish: 17/06/09 20:10:40 INFO executor.CoarseGrainedExecutorBackend: ...
            (Regex::new(r"^(\d{2})/(\d{2})/(\d{2}) (\d{2}):(\d{2}):(\d{2}) (\w+) ([^:]+): ").unwrap(), "spark"),
            // BGL: - 1117838570 2005.06.03 R02-M1-N0-C:J12-U11 2005-06-03-15.42.50.675872 ... RAS KERNEL INFO ...
            (Regex::new(r"^\S+ \d+ \d{4}\.\d{2}\.\d{2} \S+ (\d{4})-(\d{2})-(\d{2})-(\d{2})\.(\d{2})\.(\d{2})\.\d+ \S+ \S+ (\S+) (\w+) ").unwrap(), "bgl"),
            // Generic ISO: 2015-10-18 18:01:47,978 INFO [main] ...
            (Regex::new(r"^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2}):(\d{2})[,.]?\d* +(\w+) +(\S+)").unwrap(), "iso"),
        ];
        Self { clf, loghub }
    }

    fn source_clf(&self, line: &str, j: &mut J) -> bool {
        let Some(c) = self.clf.captures(line) else {
            return false;
        };
        j.obj();
        if let Some(t) = clf_time(&c[4]) {
            j.kstr("@timestamp", &t);
        }
        j.kstr("host", &c[1]);
        if &c[3] != "-" {
            j.kstr("user", &c[3]);
        }
        let req = &c[5];
        let mut parts = req.splitn(3, ' ');
        let (m, p, v) = (parts.next(), parts.next(), parts.next());
        j.kstr("request", req);
        if let (Some(m), Some(p)) = (m, p) {
            j.kstr("method", m).kstr("path", p);
        }
        if let Some(v) = v {
            j.kstr("http_version", v);
        }
        if let Ok(s) = c[6].parse::<u64>() {
            j.ku("status", s);
        }
        if let Ok(s) = c[7].parse::<u64>() {
            j.ku("size", s);
        }
        if let Some(r) = c.get(8) {
            j.kstr("referrer", r.as_str());
        }
        if let Some(u) = c.get(9) {
            j.kstr("user_agent", u.as_str());
        }
        j.end_obj();
        true
    }

    fn source_loghub(&self, line: &str, set: &str, j: &mut J) {
        j.obj();
        for (re, kind) in &self.loghub {
            if let Some(c) = re.captures(line) {
                let g = |k: usize| c.get(k).map_or("", |m| m.as_str());
                let ts = match *kind {
                    // two-digit years (yyMMdd / yy/MM/dd)
                    "hdfs" | "spark" => {
                        format!("20{}-{}-{}T{}:{}:{}Z", g(1), g(2), g(3), g(4), g(5), g(6))
                    }
                    _ => format!("{}-{}-{}T{}:{}:{}Z", g(1), g(2), g(3), g(4), g(5), g(6)),
                };
                j.kstr("@timestamp", &ts)
                    .kstr("level", g(7))
                    .kstr("component", g(8));
                break;
            }
        }
        j.kstr("source", &format!("loghub/{set}"))
            .kstr("message", line)
            .end_obj();
    }

    /// Convert one input stream. `index` names the target index; `dataset`
    /// and `file` feed the deterministic id.
    #[allow(clippy::too_many_arguments)]
    pub fn run<R: BufRead, W: Write>(
        &self,
        kind: &str,
        dataset: &str,
        file: &str,
        index: &str,
        max_docs: Option<u64>,
        input: R,
        out: &mut W,
    ) -> Result<Stats> {
        if !["rally", "clf", "loghub"].contains(&kind) {
            bail!("unknown converter kind {kind} (rally, clf, loghub)");
        }
        let mut st = Stats::default();
        let mut j = J::with_capacity(4096);
        let mut line_buf = Vec::with_capacity(4096);
        let mut input = input;
        let mut head = Vec::with_capacity(64);
        esc(&mut head, index);
        loop {
            if max_docs.is_some_and(|m| st.written >= m) {
                break;
            }
            line_buf.clear();
            if input.read_until(b'\n', &mut line_buf)? == 0 {
                break;
            }
            st.lines += 1;
            while matches!(line_buf.last(), Some(b'\n' | b'\r')) {
                line_buf.pop();
            }
            if line_buf.iter().all(u8::is_ascii_whitespace) {
                st.skipped += 1;
                continue;
            }
            j.clear();
            let ok = match kind {
                "rally" => {
                    // must be exactly one JSON object; copy its bytes verbatim
                    if line_buf.first() == Some(&b'{')
                        && sonic_rs::from_slice::<serde::de::IgnoredAny>(&line_buf).is_ok()
                    {
                        j.b.extend_from_slice(&line_buf);
                        true
                    } else {
                        false
                    }
                }
                "clf" => self.source_clf(&String::from_utf8_lossy(&line_buf), &mut j),
                _ => {
                    self.source_loghub(&String::from_utf8_lossy(&line_buf), dataset, &mut j);
                    true
                }
            };
            if !ok {
                st.skipped += 1;
                continue;
            }
            out.write_all(b"{\"_index\":")?;
            out.write_all(&head)?;
            out.write_all(b",\"_id\":\"")?;
            out.write_all(det_id(dataset, file, st.lines).as_bytes())?;
            out.write_all(b"\",\"_source\":")?;
            out.write_all(&j.b)?;
            out.write_all(b"}\n")?;
            st.written += 1;
        }
        Ok(st)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(kind: &str, input: &str) -> (String, Stats) {
        let mut out = Vec::new();
        let st = Converter::new()
            .run(kind, "ds", "f1", "idx", None, input.as_bytes(), &mut out)
            .unwrap();
        (String::from_utf8(out).unwrap(), st)
    }

    #[test]
    fn clf_combined_and_ids_are_deterministic() {
        let l = r#"199.72.81.55 - - [01/Jul/1995:00:00:01 -0400] "GET /history/apollo/ HTTP/1.0" 200 6245
127.0.0.1 - frank [10/Oct/2000:13:55:36 -0700] "GET /a.gif HTTP/1.0" 200 2326 "http://ex.com/" "Mozilla/4.08"
garbage line
"#;
        let (a, st) = conv("clf", l);
        let (b, _) = conv("clf", l);
        assert_eq!(a, b);
        assert_eq!((st.written, st.skipped), (2, 1));
        let first: serde_json::Value = serde_json::from_str(a.lines().next().unwrap()).unwrap();
        assert_eq!(first["_source"]["@timestamp"], "1995-07-01T00:00:01-04:00");
        assert_eq!(first["_source"]["status"], 200);
        assert_eq!(first["_id"], det_id("ds", "f1", 1));
        let second: serde_json::Value = serde_json::from_str(a.lines().nth(1).unwrap()).unwrap();
        assert_eq!(second["_source"]["user_agent"], "Mozilla/4.08");
        assert_eq!(second["_source"]["user"], "frank");
    }

    #[test]
    fn rally_lines_copied_verbatim() {
        let (a, st) = conv(
            "rally",
            "{\"@timestamp\": 893964617, \"clientip\":\"40.135.0.0\"}\nnot json\n{\"a\":1} trailing\n",
        );
        assert_eq!((st.written, st.skipped), (1, 2));
        assert!(a.contains("\"_source\":{\"@timestamp\": 893964617, \"clientip\":\"40.135.0.0\"}"));
    }

    #[test]
    fn loghub_timestamps() {
        let (a, _) = conv(
            "loghub",
            "081109 203615 148 INFO dfs.DataNode$PacketResponder: PacketResponder 1 for block blk_38865049064139660 terminating\n17/06/09 20:10:40 INFO executor.CoarseGrainedExecutorBackend: Registered signal handlers\nweird \u{1} line \"q\"\n",
        );
        let v: Vec<serde_json::Value> = a
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(v[0]["_source"]["@timestamp"], "2008-11-09T20:36:15Z");
        assert_eq!(v[0]["_source"]["level"], "INFO");
        assert_eq!(v[1]["_source"]["@timestamp"], "2017-06-09T20:10:40Z");
        assert_eq!(v[2]["_source"]["message"], "weird \u{1} line \"q\"");
    }
}
