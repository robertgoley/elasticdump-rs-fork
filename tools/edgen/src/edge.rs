//! Edge-case catalogue (TEST-REQUIREMENTS §5.4, TST-DATA-003). Each document
//! carries `case` so a failure names the case. Values whose exact bytes
//! matter (numbers, deep structures) live under `raw`, an `enabled: false`
//! object, so the engine stores them without mapping them.

use crate::json::J;
use crate::profiles::Meta;

/// Expected outcome when loaded into a cluster with the `edge` mapping.
/// `verify` = differs by engine/version; recorded per version by A4 (DATA-003).
pub struct Case {
    pub name: &'static str,
    pub expect: &'static str,
    /// Too big or too deep for a cluster load; file-level tests only.
    pub file_only: bool,
    /// Writes into a different index (opt-in: creates extra indices).
    pub index_case: bool,
}

macro_rules! c {
    ($n:expr, $e:expr) => {
        Case {
            name: $n,
            expect: $e,
            file_only: false,
            index_case: false,
        }
    };
    ($n:expr, $e:expr, file) => {
        Case {
            name: $n,
            expect: $e,
            file_only: true,
            index_case: false,
        }
    };
    ($n:expr, $e:expr, index) => {
        Case {
            name: $n,
            expect: $e,
            file_only: false,
            index_case: true,
        }
    };
}

pub const CASES: &[Case] = &[
    c!("unicode_planes", "store"),
    c!("unicode_zwj_rtl_combining", "store"),
    c!("nul_escape", "store"),
    c!("lone_surrogate", "verify"),
    c!("line_separators", "store"),
    c!("all_escapes", "store"),
    c!("only_backslashes", "store"),
    c!("only_quotes", "store"),
    c!("int_gt_2_53", "store"),
    c!("int_gt_2_64", "store"),
    c!("negative_zero", "store"),
    c!("float_1e308", "store"),
    c!("float_subnormal", "store"),
    c!("one_point_zero", "store"),
    c!("long_mantissa", "store"),
    c!("deep_20", "store"),
    c!("deep_100", "verify"),
    c!("deep_1000", "reject", file),
    c!("deep_10000", "reject", file),
    c!("empty_containers", "store"),
    c!("mixed_array_disabled", "store"),
    c!("array_10k", "store"),
    c!("doc_1mb", "store"),
    c!("doc_10mb", "store", file),
    c!("doc_90mb", "verify", file),
    c!("string_64kb_keyword", "store"),
    c!("id_specials", "store"),
    c!("id_space", "store"),
    c!("id_unicode", "store"),
    c!("id_leading_underscore", "store"),
    c!("id_512_bytes", "store"),
    c!("id_513_bytes", "reject"),
    c!("id_duplicate", "store_last_wins"),
    c!("routing_custom", "store"),
    c!("meta_named_fields", "store"),
    c!("meta_named_fields_raw", "store"),
    c!("date_nanos", "store"),
    c!("date_epoch_millis_string", "store"),
    c!("date_custom_format", "store"),
    c!("index_unicode", "store", index),
    c!("index_hash_plus_minus", "store", index),
    c!("index_hidden_dot", "verify", index),
    c!("index_date_math", "verify", index),
    c!("index_255_bytes", "store", index),
    c!("index_leading_minus", "reject", index),
];

/// Cases active for a run.
pub fn active(file_only: bool, index_cases: bool) -> Vec<&'static Case> {
    CASES
        .iter()
        .filter(|c| (file_only || !c.file_only) && (index_cases || !c.index_case))
        .collect()
}

fn deep(j: &mut J, depth: usize, arrays: bool) {
    for d in 0..depth {
        if arrays && d % 2 == 1 {
            j.arr();
        } else {
            j.obj().key("d");
        }
    }
    j.u(depth as u64);
    for d in (0..depth).rev() {
        if arrays && d % 2 == 1 {
            j.end_arr();
        } else {
            j.end_obj();
        }
    }
}

fn big_string(n: usize) -> String {
    (0..n).map(|k| (b'a' + (k % 26) as u8) as char).collect()
}

/// Write case `c` for document `i`; returns metadata overrides.
pub fn write(c: &Case, i: u64, j: &mut J) -> Meta {
    let mut m = Meta::default();
    j.obj()
        .kstr("case", c.name)
        .kstr("expect", c.expect)
        .ku("n", i);
    match c.name {
        "unicode_planes" => {
            j.kstr(
                "text",
                "BMP é ß Ж 中文 한국어 | SMP 😀 🦀 𝄞 𐍈 | PUA \u{E000} | max \u{10FFFD}",
            );
        }
        "unicode_zwj_rtl_combining" => {
            j.kstr(
                "text",
                "👨\u{200D}👩\u{200D}👧 שלום مرحبا e\u{0301} a\u{0308}\u{0323}",
            );
        }
        "nul_escape" => {
            j.kstr("text", "a\u{0000}b");
        }
        "lone_surrogate" => {
            j.key("text").raw("\"x\\ud800y\"");
        }
        "line_separators" => {
            j.kstr("text", "a\u{2028}b\u{2029}c");
        }
        "all_escapes" => {
            j.key("text")
                .raw("\"\\\" \\\\ \\/ \\b \\f \\n \\r \\t \\u0041\"");
        }
        "only_backslashes" => {
            j.kstr("text", "\\\\\\\\\\\\\\\\");
        }
        "only_quotes" => {
            j.kstr("text", "\"\"\"\"\"\"");
        }
        "int_gt_2_53" => {
            j.key("raw")
                .obj()
                .key("v")
                .raw("9007199254740993")
                .end_obj();
        }
        "int_gt_2_64" => {
            j.key("raw")
                .obj()
                .key("v")
                .raw("18446744073709551617")
                .end_obj();
        }
        "negative_zero" => {
            j.key("raw").obj().key("v").raw("-0.0").end_obj();
        }
        "float_1e308" => {
            j.key("raw").obj().key("v").raw("1e308").end_obj();
        }
        "float_subnormal" => {
            j.key("raw").obj().key("v").raw("1e-320").end_obj();
        }
        "one_point_zero" => {
            j.key("raw")
                .obj()
                .key("a")
                .raw("1.0")
                .key("b")
                .raw("1")
                .end_obj();
        }
        "long_mantissa" => {
            j.key("raw")
                .obj()
                .key("v")
                .raw("0.1000000000000000055511151231257827021181583404541015625")
                .end_obj();
        }
        "deep_20" | "deep_100" | "deep_1000" | "deep_10000" => {
            let d: usize = c.name[5..].parse().unwrap();
            j.key("raw");
            deep(j, d, true);
        }
        "empty_containers" => {
            j.key("raw")
                .obj()
                .key("o")
                .obj()
                .end_obj()
                .key("a")
                .arr()
                .end_arr()
                .kstr("s", "")
                .end_obj();
        }
        "mixed_array_disabled" => {
            j.key("raw")
                .obj()
                .key("v")
                .arr()
                .u(1)
                .str("two")
                .bool(true)
                .null()
                .obj()
                .ku("x", 1)
                .end_obj()
                .arr()
                .u(2)
                .end_arr()
                .fixed(35, 1)
                .end_arr()
                .end_obj();
        }
        "array_10k" => {
            j.key("raw").obj().key("v").arr();
            for k in 0..10_000u64 {
                j.u(k);
            }
            j.end_arr().end_obj();
        }
        "doc_1mb" | "doc_10mb" | "doc_90mb" => {
            let mb: usize = c.name[4..c.name.len() - 2].parse().unwrap();
            j.key("raw").obj().key("parts").arr();
            let s = big_string(65_536);
            for _ in 0..(mb << 20) / 65_536 {
                j.str(&s);
            }
            j.end_arr().end_obj();
        }
        "string_64kb_keyword" => {
            j.kstr("kw", &big_string(65_536));
        }
        "id_specials" => m.id = Some(format!("a/b?c#d%e&f+g:h*i\"j\\k<l>m|n-{i}")),
        "id_space" => m.id = Some(format!("with space {i}")),
        "id_unicode" => m.id = Some(format!("ид-😀-中-{i}")),
        "id_leading_underscore" => m.id = Some(format!("_lead-{i}")),
        "id_512_bytes" | "id_513_bytes" => {
            let n: usize = c.name[3..6].parse().unwrap();
            let p = format!("{i}-");
            m.id = Some(p.clone() + &"x".repeat(n - p.len()));
        }
        "id_duplicate" => m.id = Some("edge-duplicate-id".into()),
        "routing_custom" => m.routing = Some(format!("custom-{}", i % 3)),
        "meta_named_fields" => {
            j.kstr("hits", "h")
                .kstr("sort", "s")
                .kstr("pit_id", "p")
                .kstr("_scroll_id", "x");
        }
        "meta_named_fields_raw" => {
            j.key("raw")
                .obj()
                .kstr("_id", "fake")
                .key("_source")
                .obj()
                .ku("a", 1)
                .end_obj()
                .kstr("_index", "other")
                .end_obj();
        }
        "date_nanos" => {
            j.kstr("ts_nanos", "2026-10-29T12:00:00.123456789Z");
        }
        "date_epoch_millis_string" => {
            j.kstr("ts_millis", "1761739200123");
        }
        "date_custom_format" => {
            j.kstr("ts_custom", "29/10/2026 12:00");
        }
        "index_unicode" => m.index = Some("edge-ünïcödé-索引".into()),
        "index_hash_plus_minus" => m.index = Some("edge#a+b-c".into()),
        "index_hidden_dot" => m.index = Some(".edge-hidden".into()),
        "index_date_math" => m.index = Some("<edge-{now/d}>".into()),
        "index_255_bytes" => m.index = Some("edge-".to_string() + &"x".repeat(250)),
        "index_leading_minus" => m.index = Some("-edge".into()),
        other => unreachable!("case {other}"),
    }
    j.end_obj();
    m
}
