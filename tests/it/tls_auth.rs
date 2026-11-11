//! M1: TLS and authentication against the secured profiles (OpenSearch demo
//! security, or bitnami Elasticsearch with basic auth + TLS). Every test is a
//! no-op on the plain profiles.

use crate::harness::*;

struct Secure {
    c: Cluster,
    ca: String,
    cert: String,
    key: String,
    key_enc: String,
    key_pass: String,
    user: String,
    pw: String,
}

fn secure() -> Option<Secure> {
    if !is_secure() {
        eprintln!("skipping: not a secured profile");
        return None;
    }
    Some(Secure {
        c: Cluster::from_env(),
        ca: env("IT_CA")?,
        cert: env("IT_CLIENT_CERT")?,
        key: env("IT_CLIENT_KEY")?,
        key_enc: env("IT_CLIENT_KEY_ENC")?,
        key_pass: env("IT_CLIENT_KEY_PASS")?,
        user: env("IT_ADMIN_USER")?,
        pw: env("IT_ADMIN_PW")?,
    })
}

async fn seeded(s: &Secure, label: &str) -> String {
    let index = unique(label);
    seed_index(&s.c, &index, 50).await;
    index
}

async fn dump(
    args: Vec<String>,
    envs: Vec<(String, String)>,
) -> (std::process::Output, std::path::PathBuf) {
    let out = tmp_path("tls", "json");
    let mut a = args;
    a.extend(s(&["--output", out.to_str().unwrap()]));
    (run_env(a, envs).await, out)
}

async fn expect_full_dump(s: &Secure, index: &str, args: Vec<String>, envs: Vec<(String, String)>) {
    let (o, out) = dump(args, envs).await;
    assert_success(&o);
    assert_eq!(lines(&out).len() as u64, s.c.count(index).await);
    let _ = std::fs::remove_file(out);
}

#[tokio::test]
async fn input_ca_with_url_credentials() {
    let Some(s) = secure() else { return };
    let index = seeded(&s, "tls_ca").await;
    expect_full_dump(
        &s,
        &index,
        s_args(&["--input", &s.c.url_with_auth(&index), "--input-ca", &s.ca]),
        vec![],
    )
    .await;
    // shared --ca and legacy --caFile work too
    expect_full_dump(
        &s,
        &index,
        s_args(&["--input", &s.c.url_with_auth(&index), "--ca", &s.ca]),
        vec![],
    )
    .await;
    expect_full_dump(
        &s,
        &index,
        s_args(&["--input", &s.c.url_with_auth(&index), "--caFile", &s.ca]),
        vec![],
    )
    .await;
    s.c.delete_index(&index).await;
}

#[tokio::test]
async fn verification_fails_without_ca_and_insecure_skips_it() {
    let Some(s) = secure() else { return };
    let index = seeded(&s, "tls_insecure").await;
    let (o, _) = dump(
        s_args(&["--input", &s.c.url_with_auth(&index), "--noImplicitRetry"]),
        vec![],
    )
    .await;
    assert!(
        !o.status.success(),
        "self-signed cert must be rejected without a CA"
    );
    let err = String::from_utf8_lossy(&o.stderr).to_lowercase();
    assert!(
        err.contains("certificate") || err.contains("unknownissuer"),
        "{err}"
    );

    expect_full_dump(
        &s,
        &index,
        s_args(&["--input", &s.c.url_with_auth(&index), "--insecure"]),
        vec![],
    )
    .await;
    expect_full_dump(
        &s,
        &index,
        s_args(&["--input", &s.c.url_with_auth(&index), "--input-insecure"]),
        vec![],
    )
    .await;
    expect_full_dump(
        &s,
        &index,
        s_args(&["--input", &s.c.url_with_auth(&index)]),
        vec![("NODE_TLS_REJECT_UNAUTHORIZED".into(), "0".into())],
    )
    .await;
    s.c.delete_index(&index).await;
}

#[tokio::test]
async fn client_certificate_plain_and_encrypted_key() {
    let Some(s) = secure() else { return };
    let index = seeded(&s, "tls_mtls").await;
    // OpenSearch: the admin (kirk) certificate authenticates on its own.
    // Elasticsearch: certificates are verified (client_authentication:
    // optional) but authentication stays basic auth.
    let url = if cert_auth() {
        s.c.index_url(&index)
    } else {
        s.c.url_with_auth(&index)
    };
    if !cert_auth() {
        // prove the client certificate is really presented: one from an
        // unknown CA fails the handshake
        let d = tmp_path("rogue", "d");
        std::fs::create_dir_all(&d).unwrap();
        let (k, c) = (d.join("k.pem"), d.join("c.pem"));
        let st = std::process::Command::new("openssl")
            .args([
                "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
            ])
            .args(["-subj", "/CN=rogue", "-keyout"])
            .arg(&k)
            .arg("-out")
            .arg(&c)
            .output()
            .unwrap();
        assert!(st.status.success());
        let (o, _) = dump(
            s_args(&[
                "--input",
                &url,
                "--input-ca",
                &s.ca,
                "--input-cert",
                c.to_str().unwrap(),
                "--input-key",
                k.to_str().unwrap(),
                "--noImplicitRetry",
            ]),
            vec![],
        )
        .await;
        assert!(
            !o.status.success(),
            "untrusted client cert must be rejected"
        );
    }
    expect_full_dump(
        &s,
        &index,
        s_args(&[
            "--input",
            &url,
            "--input-ca",
            &s.ca,
            "--input-cert",
            &s.cert,
            "--input-key",
            &s.key,
            "--tlsAuth",
        ]),
        vec![],
    )
    .await;
    expect_full_dump(
        &s,
        &index,
        s_args(&[
            "--input",
            &url,
            "--input-ca",
            &s.ca,
            "--input-cert",
            &s.cert,
            "--input-key",
            &s.key_enc,
            "--input-pass",
            &s.key_pass,
            "--tlsAuth",
        ]),
        vec![],
    )
    .await;
    // wrong passphrase is a clear error
    let (o, _) = dump(
        s_args(&[
            "--input",
            &url,
            "--input-ca",
            &s.ca,
            "--input-cert",
            &s.cert,
            "--input-key",
            &s.key_enc,
            "--input-pass",
            "wrong",
            "--noImplicitRetry",
        ]),
        vec![],
    )
    .await;
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr).to_lowercase();
    assert!(
        err.contains("decrypt") || err.contains("passphrase"),
        "{err}"
    );
    s.c.delete_index(&index).await;
}

#[tokio::test]
async fn credentials_from_env_and_auth_file() {
    let Some(s) = secure() else { return };
    let index = seeded(&s, "tls_creds").await;
    let url = s.c.index_url(&index);

    // no credentials at all → 401
    let (o, _) = dump(
        s_args(&["--input", &url, "--input-ca", &s.ca, "--noImplicitRetry"]),
        vec![],
    )
    .await;
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("401"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );

    expect_full_dump(
        &s,
        &index,
        s_args(&["--input", &url, "--input-ca", &s.ca]),
        vec![
            ("SEARCHDUMP_INPUT_USERNAME".into(), s.user.clone()),
            ("SEARCHDUMP_INPUT_PASSWORD".into(), s.pw.clone()),
        ],
    )
    .await;

    let auth = tmp_path("auth", "ini");
    std::fs::write(&auth, format!("user={}\npassword={}\n", s.user, s.pw)).unwrap();
    expect_full_dump(
        &s,
        &index,
        s_args(&[
            "--input",
            &url,
            "--input-ca",
            &s.ca,
            "--httpAuthFile",
            auth.to_str().unwrap(),
        ]),
        vec![],
    )
    .await;
    let _ = std::fs::remove_file(auth);
    s.c.delete_index(&index).await;
}

fn s_args(a: &[&str]) -> Vec<String> {
    s(a)
}
