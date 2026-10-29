use crate::harness::*;

/// M0 smoke test: a full scroll dump of a seeded 2-shard index returns every
/// document.
#[tokio::test]
async fn scroll_dump_returns_all_docs() {
    let c = Cluster::from_env();
    let index = unique("smoke");
    seed_index(&c, &index, 500).await;
    let out = tmp_path("smoke", "json");
    let mut args = s(&["--input", &c.url_with_auth(&index), "--output", out.to_str().unwrap()]);
    args.extend(c.tls_args("input"));
    let o = run(args).await;
    assert_success(&o);
    assert_eq!(lines(&out).len() as u64, c.count(&index).await);
    c.delete_index(&index).await;
    let _ = std::fs::remove_file(out);
}
