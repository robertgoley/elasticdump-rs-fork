//! L-438 (TST-API-070): cross-version matrix, "export once, import everywhere".
//!
//! Pass 1 (`xmatrix_export_fixture_set`): every image exports the standard fixture set of the
//! Node interop suite (index with analysis, aliases and 20 docs, legacy/component/index
//! templates, ILM or ISM policy, stored script: 11 types) to zstd files under
//! `$IT_XMATRIX_DIR/<label>/` once, plus `meta.json` (names, docs).
//! Pass 2 (`xmatrix_import_every_export`): every image imports every *other* image's export, type
//! by type, each from a clean state (`mapping` and `alias` after the export's `analyzer`, `data`
//! after `analyzer` + `mapping`, `index_template` after its `component_template`). Each cell is classified as
//! `pass`, `pass-with-warnings[w | …]`, `needs-flag[--flag …]` (failed, then passed with the
//! flag the error message named) or `refused[message]`; a restore that "passes" but leaves the
//! object missing or the docs different is `mismatch[…]`.
//!
//! L-439 (TST-API-071): the cells must match the committed table
//! `tests/golden/cross-expectations.toml` (see its header for the matching rules). Every pass-2
//! run also writes the actual cells to `$IT_XMATRIX_DIR/actual-<label>.toml`;
//! `IT_XMATRIX_RECORD=1` reports differences without failing (to review a table change).
//!
//! Without `IT_XMATRIX_DIR` both tests skip. Driver: `scripts/it/xmatrix.sh`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{Value, json};

use crate::harness::*;
use crate::node_interop::{Names, TYPES, cleanup, restore_target, seed};

type Docs = BTreeMap<String, (Option<String>, Value)>;

fn xdir() -> Option<PathBuf> {
    env("IT_XMATRIX_DIR").map(PathBuf::from)
}

/// `opensearch-2.19.7`, `elasticsearch-8.19.23`, `elasticsearch-oss-7.10.2`.
async fn label(c: &Cluster) -> String {
    let engine = match c.engine {
        Engine::OpenSearch => "opensearch",
        Engine::Elasticsearch if c.is_oss().await => "elasticsearch-oss",
        Engine::Elasticsearch => "elasticsearch",
    };
    format!("{engine}-{}", c.version().await)
}

#[tokio::test]
async fn xmatrix_export_fixture_set() {
    let Some(base) = xdir() else {
        println!("SKIP: IT_XMATRIX_DIR not set");
        return;
    };
    let c = Cluster::from_env();
    let me = label(&c).await;
    let dir = base.join(&me);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let es = matches!(c.engine, Engine::Elasticsearch);
    let n = Names::new();
    cleanup(&c, &n, es).await;
    seed(&c, &n, es).await;
    let policies = c.has_policies().await;
    for ty in TYPES {
        if ty == "policy" && !policies {
            continue;
        }
        let f = dir.join(format!("{ty}.json.zst"));
        let mut a = s(&[
            "--input",
            &c.url_with_auth(n.selector(ty)),
            "--output",
            f.to_str().unwrap(),
            "--type",
            ty,
            "--compressionType",
            "zstd",
        ]);
        a.extend(c.tls_args("input"));
        assert_success(&run(a).await);
    }
    let meta = json!({"label": me, "u": n.u, "docs": c.docs(&n.ix).await});
    std::fs::write(dir.join("meta.json"), meta.to_string()).unwrap();
    cleanup(&c, &n, es).await;
    println!("{me}: exported to {}", dir.display());
}

/// Remove every object of the export's names (both policy kinds).
async fn clean_all(c: &Cluster, n: &Names) {
    cleanup(c, n, true).await;
    let _ = c
        .call("DELETE", &format!("_plugins/_ism/policies/{}", n.pol), None)
        .await;
}

fn normalise_msg(line: &str) -> String {
    static RE: std::sync::OnceLock<(Regex, Regex, Regex)> = std::sync::OnceLock::new();
    let (u, url, ts) = RE.get_or_init(|| {
        (
            Regex::new(r"it_[a-z]+_\d+_\d+").unwrap(),
            Regex::new(r"https?://[^\s,;)'\x22]+").unwrap(),
            Regex::new(r"^\[[^\]]*\]\s*").unwrap(),
        )
    });
    let l = ts.replace(line.trim(), "");
    let l = l.strip_prefix("Error:").unwrap_or(&l).trim().to_string();
    let l = u.replace_all(&l, "__U__");
    let l = url.replace_all(&l, "<url>");
    let mut out: String = l.chars().take(140).collect();
    out = out.replace(['[', ']'], "'");
    out
}

fn warnings(stderr: &str) -> Vec<String> {
    let mut w: Vec<String> = stderr
        .lines()
        .filter(|l| l.contains(" WARN ") || l.starts_with("[WARN") || l.contains(" WARN]"))
        .map(normalise_msg)
        .collect();
    w.sort();
    w.dedup();
    w
}

fn error_of(stderr: &str) -> String {
    stderr
        .lines()
        .rev()
        .find(|l| l.contains("Error") || l.contains("ERROR"))
        .or_else(|| stderr.lines().rev().find(|l| !l.trim().is_empty()))
        .map(normalise_msg)
        .unwrap_or_default()
}

/// A flag the error message asks for (and its value, for `--excludeSettings=a,b`).
fn suggested_flag(stderr: &str) -> Option<(String, Option<String>)> {
    let re = Regex::new(
        r"--(excludeSettings|convertIlmToIsm|lenientPolicyConversion)(?:=([A-Za-z0-9_.,*-]+))?",
    )
    .unwrap();
    re.captures(stderr).map(|c| {
        (
            format!("--{}", &c[1]),
            c.get(2).map(|m| m.as_str().to_string()),
        )
    })
}

async fn exists(c: &Cluster, n: &Names, ty: &str) -> bool {
    let path = match ty {
        "alias" => format!("_alias/{}", n.al),
        "template" => format!("_template/{}", n.lt),
        "component_template" => format!("_component_template/{}", n.ct),
        "index_template" => format!("_index_template/{}", n.it),
        "script" => format!("_scripts/{}", n.sc),
        "policy" => match c.engine {
            Engine::OpenSearch => format!("_plugins/_ism/policies/{}", n.pol),
            Engine::Elasticsearch => format!("_ilm/policy/{}", n.pol),
        },
        _ => n.ix.clone(),
    };
    c.call("GET", &path, None).await.0 == 200
}

async fn import(
    c: &Cluster,
    n: &Names,
    file: &Path,
    ty: &str,
    extra: &[String],
) -> std::process::Output {
    let mut a = s(&[
        "--input",
        file.to_str().unwrap(),
        "--output",
        &restore_target(c, n, ty),
        "--type",
        ty,
    ]);
    a.extend(c.tls_args("output"));
    a.extend(extra.iter().cloned());
    run_loud(a).await
}

/// Import one type from a clean state and classify the outcome.
async fn cell(c: &Cluster, n: &Names, dir: &Path, ty: &str, want_docs: &Docs) -> String {
    clean_all(c, n).await;
    // prerequisites that pass on every engine pair: `analyzer` creates the index with its
    // analysis (no engine-specific settings), `mapping` then needs only that
    let pre: &[&str] = match ty {
        "data" => &["analyzer", "mapping"],
        "alias" | "mapping" => &["analyzer"],
        "index_template" => &["component_template"],
        _ => &[],
    };
    for p in pre {
        let _ = import(c, n, &dir.join(format!("{p}.json.zst")), p, &[]).await;
    }
    let file = dir.join(format!("{ty}.json.zst"));
    let mut flags: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    for _ in 0..3 {
        let o = import(c, n, &file, ty, &args).await;
        let err = String::from_utf8_lossy(&o.stderr).to_string();
        if o.status.success() {
            if !exists(c, n, ty).await {
                return "mismatch[object missing after a successful restore]".into();
            }
            if ty == "data" {
                let got = c.docs(&n.ix).await;
                if &got != want_docs {
                    return format!(
                        "mismatch[docs: {} restored, {} expected]",
                        got.len(),
                        want_docs.len()
                    );
                }
            }
            if !flags.is_empty() {
                return format!("needs-flag[{}]", flags.join(" "));
            }
            let w = warnings(&err);
            return if w.is_empty() {
                "pass".into()
            } else {
                format!("pass-with-warnings[{}]", w.join(" | "))
            };
        }
        match suggested_flag(&err) {
            Some((f, v)) if !flags.contains(&f) => {
                args.push(match v {
                    Some(v) => format!("{f}={v}"),
                    None => f.clone(),
                });
                flags.push(f);
            }
            _ => return format!("refused[{}]", error_of(&err)),
        }
    }
    "refused[still failing after three flags]".into()
}

/// `kind[detail]` -> (kind, detail)
fn split_result(r: &str) -> (&str, &str) {
    match r.find('[') {
        Some(i) if r.ends_with(']') => (&r[..i], &r[i + 1..r.len() - 1]),
        _ => (r, ""),
    }
}

/// Matching rules of the table: same kind; `needs-flag` lists the same flags; `refused` and
/// `mismatch`: the actual message contains the expected text; `pass-with-warnings`: the same
/// number of warnings and each expected item is contained in the actual one at its position.
pub(crate) fn matches(expected: &str, actual: &str) -> bool {
    let (ek, ed) = split_result(expected);
    let (ak, ad) = split_result(actual);
    if ek != ak {
        return false;
    }
    match ek {
        "pass" => true,
        "needs-flag" => ed == ad,
        "pass-with-warnings" => {
            let e: Vec<&str> = ed.split(" | ").collect();
            let a: Vec<&str> = ad.split(" | ").collect();
            e.len() == a.len() && e.iter().zip(&a).all(|(e, a)| a.contains(e))
        }
        _ => ad.contains(ed),
    }
}

pub(crate) fn table_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/cross-expectations.toml")
}

/// `"<from> -> <to> : <type>" = "<result>"` lines of the `[cells]` table.
pub(crate) fn load_table(text: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let (k, v) = line
            .split_once("\" = \"")
            .unwrap_or_else(|| panic!("cross-expectations.toml: {line}"));
        let k: String = serde_json::from_str(&format!("{k}\"")).unwrap();
        let v: String = serde_json::from_str(&format!("\"{v}")).unwrap();
        assert!(m.insert(k.clone(), v).is_none(), "duplicate cell {k}");
    }
    m
}

pub(crate) fn cell_key(from: &str, to: &str, ty: &str) -> String {
    format!("{from} -> {to} : {ty}")
}

#[tokio::test]
async fn xmatrix_import_every_export() {
    let Some(base) = xdir() else {
        println!("SKIP: IT_XMATRIX_DIR not set");
        return;
    };
    let c = Cluster::from_env();
    let me = label(&c).await;
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&base)
        .into_iter()
        .flatten()
        .flatten()
        .map(|d| d.path())
        .filter(|p| p.join("meta.json").exists() && !p.ends_with(&me))
        .collect();
    dirs.sort();
    if dirs.is_empty() {
        println!("SKIP: no other export in {}", base.display());
        return;
    }
    let mut actual = BTreeMap::new();
    for dir in &dirs {
        let meta: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).unwrap()).unwrap();
        let from = meta["label"].as_str().unwrap().to_string();
        let n = Names::from_u(meta["u"].as_str().unwrap().to_string());
        let want: Docs = serde_json::from_value(meta["docs"].clone()).unwrap();
        for ty in TYPES {
            if !dir.join(format!("{ty}.json.zst")).exists() {
                continue;
            }
            let r = cell(&c, &n, dir, ty, &want).await;
            println!("{from} -> {me} : {ty} = {r}");
            actual.insert(cell_key(&from, &me, ty), r);
        }
        clean_all(&c, &n).await;
    }
    let mut out = format!("# actual cells for destination {me}\n");
    for (k, v) in &actual {
        out.push_str(&format!("{} = {}\n", json!(k), json!(v)));
    }
    std::fs::write(base.join(format!("actual-{me}.toml")), out).unwrap();

    let table = load_table(&std::fs::read_to_string(table_path()).unwrap_or_default());
    let mut diffs = Vec::new();
    for (k, a) in &actual {
        match table.get(k) {
            None => diffs.push(format!("{k}: not in the table, actual {a}")),
            Some(e) if !matches(e, a) => diffs.push(format!("{k}: expected {e}, actual {a}")),
            _ => {}
        }
    }
    let froms: Vec<String> = actual
        .keys()
        .map(|k| k.split(" -> ").next().unwrap().to_string())
        .collect();
    for k in table.keys() {
        let (from, rest) = k.split_once(" -> ").unwrap();
        if rest.starts_with(&format!("{me} : "))
            && froms.iter().any(|f| f == from)
            && !actual.contains_key(k)
        {
            diffs.push(format!("{k}: in the table but not produced"));
        }
    }
    if diffs.is_empty() {
        println!("{me}: {} cells match the expectation table", actual.len());
    } else if env("IT_XMATRIX_RECORD").as_deref() == Some("1") {
        println!("RECORD: {} differences:\n{}", diffs.len(), diffs.join("\n"));
    } else {
        panic!(
            "{} cells differ from tests/golden/cross-expectations.toml (a table change needs a review):\n{}",
            diffs.len(),
            diffs.join("\n")
        );
    }
}

// L-439 (TST-API-071): the committed table parses, covers the 9 x 8 pairs, and the matching
// rules reject a changed cell (no cluster needed).
#[test]
fn expectation_table_is_complete_and_strict() {
    let t = load_table(&std::fs::read_to_string(table_path()).unwrap());
    let pairs: std::collections::BTreeSet<&str> =
        t.keys().map(|k| k.split(" : ").next().unwrap()).collect();
    assert_eq!(pairs.len(), 72, "9 images x 8 destinations");
    assert_eq!(
        t.len(),
        72 * 11 - 8,
        "every type, no policy from the ES OSS build"
    );
    for v in t.values() {
        let (kind, _) = split_result(v);
        assert!(
            [
                "pass",
                "pass-with-warnings",
                "needs-flag",
                "refused",
                "mismatch"
            ]
            .contains(&kind),
            "{v}"
        );
    }
    assert!(matches("pass", "pass"));
    assert!(!matches("pass", "pass-with-warnings[x]"));
    assert!(matches(
        "refused[ISM → ILM]",
        "refused[these are ISM → ILM policies]"
    ));
    assert!(!matches("refused[ISM]", "refused[ILM]"));
    assert!(!matches(
        "needs-flag[--excludeSettings]",
        "needs-flag[--convertIlmToIsm]"
    ));
    assert!(matches(
        "pass-with-warnings[a | b]",
        "pass-with-warnings[xa | bx]"
    ));
    assert!(!matches(
        "pass-with-warnings[a]",
        "pass-with-warnings[a | b]"
    ));
    assert!(!matches(
        "pass-with-warnings[a | b]",
        "pass-with-warnings[b | a]"
    ));
}

// L-440 (TST-API-072): live cluster-to-cluster pairs (the `cross` profile with chosen images,
// scripts/it/xpairs.sh). Every type is copied in both directions straight from the source
// cluster; data must arrive digest-equal and each metadata outcome must agree with the file
// cell of the same pair in the expectation table, translated for a live source:
//   pass, pass-with-warnings          -> succeeds
//   needs-flag[--excludeSettings]     -> succeeds without the flag (the source engine is known,
//                                        so its own keys are dropped with a "Dropping" warning)
//   needs-flag[other flags]           -> fails without, succeeds with those flags
//   refused[…]                        -> fails
#[tokio::test]
async fn cluster_to_cluster_pair_every_type() {
    let Some(peer) = Cluster::peer() else {
        println!("SKIP: no peer cluster (cross profile only)");
        return;
    };
    let table = load_table(&std::fs::read_to_string(table_path()).unwrap());
    let a = Cluster::from_env();
    for (src, dst) in [(a.clone(), peer.clone()), (peer, a)] {
        let (from, to) = (label(&src).await, label(&dst).await);
        let es = matches!(src.engine, Engine::Elasticsearch);
        let n = Names::new();
        clean_all(&src, &n).await;
        seed(&src, &n, es).await;
        let want = src.docs(&n.ix).await;
        let policies = src.has_policies().await;
        for ty in TYPES {
            if ty == "policy" && !policies {
                continue;
            }
            let key = cell_key(&from, &to, ty);
            let cellv = table
                .get(&key)
                .unwrap_or_else(|| panic!("{key}: no file cell in the table"));
            let (kind, detail) = split_result(cellv);
            let flags: Vec<String> = match kind {
                "needs-flag" if detail != "--excludeSettings" => {
                    detail.split(' ').map(String::from).collect()
                }
                _ => vec![],
            };
            let copy = |extra: Vec<String>| {
                let (src, dst, n) = (&src, &dst, &n);
                async move {
                    clean_all(dst, n).await;
                    let pre: &[&str] = match ty {
                        "data" => &["analyzer", "mapping"],
                        "alias" | "mapping" => &["analyzer"],
                        "index_template" => &["component_template"],
                        _ => &[],
                    };
                    let one = |t: &'static str, extra: Vec<String>| {
                        let mut a = s(&[
                            "--input",
                            &src.url_with_auth(n.selector(t)),
                            "--output",
                            &restore_target(dst, n, t),
                            "--type",
                            t,
                        ]);
                        a.extend(extra);
                        run_loud(a)
                    };
                    for p in pre {
                        let _ = one(p, vec![]).await;
                    }
                    one(ty, extra).await
                }
            };
            let o = copy(vec![]).await;
            let err = String::from_utf8_lossy(&o.stderr).to_string();
            match kind {
                "refused" => assert!(!o.status.success(), "{key}: expected a refusal; {err}"),
                "needs-flag" if !flags.is_empty() => {
                    assert!(
                        !o.status.success(),
                        "{key}: expected to need {flags:?}; {err}"
                    );
                    let o = copy(flags.clone()).await;
                    assert_success(&o);
                }
                _ => {
                    assert!(o.status.success(), "{key} ({cellv}): {err}");
                    if detail == "--excludeSettings" {
                        assert!(err.contains("Dropping"), "{key}: {err}");
                    }
                }
            }
            if o.status.success() || !flags.is_empty() {
                assert!(exists(&dst, &n, ty).await, "{key}: object missing");
                if ty == "data" {
                    assert_eq!(dst.docs(&n.ix).await, want, "{key}: docs");
                }
            }
            println!("{key}: {cellv} -> live copy agrees");
        }
        clean_all(&dst, &n).await;
        clean_all(&src, &n).await;
    }
}
