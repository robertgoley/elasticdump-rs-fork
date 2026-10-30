//! M7-M13: the metadata types `index`, `analyzer`, `alias`, `template`,
//! `component_template`, `index_template`, `policy` and `script`, plus the
//! index rename rules, on every profile. Cluster-global resources use
//! unique names and are cleaned up by each test.

use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};

use crate::harness::*;

fn tls(c: &Cluster, a: &mut Vec<String>, input: &str, output: &str) {
    if input.starts_with("http") {
        a.extend(c.tls_args("input"));
    }
    if output.starts_with("http") {
        a.extend(c.tls_args("output"));
    }
}

/// Run elasticdump-rs (quiet) for `--type ty`.
async fn rs(c: &Cluster, input: &str, output: &str, ty: &str, extra: &[&str]) -> Output {
    let mut a = s(&["--input", input, "--output", output, "--type", ty]);
    tls(c, &mut a, input, output);
    a.extend(s(extra));
    run(a).await
}

/// Same, keeping warnings on stderr.
async fn rs_loud(c: &Cluster, input: &str, output: &str, ty: &str, extra: &[&str]) -> Output {
    let mut a = s(&["--input", input, "--output", output, "--type", ty]);
    tls(c, &mut a, input, output);
    a.extend(s(extra));
    run_loud(a).await
}

async fn dump(c: &Cluster, input_path: &str, ty: &str, extra: &[&str]) -> PathBuf {
    let f = tmp_path(ty, "json");
    let o = rs(
        c,
        &c.url_with_auth(input_path),
        f.to_str().unwrap(),
        ty,
        extra,
    )
    .await;
    assert_success(&o);
    f
}

/// Decode a metadata file (string literal line).
fn read_doc(p: &Path) -> Value {
    let l = &lines(p)[0];
    let v: Value = serde_json::from_str(l).unwrap();
    match v {
        Value::String(s) => serde_json::from_str(&s).unwrap(),
        o => o,
    }
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

fn host(c: &Cluster) -> String {
    c.url_with_auth("").trim_end_matches('/').to_string()
}

fn node() -> Option<&'static str> {
    let p = "/tmp/ed/node_modules/.bin/elasticdump";
    (Path::new(p).exists() && Path::new("/usr/bin/node").exists()).then_some(p)
}

async fn run_node(args: &[&str]) -> Output {
    let mut cmd = tokio::process::Command::new(node().unwrap());
    cmd.args(args).env("NODE_TLS_REJECT_UNAUTHORIZED", "0");
    cmd.output().await.unwrap()
}

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/node/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// An index with a custom analyzer, a mapping and two aliases.
async fn seed_meta_index(c: &Cluster, index: &str, aliases: Value) {
    c.delete_index(index).await;
    c.ok(
        "PUT",
        index,
        Some(json!({
            "settings": {"number_of_shards": 1, "number_of_replicas": 0,
                "analysis": {"analyzer": {"my_an": {"type": "custom", "tokenizer": "whitespace", "filter": ["lowercase"]}}}},
            "mappings": {"properties": {"k": {"type": "keyword"}, "t": {"type": "text", "analyzer": "my_an"}}},
            "aliases": aliases
        })),
    )
    .await;
    c.bulk_raw(format!(
        "{{\"index\":{{\"_index\":\"{index}\",\"_id\":\"1\"}}}}\n{{\"k\":\"x\",\"t\":\"Hello World\"}}\n{{\"index\":{{\"_index\":\"{index}\",\"_id\":\"2\"}}}}\n{{\"k\":\"y\",\"t\":\"Other\"}}\n"
    ))
    .await;
    c.ok("POST", &format!("{index}/_refresh"), None).await;
}

/// `GET /{index}` minus generated settings, keyed by `key` instead of the
/// real name.
async fn index_shape(c: &Cluster, index: &str) -> Value {
    let v = c.ok("GET", index, None).await;
    let mut d = v[index].clone();
    let s = d["settings"]["index"].as_object_mut().unwrap();
    for k in [
        "uuid",
        "creation_date",
        "provided_name",
        "version",
        "history",
        "routing",
        "replication",
    ] {
        s.remove(k);
    }
    d
}

// ------------------------------------------------------------------ index

#[tokio::test]
async fn index_round_trip_wildcard_dump_and_node_interop() {
    let c = Cluster::from_env();
    let src = unique("ix");
    let al = format!("{src}-al");
    seed_meta_index(
        &c,
        &src,
        json!({ al.as_str(): {"filter": {"term": {"k": "x"}}, "routing": "1"} }),
    )
    .await;
    let before = index_shape(&c, &src).await;
    let f = dump(&c, &src, "index", &[]).await;
    let doc = read_doc(&f);
    assert!(
        doc[&src]["settings"]["index"].get("uuid").is_none(),
        "{doc}"
    );
    c.delete_index(&src).await;
    assert_success(&rs(&c, f.to_str().unwrap(), &host(&c), "index", &[]).await);
    assert_eq!(index_shape(&c, &src).await, before);

    // existing target: error, or skipped with --skip-existing
    let o = rs(&c, f.to_str().unwrap(), &host(&c), "index", &[]).await;
    assert!(!o.status.success());
    assert_success(
        &rs(
            &c,
            f.to_str().unwrap(),
            &host(&c),
            "index",
            &["--skip-existing"],
        )
        .await,
    );

    // wildcard dump: generated keys stripped on every index (elasticdump bug 2)
    let pat = format!("{}*", &src[..src.len() - 1]);
    let w = dump(&c, &pat, "index", &[]).await;
    for (_, def) in read_doc(&w).as_object().unwrap() {
        let s = &def["settings"]["index"];
        assert!(
            s.get("uuid").is_none() && s.get("creation_date").is_none(),
            "{s}"
        );
    }

    if node().is_some() {
        // Node dump -> rs restore
        let nf = tmp_path("node_index", "json");
        let o = run_node(&[
            "--input",
            &c.url_with_auth(&src),
            "--output",
            nf.to_str().unwrap(),
            "--type",
            "index",
        ])
        .await;
        assert!(o.status.success(), "{}", err(&o));
        c.delete_index(&src).await;
        assert_success(&rs(&c, nf.to_str().unwrap(), &host(&c), "index", &[]).await);
        assert_eq!(index_shape(&c, &src).await, before);
        // rs dump -> Node restore (Node PUTs the output URL)
        c.delete_index(&src).await;
        let o = run_node(&[
            "--input",
            f.to_str().unwrap(),
            "--output",
            &c.url_with_auth(&src),
            "--type",
            "index",
        ])
        .await;
        assert!(o.status.success(), "{}", err(&o));
        assert_eq!(index_shape(&c, &src).await, before);
    }
    c.delete_index(&src).await;
}

// ------------------------------------------------------------------ alias

async fn aliases_of(c: &Cluster, index: &str) -> Value {
    c.ok("GET", &format!("{index}/_alias"), None).await[index]["aliases"].clone()
}

#[tokio::test]
async fn alias_round_trip_filters_prefix_and_node_forms() {
    let c = Cluster::from_env();
    let src = unique("al");
    let a1 = format!("{src}-a1");
    let a2 = format!("{src}-a2");
    seed_meta_index(
        &c,
        &src,
        json!({ a1.as_str(): {}, a2.as_str(): {"filter": {"term": {"k": "x"}}, "index_routing": "1", "search_routing": "1,2", "is_write_index": true} }),
    )
    .await;
    let before = aliases_of(&c, &src).await;
    let f = dump(&c, &src, "alias", &[]).await;
    c.ok(
        "POST",
        "_aliases",
        Some(json!({"actions": [{"remove": {"index": src, "alias": "*"}}]})),
    )
    .await;
    assert_eq!(aliases_of(&c, &src).await, json!({}));
    assert_success(&rs(&c, f.to_str().unwrap(), &host(&c), "alias", &[]).await);
    assert_eq!(aliases_of(&c, &src).await, before);

    // alias-name filter via --input-index index/alias
    let only = tmp_path("alias_a1", "json");
    let spec = format!("{src}/{a1}");
    assert_success(
        &rs(
            &c,
            &host(&c),
            only.to_str().unwrap(),
            "alias",
            &["--input-index", &spec],
        )
        .await,
    );
    assert_eq!(
        read_doc(&only)[&src]["aliases"].as_object().unwrap().len(),
        1
    );
    // elasticdump's two-segment URL form, with a warning
    let two = tmp_path("alias_two", "json");
    let o = rs_loud(
        &c,
        &c.url_with_auth(&format!("{src}/{a1}")),
        two.to_str().unwrap(),
        "alias",
        &[],
    )
    .await;
    assert_success(&o);
    assert!(err(&o).contains("<index>/<alias>"), "{}", err(&o));
    assert_eq!(read_doc(&two), read_doc(&only));

    // --prefix applies to the indices the aliases point at
    let p = format!("p_{src}");
    seed_meta_index(&c, &p, json!({})).await;
    c.ok(
        "POST",
        "_aliases",
        Some(json!({"actions": [{"remove": {"index": src, "alias": "*"}}]})),
    )
    .await;
    assert_success(
        &rs(
            &c,
            f.to_str().unwrap(),
            &host(&c),
            "alias",
            &["--prefix", "p_"],
        )
        .await,
    );
    assert_eq!(aliases_of(&c, &p).await, before);

    // elasticdump's fixture (index source_index)
    c.delete_index("source_index").await;
    c.ok(
        "PUT",
        "source_index",
        Some(json!({"settings": {"number_of_replicas": 0}})),
    )
    .await;
    assert_success(&rs(&c, &fixture("alias.json"), &host(&c), "alias", &[]).await);
    let a = aliases_of(&c, "source_index").await;
    assert!(
        a.get("the-source").is_some() && a.get("test-2018").is_some(),
        "{a}"
    );
    c.delete_index("source_index").await;

    if node().is_some() {
        // rs dump -> Node restore
        c.ok(
            "POST",
            "_aliases",
            Some(json!({"actions": [{"remove": {"index": p, "alias": "*"}}]})),
        )
        .await;
        let o = run_node(&[
            "--input",
            f.to_str().unwrap(),
            "--output",
            &host(&c),
            "--type",
            "alias",
        ])
        .await;
        assert!(o.status.success(), "{}", err(&o));
        assert_eq!(aliases_of(&c, &src).await, before);
        // Node dump -> rs restore
        let nf = tmp_path("node_alias", "json");
        let o = run_node(&[
            "--input",
            &c.url_with_auth(&src),
            "--output",
            nf.to_str().unwrap(),
            "--type",
            "alias",
        ])
        .await;
        assert!(o.status.success(), "{}", err(&o));
        c.ok(
            "POST",
            "_aliases",
            Some(json!({"actions": [{"remove": {"index": src, "alias": "*"}}]})),
        )
        .await;
        assert_success(&rs(&c, nf.to_str().unwrap(), &host(&c), "alias", &[]).await);
        assert_eq!(aliases_of(&c, &src).await, before);
    }
    c.delete_index(&src).await;
    c.delete_index(&p).await;
}

// --------------------------------------------------------------- analyzer

async fn analyze(c: &Cluster, index: &str) -> (u16, Value) {
    c.call(
        "POST",
        &format!("{index}/_analyze"),
        Some(json!({"analyzer": "my_an", "text": "Hello World"})),
    )
    .await
}

async fn index_open(c: &Cluster, index: &str) -> bool {
    let v = c
        .ok(
            "GET",
            &format!("_cat/indices/{index}?format=json&expand_wildcards=all"),
            None,
        )
        .await;
    v[0]["status"] == "open"
}

#[tokio::test]
async fn analyzer_create_close_index_and_failure_reopens() {
    let c = Cluster::from_env();
    let src = unique("an");
    seed_meta_index(&c, &src, json!({})).await;
    let f = dump(&c, &src, "analyzer", &[]).await;

    // new index: created with the analysis
    let fresh = unique("an_new");
    c.delete_index(&fresh).await;
    assert_success(
        &rs(
            &c,
            f.to_str().unwrap(),
            &c.url_with_auth(&fresh),
            "analyzer",
            &[],
        )
        .await,
    );
    let (st, v) = analyze(&c, &fresh).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["tokens"][0]["token"], "hello");

    // existing index with docs: refused, skipped, or closed/updated/reopened
    let live = unique("an_live");
    c.delete_index(&live).await;
    c.ok(
        "PUT",
        &live,
        Some(json!({"settings": {"number_of_replicas": 0}})),
    )
    .await;
    c.bulk_raw(format!(
        "{{\"index\":{{\"_index\":\"{live}\",\"_id\":\"1\"}}}}\n{{\"a\":1}}\n"
    ))
    .await;
    let o = rs(
        &c,
        f.to_str().unwrap(),
        &c.url_with_auth(&live),
        "analyzer",
        &[],
    )
    .await;
    assert!(
        !o.status.success() && err(&o).contains("--closeIndex"),
        "{}",
        err(&o)
    );
    assert_success(
        &rs(
            &c,
            f.to_str().unwrap(),
            &c.url_with_auth(&live),
            "analyzer",
            &["--skip-existing"],
        )
        .await,
    );
    assert_eq!(analyze(&c, &live).await.0, 400);
    let o = rs_loud(
        &c,
        f.to_str().unwrap(),
        &c.url_with_auth(&live),
        "analyzer",
        &["--closeIndex"],
    )
    .await;
    assert_success(&o);
    assert!(err(&o).contains("OFFLINE"), "{}", err(&o));
    assert!(index_open(&c, &live).await);
    assert_eq!(analyze(&c, &live).await.0, 200);
    assert_eq!(c.count(&live).await, 1);

    // an invalid analyzer: exit 1 and the index is open again
    let bad = tmp_path("bad_an", "json");
    let doc = json!({"x": {"settings": {"index": {"analysis": {"analyzer": {"zz": {"type": "custom", "tokenizer": "no_such_tokenizer"}}}}}}});
    std::fs::write(
        &bad,
        serde_json::to_string(&Value::String(doc.to_string())).unwrap(),
    )
    .unwrap();
    let o = rs(
        &c,
        bad.to_str().unwrap(),
        &c.url_with_auth(&live),
        "analyzer",
        &["--closeIndex"],
    )
    .await;
    assert!(!o.status.success());
    assert!(err(&o).contains("reopened"), "{}", err(&o));
    assert!(index_open(&c, &live).await);

    if node().is_some() {
        // Node dump -> rs restore
        let nf = tmp_path("node_an", "json");
        let o = run_node(&[
            "--input",
            &c.url_with_auth(&src),
            "--output",
            nf.to_str().unwrap(),
            "--type",
            "analyzer",
        ])
        .await;
        assert!(o.status.success(), "{}", err(&o));
        let t = unique("an_node");
        assert_success(
            &rs(
                &c,
                nf.to_str().unwrap(),
                &c.url_with_auth(&t),
                "analyzer",
                &[],
            )
            .await,
        );
        assert_eq!(analyze(&c, &t).await.0, 200);
        // rs dump -> Node restore (Node waits for green; replicas 0 here)
        let t2 = unique("an_node2");
        let o = run_node(&[
            "--input",
            f.to_str().unwrap(),
            "--output",
            &c.url_with_auth(&t2),
            "--type",
            "analyzer",
        ])
        .await;
        assert!(o.status.success(), "{}", err(&o));
        assert_eq!(analyze(&c, &t2).await.0, 200);
        c.delete_index(&t).await;
        c.delete_index(&t2).await;
    }
    for i in [&src, &fresh, &live] {
        c.delete_index(i).await;
    }
}

// -------------------------------------------------------------- templates

async fn get_or_null(c: &Cluster, path: &str) -> Value {
    let (st, v) = c.call("GET", path, None).await;
    if st == 404 { Value::Null } else { v }
}

#[tokio::test]
async fn templates_round_trip_order_and_node_fixtures() {
    let c = Cluster::from_env();
    let u = unique("tp");
    let comp = format!("{u}-c");
    let legacy = json!({"index_patterns": [format!("{u}-legacy-*")], "order": 3,
        "settings": {"index": {"number_of_shards": "1"}},
        "mappings": {"properties": {"a": {"type": "keyword"}}}, "aliases": {format!("{u}-la"): {}}});
    let component =
        json!({"template": {"mappings": {"properties": {"ts": {"type": "date"}}}}, "version": 2});
    let composable = json!({"index_patterns": [format!("{u}-ix-*")], "priority": 77, "composed_of": [comp],
        "template": {"settings": {"index": {"number_of_shards": "1"}}}, "_meta": {"owner": "it"}});
    c.ok("PUT", &format!("_template/{u}"), Some(legacy)).await;
    c.ok(
        "PUT",
        &format!("_component_template/{comp}"),
        Some(component),
    )
    .await;
    c.ok("PUT", &format!("_index_template/{u}"), Some(composable))
        .await;
    let g_legacy = c.ok("GET", &format!("_template/{u}"), None).await;
    let g_comp = c
        .ok("GET", &format!("_component_template/{comp}"), None)
        .await;
    let g_ix = c.ok("GET", &format!("_index_template/{u}"), None).await;

    let f_legacy = dump(&c, &u, "template", &[]).await;
    let f_comp = dump(&c, &format!("{u}*"), "component_template", &[]).await;
    let f_ix = dump(&c, &u, "index_template", &[]).await;
    assert_eq!(
        read_doc(&f_comp)["component_templates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    c.ok("DELETE", &format!("_index_template/{u}"), None).await;
    c.ok("DELETE", &format!("_component_template/{comp}"), None)
        .await;
    c.ok("DELETE", &format!("_template/{u}"), None).await;

    // index template before its component template: clear error
    let o = rs(&c, f_ix.to_str().unwrap(), &host(&c), "index_template", &[]).await;
    assert!(
        !o.status.success() && err(&o).contains("restore component templates first"),
        "{}",
        err(&o)
    );
    // wrong --type for the file
    let o = rs(&c, f_ix.to_str().unwrap(), &host(&c), "template", &[]).await;
    assert!(err(&o).contains("use --type index_template"), "{}", err(&o));

    for (f, ty) in [
        (&f_comp, "component_template"),
        (&f_ix, "index_template"),
        (&f_legacy, "template"),
    ] {
        assert_success(&rs(&c, f.to_str().unwrap(), &host(&c), ty, &[]).await);
    }
    assert_eq!(c.ok("GET", &format!("_template/{u}"), None).await, g_legacy);
    assert_eq!(
        c.ok("GET", &format!("_component_template/{comp}"), None)
            .await,
        g_comp
    );
    assert_eq!(
        c.ok("GET", &format!("_index_template/{u}"), None).await,
        g_ix
    );
    // --skip-existing leaves them alone
    assert_success(
        &rs(
            &c,
            f_legacy.to_str().unwrap(),
            &host(&c),
            "template",
            &["--skip-existing"],
        )
        .await,
    );

    // elasticdump fixtures (6.x typed mapping unwrapped on 7+/OpenSearch)
    for (fx, ty) in [
        ("template_6x.json", "template"),
        ("template_7x_component_template.json", "component_template"),
        ("template_7x_index_template.json", "index_template"),
    ] {
        assert_success(&rs(&c, &fixture(fx), &host(&c), ty, &[]).await);
    }
    let t1 = c.ok("GET", "_template/template_1", None).await;
    assert_eq!(
        t1["template_1"]["mappings"]["_source"]["enabled"], false,
        "{t1}"
    );
    let it1 = c.ok("GET", "_index_template/template_1", None).await;
    assert_eq!(
        it1["index_templates"][0]["index_template"]["priority"], 200,
        "{it1}"
    );

    if node().is_some() {
        // rs dump -> Node restore, Node dump -> rs dump equal after decode
        c.ok("DELETE", &format!("_template/{u}"), None).await;
        let o = run_node(&[
            "--input",
            f_legacy.to_str().unwrap(),
            "--output",
            &host(&c),
            "--type",
            "template",
        ])
        .await;
        assert!(o.status.success(), "{}", err(&o));
        assert_eq!(c.ok("GET", &format!("_template/{u}"), None).await, g_legacy);
        for ty in ["template", "index_template", "component_template"] {
            let nf = tmp_path("node_tpl", "json");
            let o = run_node(&[
                "--input",
                &c.url_with_auth(&format!("{u}*")),
                "--output",
                nf.to_str().unwrap(),
                "--type",
                ty,
            ])
            .await;
            assert!(o.status.success(), "{}", err(&o));
            let rf = dump(&c, &format!("{u}*"), ty, &[]).await;
            assert_eq!(read_doc(&nf), read_doc(&rf), "{ty}");
        }
    }
    c.call("DELETE", &format!("_index_template/{u}"), None)
        .await;
    c.call("DELETE", "_index_template/template_1", None).await;
    c.call("DELETE", &format!("_component_template/{comp}"), None)
        .await;
    c.call("DELETE", "_component_template/component_template1", None)
        .await;
    c.call(
        "DELETE",
        "_component_template/other_component_template",
        None,
    )
    .await;
    c.call("DELETE", &format!("_template/{u}"), None).await;
    c.call("DELETE", "_template/template_1", None).await;
    let _ = get_or_null(&c, "_template/none").await;
}

#[tokio::test]
async fn managed_templates_and_policies_are_filtered_on_elasticsearch() {
    let c = Cluster::from_env();
    if c.engine != Engine::Elasticsearch {
        eprintln!("SKIP: Elasticsearch only");
        return;
    }
    let mut seen = Vec::new();
    let kinds: &[&str] = if c.has_policies().await {
        &["index_template", "component_template", "policy"]
    } else {
        eprintln!("note: OSS build, no ILM policies to filter");
        &["index_template", "component_template"]
    };
    for &ty in kinds {
        let def = dump(&c, "", ty, &[]).await;
        let all = dump(&c, "", ty, &["--filterManaged=false"]).await;
        let managed = |v: &Value| -> Vec<String> {
            let mut out = Vec::new();
            let items: Vec<(String, Value)> = match ty {
                "policy" => v
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, p)| (k.clone(), p["policy"].clone()))
                    .collect(),
                _ => {
                    let key = format!("{ty}s");
                    v[&key]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|i| (i["name"].as_str().unwrap().to_string(), i[ty].clone()))
                        .collect()
                }
            };
            for (n, b) in items {
                if b.pointer("/_meta/managed") == Some(&json!(true)) {
                    out.push(n);
                }
            }
            out
        };
        assert!(managed(&read_doc(&def)).is_empty(), "{ty}");
        let m = managed(&read_doc(&all));
        // ES 8+ ships managed built-in templates; 7.x only partly
        if c.major().await >= 8 && ty != "policy" {
            assert!(
                !m.is_empty(),
                "{ty}: expected built-in managed entries on Elasticsearch"
            );
        }
        seen.push(format!("{ty}: {}", m.join(",")));
    }
    // recorded for the README (run with --nocapture)
    eprintln!(
        "ES {} managed built-ins: {}",
        c.version().await,
        seen.join(" | ")
    );
}

// ----------------------------------------------------------------- policy

#[tokio::test]
async fn policy_round_trip_same_engine() {
    let c = Cluster::from_env();
    if !c.has_policies().await {
        // OSS build: the tool must refuse clearly
        let f = tmp_path("oss_pol", "json");
        let o = rs(&c, &host(&c), f.to_str().unwrap(), "policy", &[]).await;
        assert!(
            !o.status.success() && err(&o).contains("OSS build"),
            "{}",
            err(&o)
        );
        eprintln!("SKIP: ILM needs X-Pack; this is an Elasticsearch OSS build (refusal checked)");
        return;
    }
    let u = unique("pol");
    match c.engine {
        Engine::Elasticsearch => {
            // max_primary_shard_size arrived in ES 7.13
            let size_key = if c.vnum().await >= (7, 13) {
                "max_primary_shard_size"
            } else {
                "max_size"
            };
            let a = json!({"policy": {"phases": {
                "hot": {"actions": {"rollover": {"max_age": "1d", (size_key): "10gb"}, "set_priority": {"priority": 100}}},
                "delete": {"min_age": "30d", "actions": {"delete": {}}}}}});
            let mut a = a;
            if c.vnum().await >= (7, 14) {
                // policy `_meta` arrived in ES 7.14
                a["policy"]["_meta"] = json!({"description": "it"});
            }
            let b = json!({"policy": {"phases": {
                "warm": {"min_age": "1d", "actions": {"forcemerge": {"max_num_segments": 1}, "shrink": {"number_of_shards": 1}, "allocate": {"number_of_replicas": 0}}}}}});
            c.ok("PUT", &format!("_ilm/policy/{u}-a"), Some(a)).await;
            c.ok("PUT", &format!("_ilm/policy/{u}-b"), Some(b)).await;
            let before = c.ok("GET", &format!("_ilm/policy/{u}-a,{u}-b"), None).await;
            let f = dump(&c, &format!("{u}-*"), "policy", &[]).await;
            assert_eq!(read_doc(&f).as_object().unwrap().len(), 2);
            for p in ["a", "b"] {
                c.ok("DELETE", &format!("_ilm/policy/{u}-{p}"), None).await;
            }
            assert_success(&rs(&c, f.to_str().unwrap(), &host(&c), "policy", &[]).await);
            let after = c.ok("GET", &format!("_ilm/policy/{u}-a,{u}-b"), None).await;
            for p in ["a", "b"] {
                let k = format!("{u}-{p}");
                assert_eq!(after[&k]["policy"], before[&k]["policy"], "{k}");
            }
            assert_success(
                &rs(
                    &c,
                    f.to_str().unwrap(),
                    &host(&c),
                    "policy",
                    &["--skip-existing"],
                )
                .await,
            );
            if node().is_some() {
                let nf = tmp_path("node_ilm", "json");
                let o = run_node(&[
                    "--input",
                    &c.url_with_auth(&format!("{u}-a")),
                    "--output",
                    nf.to_str().unwrap(),
                    "--type",
                    "policy",
                ])
                .await;
                assert!(o.status.success(), "{}", err(&o));
                c.ok("DELETE", &format!("_ilm/policy/{u}-a"), None).await;
                assert_success(&rs(&c, nf.to_str().unwrap(), &host(&c), "policy", &[]).await);
                c.ok("DELETE", &format!("_ilm/policy/{u}-a"), None).await;
                let o = run_node(&[
                    "--input",
                    f.to_str().unwrap(),
                    "--output",
                    &host(&c),
                    "--type",
                    "policy",
                ])
                .await;
                assert!(o.status.success(), "{}", err(&o));
                let g = c.ok("GET", &format!("_ilm/policy/{u}-a"), None).await;
                assert_eq!(
                    g[format!("{u}-a")]["policy"],
                    before[format!("{u}-a")]["policy"]
                );
            }
            for p in ["a", "b"] {
                c.call("DELETE", &format!("_ilm/policy/{u}-{p}"), None)
                    .await;
            }
        }
        Engine::OpenSearch => {
            // 25 policies: more than ISM's default page
            for i in 0..25 {
                c.ok(
                    "PUT",
                    &format!("_plugins/_ism/policies/{u}-{i:02}"),
                    Some(json!({"policy": {"description": "it", "default_state": "hot",
                        "states": [{"name": "hot", "actions": [], "transitions": [{"state_name": "delete", "conditions": {"min_index_age": "30d"}}]},
                                   {"name": "delete", "actions": [{"delete": {}}], "transitions": []}],
                        "ism_template": [{"index_patterns": [format!("{u}-{i:02}-*")], "priority": 1}]}})),
                )
                .await;
            }
            let pol = |v: &Value| {
                let mut p = v["policy"].clone();
                for k in ["last_updated_time", "schema_version", "policy_id"] {
                    p.as_object_mut().unwrap().remove(k);
                }
                if let Some(ts) = p["ism_template"].as_array_mut() {
                    for t in ts {
                        t.as_object_mut().unwrap().remove("last_updated_time");
                    }
                }
                p
            };
            let before = pol(&c
                .ok("GET", &format!("_plugins/_ism/policies/{u}-07"), None)
                .await);
            let f = dump(&c, &format!("{u}-*"), "policy", &[]).await;
            assert_eq!(read_doc(&f)["policies"].as_array().unwrap().len(), 25);
            // overwrite path (409 -> if_seq_no) on the same cluster
            assert_success(&rs(&c, f.to_str().unwrap(), &host(&c), "policy", &[]).await);
            // create path
            c.ok("DELETE", &format!("_plugins/_ism/policies/{u}-07"), None)
                .await;
            assert_success(
                &rs(
                    &c,
                    f.to_str().unwrap(),
                    &host(&c),
                    "policy",
                    &["--skip-existing"],
                )
                .await,
            );
            assert_eq!(
                pol(&c
                    .ok("GET", &format!("_plugins/_ism/policies/{u}-07"), None)
                    .await),
                before
            );
            if node().is_some() {
                // Node's ISM restore iterates the list keys and never restores a policy
                c.ok("DELETE", &format!("_plugins/_ism/policies/{u}-07"), None)
                    .await;
                let _ = run_node(&[
                    "--input",
                    f.to_str().unwrap(),
                    "--output",
                    &host(&c),
                    "--type",
                    "policy",
                ])
                .await;
                let (st, _) = c
                    .call("GET", &format!("_plugins/_ism/policies/{u}-07"), None)
                    .await;
                assert_eq!(st, 404, "elasticdump's ISM restore bug seems fixed");
                // Node's dump is not paginated
                let nf = tmp_path("node_ism", "json");
                let o = run_node(&[
                    "--input",
                    &host(&c),
                    "--output",
                    nf.to_str().unwrap(),
                    "--type",
                    "policy",
                ])
                .await;
                if o.status.success() {
                    let n = read_doc(&nf)["policies"]
                        .as_array()
                        .map(Vec::len)
                        .unwrap_or(0);
                    let total = read_doc(&nf)["total_policies"].as_u64().unwrap_or(0);
                    eprintln!("Node ISM dump: {n} of {total} policies");
                    assert!(n as u64 <= total);
                } else {
                    eprintln!("Node ISM dump failed: {}", err(&o));
                }
                // Node-shaped single-policy dump restores with rs
                assert_success(&rs(&c, f.to_str().unwrap(), &host(&c), "policy", &[]).await);
            }
            for i in 0..25 {
                c.call(
                    "DELETE",
                    &format!("_plugins/_ism/policies/{u}-{i:02}"),
                    None,
                )
                .await;
            }
        }
    }
}

// ----------------------------------------------------------------- script

#[tokio::test]
async fn scripts_round_trip_search_template_and_node() {
    let c = Cluster::from_env();
    let u = unique("sc");
    c.ok("PUT", &format!("_scripts/{u}-p"), Some(json!({"script": {"lang": "painless", "source": "Math.log(_score * 2) + params['my_modifier']"}}))).await;
    c.ok(
        "PUT",
        &format!("_scripts/{u}-m"),
        Some(
            json!({"script": {"lang": "mustache", "source": {"query": {"term": {"k": "{{q}}"}}}}}),
        ),
    )
    .await;
    let before_p = c.ok("GET", &format!("_scripts/{u}-p"), None).await;
    let before_m = c.ok("GET", &format!("_scripts/{u}-m"), None).await;
    let f = dump(&c, &format!("{u}*"), "script", &[]).await;
    assert_eq!(read_doc(&f)["stored_scripts"].as_array().unwrap().len(), 2);
    for x in ["p", "m"] {
        c.ok("DELETE", &format!("_scripts/{u}-{x}"), None).await;
    }
    assert_success(&rs(&c, f.to_str().unwrap(), &host(&c), "script", &[]).await);
    assert_eq!(
        c.ok("GET", &format!("_scripts/{u}-p"), None).await,
        before_p
    );
    assert_eq!(
        c.ok("GET", &format!("_scripts/{u}-m"), None).await,
        before_m
    );
    assert_success(
        &rs(
            &c,
            f.to_str().unwrap(),
            &host(&c),
            "script",
            &["--skip-existing"],
        )
        .await,
    );

    // the restored search template drives --searchWithTemplate
    let idx = unique("sc_ix");
    seed_meta_index(&c, &idx, json!({})).await;
    let out = tmp_path("sc_data", "json");
    let body = json!({"id": format!("{u}-m"), "params": {"q": "x"}}).to_string();
    let mut a = s(&[
        "--input",
        &c.url_with_auth(&idx),
        "--output",
        out.to_str().unwrap(),
        "--searchWithTemplate",
        "--searchBody",
        &body,
    ]);
    a.extend(c.tls_args("input"));
    assert_success(&run(a).await);
    assert_eq!(lines(&out).len(), 1);

    // elasticdump's fixture
    assert_success(
        &rs(
            &c,
            &fixture("stored_scripts.jsonl"),
            &host(&c),
            "script",
            &[],
        )
        .await,
    );
    assert_eq!(
        c.ok("GET", "_scripts/calculate-score", None).await["found"],
        true
    );
    if node().is_some() {
        c.ok("DELETE", &format!("_scripts/{u}-p"), None).await;
        let o = run_node(&[
            "--input",
            f.to_str().unwrap(),
            "--output",
            &host(&c),
            "--type",
            "script",
        ])
        .await;
        assert!(o.status.success(), "{}", err(&o));
        assert_eq!(
            c.ok("GET", &format!("_scripts/{u}-p"), None).await,
            before_p
        );
        let nf = tmp_path("node_sc", "json");
        let o = run_node(&[
            "--input",
            &c.url_with_auth(&format!("{u}*")),
            "--output",
            nf.to_str().unwrap(),
            "--type",
            "script",
        ])
        .await;
        assert!(o.status.success(), "{}", err(&o));
        // same entries; order follows the cluster state and may differ
        let by_id = |v: Value| {
            let mut a = v["stored_scripts"].as_array().unwrap().clone();
            a.sort_by_key(|x| x["_id"].as_str().unwrap().to_string());
            a
        };
        assert_eq!(by_id(read_doc(&nf)), by_id(read_doc(&f)));
    }
    for id in [
        format!("{u}-p"),
        format!("{u}-m"),
        "calculate-score".into(),
        "calculate-score2".into(),
    ] {
        c.call("DELETE", &format!("_scripts/{id}"), None).await;
    }
    c.delete_index(&idx).await;
}

// ----------------------------------------------------------------- rename

#[tokio::test]
async fn rename_prefix_wildcard_restore_and_copy_across_types() {
    let c = Cluster::from_env();
    let days = ["logstash-2026.01.01", "logstash-2026.01.02"];
    let renamed = ["logstash-srctag-2026.01.01", "logstash-srctag-2026.01.02"];
    for d in days.iter().chain(renamed.iter()) {
        c.delete_index(d).await;
    }
    for d in days {
        seed_meta_index(&c, d, json!({"logstash-all": {}})).await;
    }
    let rule = "logstash-:logstash-srctag-";
    let mut files = Vec::new();
    for ty in ["index", "alias", "data"] {
        let f = dump(&c, "logstash-*", ty, &[]).await;
        files.push((ty, f));
    }
    // explicit output index conflicts with rename rules
    let o = rs(
        &c,
        files[0].1.to_str().unwrap(),
        &c.url_with_auth("x"),
        "index",
        &["--renamePrefix", rule],
    )
    .await;
    assert_eq!(o.status.code(), Some(2), "{}", err(&o));
    // invalid result name fails clearly
    let o = rs(
        &c,
        files[0].1.to_str().unwrap(),
        &host(&c),
        "index",
        &["--renamePrefix", "logstash-:Logstash-"],
    )
    .await;
    assert!(
        !o.status.success() && err(&o).contains("lowercase"),
        "{}",
        err(&o)
    );

    // file restore: index (with aliases renamed), then alias, then data
    let o = rs_loud(
        &c,
        files[0].1.to_str().unwrap(),
        &host(&c),
        "index",
        &["--renamePrefix", rule, "--renameAliases", "--debug"],
    )
    .await;
    assert_success(&o);
    for (d, r) in days.iter().zip(renamed) {
        assert!(
            err(&o).contains(&format!("Renaming index '{d}' -> '{r}'")),
            "{}",
            err(&o)
        );
        assert_eq!(
            index_shape(&c, r).await["mappings"],
            index_shape(&c, d).await["mappings"]
        );
        assert!(aliases_of(&c, r).await.get("logstash-srctag-all").is_some());
    }
    assert_success(
        &rs(
            &c,
            files[1].1.to_str().unwrap(),
            &host(&c),
            "alias",
            &["--renameIndex", "^logstash-(.*)$:logstash-srctag-$1"],
        )
        .await,
    );
    for r in renamed {
        let a = aliases_of(&c, r).await;
        assert!(
            a.get("logstash-all").is_some() && a.get("logstash-srctag-all").is_some(),
            "{a}"
        );
    }
    assert_success(
        &rs(
            &c,
            files[2].1.to_str().unwrap(),
            &host(&c),
            "data",
            &["--renamePrefix", rule],
        )
        .await,
    );
    for (d, r) in days.iter().zip(renamed) {
        assert_eq!(c.docs(r).await, c.docs(d).await);
    }

    // cluster -> cluster copy with --renameIndex and --suffix
    for r in renamed {
        c.delete_index(r).await;
    }
    let rx = "^logstash-(.*)$:logstash-srctag-${1}";
    for ty in ["settings", "mapping", "data"] {
        assert_success(
            &rs(
                &c,
                &c.url_with_auth("logstash-2026*"),
                &host(&c),
                ty,
                &["--renameIndex", rx, "--suffix=-c"],
            )
            .await,
        );
    }
    for (d, r) in days.iter().zip(renamed) {
        let rc = format!("{r}-c");
        assert_eq!(c.docs(&rc).await, c.docs(d).await);
        assert_eq!(
            c.ok("GET", &format!("{rc}/_mapping"), None).await[&rc],
            c.ok("GET", &format!("{d}/_mapping"), None).await[*d]
        );
        c.delete_index(&rc).await;
    }

    // a rule mapping every day to one index is refused before anything is read (L-452)
    let o = rs(
        &c,
        &c.url_with_auth("logstash-2026*"),
        &host(&c),
        "data",
        &["--renameIndex", "^logstash-.*$:logstash-merged-l452"],
    )
    .await;
    assert_eq!(o.status.code(), Some(2), "{}", err(&o));
    assert!(err(&o).contains("--allowMerge"), "{}", err(&o));
    assert_eq!(c.call("GET", "logstash-merged-l452", None).await.0, 404);

    // templates: index_patterns only with --renamePatterns
    let u = unique("rnt");
    c.ok("PUT", &format!("_index_template/{u}"), Some(json!({"index_patterns": ["logstash-*"], "priority": 911, "template": {"aliases": {"logstash-all": {}}}}))).await;
    let tf = dump(&c, &u, "index_template", &[]).await;
    c.ok("DELETE", &format!("_index_template/{u}"), None).await;
    assert_success(
        &rs(
            &c,
            tf.to_str().unwrap(),
            &host(&c),
            "index_template",
            &["--renamePrefix", rule],
        )
        .await,
    );
    let g = c.ok("GET", &format!("_index_template/{u}"), None).await;
    assert_eq!(
        g["index_templates"][0]["index_template"]["index_patterns"],
        json!(["logstash-*"])
    );
    assert_success(
        &rs(
            &c,
            tf.to_str().unwrap(),
            &host(&c),
            "index_template",
            &[
                "--renamePrefix",
                rule,
                "--renamePatterns",
                "--renameAliases",
            ],
        )
        .await,
    );
    let g = c.ok("GET", &format!("_index_template/{u}"), None).await;
    let t = &g["index_templates"][0]["index_template"];
    assert_eq!(t["index_patterns"], json!(["logstash-srctag-*"]));
    assert!(
        t["template"]["aliases"]
            .get("logstash-srctag-all")
            .is_some(),
        "{t}"
    );
    c.ok("DELETE", &format!("_index_template/{u}"), None).await;

    // ISM ism_template patterns (OpenSearch)
    if c.engine == Engine::OpenSearch {
        let p = unique("rnp");
        c.ok(
            "PUT",
            &format!("_plugins/_ism/policies/{p}"),
            Some(json!({"policy": {"description": "r", "default_state": "a",
            "states": [{"name": "a", "actions": [], "transitions": []}],
            "ism_template": [{"index_patterns": ["logstash-*"], "priority": 5}]}})),
        )
        .await;
        let pf = dump(&c, &p, "policy", &[]).await;
        assert_success(
            &rs(
                &c,
                pf.to_str().unwrap(),
                &host(&c),
                "policy",
                &["--renamePrefix", rule, "--renamePatterns"],
            )
            .await,
        );
        let g = c
            .ok("GET", &format!("_plugins/_ism/policies/{p}"), None)
            .await;
        assert_eq!(
            g["policy"]["ism_template"][0]["index_patterns"],
            json!(["logstash-srctag-*"])
        );
        c.ok("DELETE", &format!("_plugins/_ism/policies/{p}"), None)
            .await;
    }
    for d in days.iter().chain(renamed.iter()) {
        c.delete_index(d).await;
    }
}

// ----------------------------------------------- M13: full backup/restore

#[tokio::test]
async fn full_backup_and_restore_in_documented_order() {
    let c = Cluster::from_env();
    let u = unique("full");
    let idx = format!("{u}-ix-1");
    c.ok(
        "PUT",
        &format!("_component_template/{u}-c"),
        Some(json!({"template": {"mappings": {"properties": {"ts": {"type": "date"}}}}})),
    )
    .await;
    c.ok("PUT", &format!("_index_template/{u}"), Some(json!({"index_patterns": [format!("{u}-ix-*")], "priority": 913, "composed_of": [format!("{u}-c")]}))).await;
    c.ok(
        "PUT",
        &format!("_template/{u}"),
        Some(json!({"index_patterns": [format!("{u}-legacy-*")], "order": 1})),
    )
    .await;
    let (policy_path, policy_body) = match c.engine {
        Engine::Elasticsearch => (
            format!("_ilm/policy/{u}"),
            json!({"policy": {"phases": {"delete": {"min_age": "9d", "actions": {"delete": {}}}}}}),
        ),
        Engine::OpenSearch => (
            format!("_plugins/_ism/policies/{u}"),
            json!({"policy": {"description": "f", "default_state": "a", "states": [{"name": "a", "actions": [], "transitions": []}]}}),
        ),
    };
    let policies = c.has_policies().await;
    if policies {
        c.ok("PUT", &policy_path, Some(policy_body)).await;
    }
    c.ok(
        "PUT",
        &format!("_scripts/{u}-s"),
        Some(json!({"script": {"lang": "mustache", "source": "{\"query\":{\"match_all\":{}}}"}})),
    )
    .await;
    seed_meta_index(&c, &idx, json!({ format!("{u}-al"): {} })).await;
    let want_docs = c.docs(&idx).await;
    let want_shape = index_shape(&c, &idx).await;

    let order = [
        ("component_template", format!("{u}*")),
        ("index_template", u.clone()),
        ("template", u.clone()),
        ("policy", u.clone()),
        ("script", format!("{u}*")),
        ("index", idx.clone()),
        ("mapping", idx.clone()),
        ("alias", idx.clone()),
        ("data", idx.clone()),
    ];
    let order: Vec<_> = order
        .into_iter()
        .filter(|(t, _)| policies || *t != "policy")
        .collect();
    let mut files = Vec::new();
    for (ty, name) in &order {
        files.push((*ty, dump(&c, name, ty, &[]).await));
    }
    // wipe everything
    c.delete_index(&idx).await;
    c.ok("DELETE", &format!("_index_template/{u}"), None).await;
    c.ok("DELETE", &format!("_component_template/{u}-c"), None)
        .await;
    c.ok("DELETE", &format!("_template/{u}"), None).await;
    if policies {
        c.ok("DELETE", &policy_path, None).await;
    }
    c.ok("DELETE", &format!("_scripts/{u}-s"), None).await;

    for (ty, f) in &files {
        let o = rs(&c, f.to_str().unwrap(), &host(&c), ty, &["--skip-existing"]).await;
        assert!(o.status.success(), "{ty}: {}", err(&o));
    }
    assert_eq!(index_shape(&c, &idx).await, want_shape);
    assert_eq!(c.docs(&idx).await, want_docs);
    if policies {
        c.ok("GET", &policy_path, None).await;
    }
    c.ok("GET", &format!("_scripts/{u}-s"), None).await;
    c.ok("GET", &format!("_template/{u}"), None).await;

    c.delete_index(&idx).await;
    c.call("DELETE", &format!("_index_template/{u}"), None)
        .await;
    c.call("DELETE", &format!("_component_template/{u}-c"), None)
        .await;
    c.call("DELETE", &format!("_template/{u}"), None).await;
    c.call("DELETE", &policy_path, None).await;
    c.call("DELETE", &format!("_scripts/{u}-s"), None).await;
}

/// L-504 (TST-API-083): `validate_index_name` agrees with the live cluster on generated
/// names (IT_NAME_ORACLE_N, default 300 here; 5,000 nightly in B). Every name the cluster
/// creates is deleted again.
#[tokio::test]
async fn xv_index_name_validation_agrees_with_the_cluster() {
    let c = Cluster::from_env();
    let n: usize = std::env::var("IT_NAME_ORACLE_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let alphabet: Vec<char> = "abcxyz0189.-_+:#*?\"<>|/\\, AZǅİéß😀".chars().collect();
    let mut seed: u64 = 0x5eed_1504;
    let mut next = |m: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % m
    };
    let tag = unique("nv");
    let mut names: Vec<String> = vec![
        ".".into(),
        "..".into(),
        "".into(),
        "a".repeat(255),
        "é".repeat(128),
        format!("{tag}-ok"),
    ];
    while names.len() < n {
        let len = 1 + next(8);
        let mut s: String = (0..len).map(|_| alphabet[next(alphabet.len())]).collect();
        if next(3) > 0 {
            s = format!("{tag}{s}"); // mostly unique, valid-start names
        }
        names.push(s);
    }
    let mut disagreements = Vec::new();
    for name in &names {
        let ours = elasticdump_rs::rename::validate_index_name(name).is_ok();
        let path = percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
        if path.is_empty() {
            continue; // PUT / is not an index request
        }
        let (status, body) = c
            .call(
                "PUT",
                &format!("{path}?wait_for_active_shards=0"),
                Some(json!({"settings": {"number_of_replicas": 0}})),
            )
            .await;
        let kind = body["error"]["type"].as_str().unwrap_or("").to_string();
        let theirs = status < 300 || kind == "resource_already_exists_exception";
        if status < 300 {
            let _ = c.call("DELETE", &path, None).await;
        }
        if ours != theirs {
            disagreements.push(format!("{name:?}: ours={ours} cluster={status} {kind}"));
        }
    }
    assert!(
        disagreements.is_empty(),
        "{} of {} names disagree on {}:\n{}",
        disagreements.len(),
        names.len(),
        c.version().await,
        disagreements.join("\n")
    );
}
