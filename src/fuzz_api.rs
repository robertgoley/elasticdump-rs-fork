//! Entry points for the cargo-fuzz targets in `fuzz/` (TST-API-084). Built
//! only with `--features fuzz` (and in unit tests, which smoke-run each one).
//! Every function takes arbitrary bytes and must never panic.

use std::cell::RefCell;

use crate::pipeline::json_split::{ValueSplitter, single_line};

/// NDJSON/concatenated splitter: the first byte picks the chunk size.
pub fn json_split(data: &[u8]) {
    let Some((&n, rest)) = data.split_first() else {
        return;
    };
    let chunk = (n as usize % 64) + 1;
    let mut s = ValueSplitter::with_max_value(Some(1 << 20));
    for c in rest.chunks(chunk) {
        s.push(c);
        while let Ok(Some(v)) = s.next_value(false) {
            let line = single_line(v);
            assert!(!line.contains(&b'\n'));
        }
    }
    while let Ok(Some(v)) = s.next_value(true) {
        let _ = single_line(v);
    }
}

/// Metadata line decoding and re-encoding.
pub fn codec_decode(data: &[u8]) {
    use crate::meta::codec::{LineEncoding, decode_any, encode_line};
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    if let Ok(doc) = decode_any(v) {
        for enc in [LineEncoding::StringLiteral, LineEncoding::Object] {
            let line = encode_line(&doc, enc).expect("encode");
            let back: serde_json::Value = serde_json::from_str(&line).expect("re-parse");
            assert_eq!(decode_any(back).expect("decode"), doc);
        }
    }
}

thread_local! {
    static SINK: RefCell<crate::pipeline::BulkSink> = RefCell::new(fuzz_sink());
}

fn fuzz_sink() -> crate::pipeline::BulkSink {
    use crate::http::{
        ClientOptions, ClusterInfo, EsClient, Flavor, RetryPolicy, gate::RequestGate,
    };
    let client = EsClient::new(
        &ClientOptions::new(url::Url::parse("http://localhost:9200/").unwrap()),
        RequestGate::default(),
    )
    .expect("client");
    let opts = crate::pipeline::BulkOptions {
        info: ClusterInfo::new(Flavor::OpenSearch, "2.19.0"),
        index: None,
        doc_type: None,
        prefix: String::new(),
        suffix: String::new(),
        rename: None,
        action: crate::pipeline::BulkAction::Index,
        handle_version: true,
        version_type: None,
        meta_fields: vec!["routing".into(), "parent".into()],
        refresh: false,
        ignore_es_write_errors: true,
        ignore_errors: false,
        max_bulk_bytes: 1 << 20,
        retry: RetryPolicy::none(),
    };
    crate::pipeline::BulkSink::new(client, opts, tokio_util::sync::CancellationToken::new())
}

/// Bulk action encoding of one input line: the output is always exactly
/// one action line (plus one source line), each valid JSON (INJ-F1).
/// Mirrors the write path: `encode_doc` only ever sees lines that passed
/// `check_doc_line` (a trailing fragment after the object is rejected there).
pub fn bulk_encode_doc(data: &[u8]) {
    let line = single_line(data.to_vec());
    if crate::pipeline::bulk::check_doc_line(&line).is_err() {
        return;
    }
    let mut out = Vec::new();
    let ok = SINK.with(|s| s.borrow_mut().encode_doc(&line, &mut out).is_ok());
    if ok {
        let lines: Vec<&[u8]> = out
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        assert!((1..=2).contains(&lines.len()), "{} lines", lines.len());
        for l in lines {
            serde_json::from_slice::<serde_json::Value>(l).expect("encoded line is JSON");
        }
    }
}

/// Search response extraction (scroll and PIT shapes).
pub fn extract_batch(data: &[u8]) {
    crate::retrieval::fuzz_extract_batch(data);
}

/// Bulk response accounting.
pub fn bulk_handle_response(data: &[u8]) {
    let docs: Vec<&[u8]> = vec![
        br#"{"_id":"1","_source":{}}"#,
        br#"{"_id":"2","_source":{}}"#,
    ];
    SINK.with(|s| {
        let _ = s.borrow_mut().fuzz_handle_response(data, &docs);
    });
}

/// `--fileSize` values.
pub fn parse_size(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = crate::pipeline::file_sink::parse_size(s);
    }
}

/// Endpoint parsing and redaction.
pub fn endpoint_parse(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data) {
        for dir in [
            crate::endpoint::Direction::Input,
            crate::endpoint::Direction::Output,
        ] {
            let _ = crate::endpoint::parse(s, dir, None);
        }
        let _ = crate::endpoint::redacted_endpoint(s);
    }
}

/// ILM → ISM conversion of an arbitrary policy body.
pub fn ilm_to_ism_convert(data: &[u8]) {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) {
        for lenient in [false, true] {
            let _ = crate::meta::policy::ilm_to_ism::convert("p", &v, lenient);
        }
    }
}

/// Rename rules: `spec\nname\nname...`.
pub fn renamer_new_index(data: &[u8]) {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let mut lines = s.lines();
    let Some(spec) = lines.next() else { return };
    let rules = [spec.to_string()];
    for (p, x) in [(&rules[..], &[][..]), (&[][..], &rules[..])] {
        if let Ok(r) = crate::rename::IndexRenamer::new(p, x, "", "") {
            for n in lines.clone().take(8) {
                let _ = r.index(n);
                let _ = r.alias(n);
                let _ = r.pattern(n);
            }
        }
    }
}

/// `--httpAuthFile` contents.
pub fn ini_auth_file(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = crate::endpoint::parse_auth_ini(s);
    }
}

#[cfg(test)]
mod tests {
    /// Each entry point survives a few awkward inputs (the fuzz smoke runs
    /// cover the rest).
    #[test]
    fn entry_points_never_panic_on_samples() {
        let samples: &[&[u8]] = &[
            b"",
            b"\x00",
            b"{}",
            b"\x05{\"a\":1}\n{\"b\":[1,2]}",
            br#""{\"a\":1}""#,
            br#"{"_index":"i","_id":"1","_source":{"a":1},"_routing":"r","_version":3}"#,
            br#"{"hits":{"total":1,"hits":[{"_id":"1"}]}}"#,
            br#"{"errors":true,"items":[{"index":{"status":429}},{"index":{"status":201}}]}"#,
            b"10mb",
            b"http://u:p@h:9200/i",
            br#"{"policy":{"phases":{"hot":{"actions":{"rollover":{"max_age":"1d"}}}}}}"#,
            b"^(a|b)-(.*):x-$2\na-1\nb-1",
            b"user=u\npassword=p\n",
            b"\xff\xfe",
            // L-505 regression: ASan heap-buffer-overflow in sonic-rs string serialisation
            // (fixed by its `sanitize` feature; only visible in an ASan release build).
            br#"{"_index":"golden_dst","_id":"r4","_source":{"k":"d","n":4}}"#,
            // a trailing fragment after the object: rejected before encoding
            br#"{"_index":"golden_src","_type":"_scoreGdoc 9"}}"#,
        ];
        for s in samples {
            super::json_split(s);
            super::codec_decode(s);
            super::bulk_encode_doc(s);
            super::extract_batch(s);
            super::bulk_handle_response(s);
            super::parse_size(s);
            super::endpoint_parse(s);
            super::ilm_to_ism_convert(s);
            super::renamer_new_index(s);
            super::ini_auth_file(s);
        }
    }
}
