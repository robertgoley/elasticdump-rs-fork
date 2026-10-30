//! `--input` / `--output` endpoint parsing and credential resolution.
//!
//! * `http://` / `https://` → Elasticsearch/OpenSearch. The last path segment
//!   is the index (a trailing `_search` is ignored); preceding segments are a
//!   proxy base path. A host-only URL addresses the whole cluster.
//! * `$` → stdin/stdout.
//! * anything else → a file path. `s3://` and `csv://` are out of scope.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use percent_encoding::percent_decode_str;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Input => "input",
            Direction::Output => "output",
        }
    }
    fn env_prefix(self) -> &'static str {
        match self {
            Direction::Input => "ELASTICDUMP_INPUT",
            Direction::Output => "ELASTICDUMP_OUTPUT",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct EsEndpoint {
    /// Scheme, host, port and base path (ending in `/`), no credentials.
    pub base: Url,
    /// Index name, comma list or pattern. `None` = whole cluster.
    pub index: Option<String>,
    /// Mapping type (`--input-index idx/type`); only meaningful on ES < 7.
    pub doc_type: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// Manual `Debug`: the password is never printed (F-08).
impl std::fmt::Debug for EsEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EsEndpoint")
            .field("base", &self.base.as_str())
            .field("index", &self.index)
            .field("doc_type", &self.doc_type)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Es(EsEndpoint),
    Stdio,
    File(PathBuf),
}

impl Endpoint {
    pub fn is_es(&self) -> bool {
        matches!(self, Endpoint::Es(_))
    }
}

/// Render a URL for logging with its password masked.
pub fn redacted_url(url: &Url) -> String {
    let mut redacted = url.clone();
    if url.password().is_some() {
        let _ = redacted.set_password(Some("***"));
    }
    redacted.to_string()
}

/// Mask `user:pass@` in an arbitrary endpoint string.
pub fn redacted_endpoint(raw: &str) -> String {
    match Url::parse(raw) {
        // A password with `/`, `?` or `#` can make a URL parse with the
        // password inside the host:port/path (`http://u:12/x@h`): mask the
        // raw text whenever userinfo was not recognised but `@` is present.
        Ok(url)
            if matches!(url.scheme(), "http" | "https")
                && (url.password().is_some() || !raw.contains('@')) =>
        {
            redacted_url(&url)
        }
        // Not a parseable http(s) URL (a typo, a scheme-less
        // `user:pw@host:9200/idx` that became a file path, ...): still mask
        // anything that looks like `user:password@` (F-06).
        _ => mask_userinfo(raw),
    }
}

/// Replace the password in `[scheme://]user:password@...` with `***`.
/// Everything between the first `:` of the authority and the LAST `@` is
/// masked, so a password containing `@`, `/` or spaces never leaks.
fn mask_userinfo(raw: &str) -> String {
    let is_scheme = |s: &str| {
        let mut c = s.chars();
        c.next().is_some_and(|f| f.is_ascii_alphabetic())
            && c.all(|ch| ch.is_ascii_alphanumeric() || "+.-".contains(ch))
    };
    let (scheme, rest) = match raw.find("://") {
        Some(i) if is_scheme(&raw[..i]) => raw.split_at(i + 3),
        _ => ("", raw),
    };
    let Some(at) = rest.rfind('@') else {
        return raw.to_string();
    };
    match rest[..at].find(':') {
        Some(colon) => format!("{scheme}{}:***{}", &rest[..colon], &rest[at..]),
        None => raw.to_string(),
    }
}

/// Replace every command-line argument that carries `user:password@` with
/// its masked form inside `msg` (error messages may quote a file path that
/// was really a mistyped URL; F-06).
pub fn scrub_args(msg: &str, args: &[String]) -> String {
    let mut out = msg.to_string();
    for a in args {
        let masked = mask_userinfo(a);
        if masked != *a && !a.is_empty() {
            out = out.replace(a.as_str(), &masked);
        }
        // `--input=user:pw@...`
        if let Some((_, v)) = a.split_once('=') {
            let masked = mask_userinfo(v);
            if masked != v && !v.is_empty() {
                out = out.replace(v, &masked);
            }
        }
    }
    out
}

/// `user:pw@host...` without a scheme is read as a FILE path; warn, since the
/// user probably meant a URL (F-06, BRK-CLI-008).
fn looks_like_url_without_scheme(raw: &str) -> bool {
    let head = raw.split('/').next().unwrap_or("");
    head.contains('@')
        || (head.contains(':')
            && head
                .rsplit(':')
                .next()
                .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())))
}

fn decode_component(raw: &str, what: &str, redact: bool) -> Result<String> {
    percent_decode_str(raw)
        .decode_utf8()
        .map(|d| d.into_owned())
        .map_err(|err| {
            if redact {
                anyhow!("Failed to percent-decode {what}: {err}")
            } else {
                anyhow!("Failed to percent-decode {what} '{raw}': {err}")
            }
        })
}

fn split_index_type(spec: &str) -> (Option<String>, Option<String>) {
    let mut parts = spec.splitn(2, '/');
    let index = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
    let ty = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
    (index, ty)
}

/// Parse an endpoint string. `index_override` is `--input-index` /
/// `--output-index` (`index[/type]`).
pub fn parse(raw: &str, dir: Direction, index_override: Option<&str>) -> Result<Endpoint> {
    let flag = dir.as_str();
    if raw == "$" {
        if index_override.is_some() {
            log::warn!("--{flag}-index is ignored for stdio");
        }
        return Ok(Endpoint::Stdio);
    }
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("s3://") || lower.starts_with("csv://") {
        bail!(
            "--{flag} {}: S3 and CSV transports are not supported by elasticdump-rs",
            raw.split("://").next().unwrap_or(raw)
        );
    }
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        if raw.is_empty() {
            bail!("--{flag} must not be empty");
        }
        if index_override.is_some() {
            log::warn!("--{flag}-index is ignored for file endpoints");
        }
        if looks_like_url_without_scheme(raw) {
            log::warn!(
                "--{flag} {} is treated as a FILE path; add http:// or https:// if it is a cluster",
                mask_userinfo(raw)
            );
        }
        return Ok(Endpoint::File(PathBuf::from(raw)));
    }

    let url = Url::parse(raw).with_context(|| format!("Failed to parse --{flag} URL"))?;
    if url.host_str().is_none_or(str::is_empty) {
        bail!("--{flag} URL has no host");
    }

    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let segments = match segments.last() {
        Some(&"_search") => &segments[..segments.len() - 1],
        _ => &segments[..],
    };

    let (mut index, base_segments) = match segments.split_last() {
        Some((last, rest)) => (Some(decode_component(last, "index name", false)?), rest),
        None => (None, segments),
    };
    if let Some(i) = &index {
        if i.starts_with('_') {
            bail!(
                "--{flag} URL must point to an index (or the cluster root), not an API endpoint ('{i}')"
            );
        }
    }
    let mut doc_type = None;
    if let Some(spec) = index_override {
        let (i, t) = split_index_type(spec);
        index = i;
        doc_type = t;
    }

    let username = match url.username() {
        "" => None,
        u => Some(decode_component(u, "URL username", false)?),
    };
    let password = match url.password() {
        Some(p) => Some(decode_component(p, "URL password", true)?),
        None => None,
    };

    if let Some(query) = url.query() {
        let names = query
            .split('&')
            .map(|p| p.split('=').next().unwrap_or(p))
            .collect::<Vec<_>>()
            .join(", ");
        log::warn!(
            "Ignoring query string parameters in --{flag} URL ({names}); use --{flag}-params"
        );
    }

    let mut base = url.clone();
    let _ = base.set_username("");
    let _ = base.set_password(None);
    base.set_query(None);
    base.set_fragment(None);
    let path = if base_segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}/", base_segments.join("/"))
    };
    base.set_path(&path);

    Ok(Endpoint::Es(EsEndpoint {
        base,
        index,
        doc_type,
        username,
        password,
    }))
}

/// Parse an `--httpAuthFile` (INI with `user=` and `password=`).
pub fn read_auth_file(path: &str) -> Result<(String, String)> {
    let ini = ini::Ini::load_from_file(path)
        .with_context(|| format!("Failed to read --httpAuthFile '{path}'"))?;
    let section = ini.general_section();
    match (section.get("user"), section.get("password")) {
        (Some(u), Some(p)) => Ok((u.to_string(), p.to_string())),
        _ => bail!("Malformed Auth File: '{path}' must define both user= and password="),
    }
}

/// Fill in credentials that the URL did not carry, in Node order: URL →
/// `--httpAuthFile` → `ELASTICDUMP_{INPUT,OUTPUT}_{USERNAME,PASSWORD}`.
/// `cli_user`/`cli_pass` (elasticdump-rs `--username/--password`) override
/// everything.
pub fn resolve_auth(
    ep: &mut EsEndpoint,
    dir: Direction,
    auth_file: Option<&(String, String)>,
    env: &dyn Fn(&str) -> Option<String>,
    cli_user: Option<&str>,
    cli_pass: Option<&str>,
) {
    if ep.username.is_none() && ep.password.is_none() {
        if let Some((u, p)) = auth_file {
            ep.username = Some(u.clone());
            ep.password = Some(p.clone());
        } else {
            let user = env(&format!("{}_USERNAME", dir.env_prefix()));
            let pass = env(&format!("{}_PASSWORD", dir.env_prefix()));
            if user.is_some() {
                ep.username = user;
                ep.password = pass;
            }
        }
    }
    if let Some(u) = cli_user {
        ep.username = Some(u.to_string());
    }
    if let Some(p) = cli_pass {
        ep.password = Some(p.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn es(raw: &str, ov: Option<&str>) -> EsEndpoint {
        match parse(raw, Direction::Input, ov).unwrap() {
            Endpoint::Es(e) => e,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_index_and_base_path() {
        let e = es("http://localhost:9200/my_index", None);
        assert_eq!(e.base.as_str(), "http://localhost:9200/");
        assert_eq!(e.index.as_deref(), Some("my_index"));
        let e = es("https://h/proxy/es/idx/_search", None);
        assert_eq!(e.base.as_str(), "https://h/proxy/es/");
        assert_eq!(e.index.as_deref(), Some("idx"));
    }

    #[test]
    fn host_only_url_has_no_index() {
        let e = es("http://localhost:9200/", None);
        assert_eq!(e.index, None);
        assert_eq!(e.base.as_str(), "http://localhost:9200/");
    }

    #[test]
    fn index_override_replaces_url_index_and_sets_type() {
        let e = es("http://h:9200/ignored", Some("real/doc"));
        assert_eq!(e.index.as_deref(), Some("real"));
        assert_eq!(e.doc_type.as_deref(), Some("doc"));
        let e = es("http://h:9200", Some("only"));
        assert_eq!(e.index.as_deref(), Some("only"));
    }

    #[test]
    fn credentials_are_decoded_and_stripped() {
        let e = es("http://us%40er:p%3Ass@h:9200/i", None);
        assert_eq!(e.username.as_deref(), Some("us@er"));
        assert_eq!(e.password.as_deref(), Some("p:ss"));
        assert_eq!(e.base.as_str(), "http://h:9200/");
    }

    #[test]
    fn rejects_api_paths_and_oos_schemes() {
        assert!(parse("http://h/_cat", Direction::Input, None).is_err());
        assert!(parse("s3://bucket/key", Direction::Input, None).is_err());
        assert!(parse("csv:///tmp/x.csv", Direction::Output, None).is_err());
    }

    #[test]
    fn file_and_stdio() {
        assert_eq!(
            parse("$", Direction::Output, None).unwrap(),
            Endpoint::Stdio
        );
        assert_eq!(
            parse("/tmp/x.json", Direction::Output, None).unwrap(),
            Endpoint::File("/tmp/x.json".into())
        );
    }

    #[test]
    fn auth_precedence_url_then_file_then_env_then_cli_override() {
        let env = |k: &str| match k {
            "ELASTICDUMP_INPUT_USERNAME" => Some("envu".to_string()),
            "ELASTICDUMP_INPUT_PASSWORD" => Some("envp".to_string()),
            _ => None,
        };
        let file = ("fileu".to_string(), "filep".to_string());

        let mut e = es("http://urlu:urlp@h/i", None);
        resolve_auth(&mut e, Direction::Input, Some(&file), &env, None, None);
        assert_eq!(e.username.as_deref(), Some("urlu"));

        let mut e = es("http://h/i", None);
        resolve_auth(&mut e, Direction::Input, Some(&file), &env, None, None);
        assert_eq!(
            (e.username.as_deref(), e.password.as_deref()),
            (Some("fileu"), Some("filep"))
        );

        let mut e = es("http://h/i", None);
        resolve_auth(&mut e, Direction::Input, None, &env, None, None);
        assert_eq!(
            (e.username.as_deref(), e.password.as_deref()),
            (Some("envu"), Some("envp"))
        );

        let mut e = es("http://h/i", None);
        resolve_auth(&mut e, Direction::Output, None, &env, None, None);
        assert_eq!(e.username, None);

        let mut e = es("http://urlu:urlp@h/i", None);
        resolve_auth(
            &mut e,
            Direction::Input,
            None,
            &env,
            Some("cli"),
            Some("clip"),
        );
        assert_eq!(
            (e.username.as_deref(), e.password.as_deref()),
            (Some("cli"), Some("clip"))
        );
    }

    #[test]
    fn auth_file_requires_both_keys() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("ok.ini");
        std::fs::write(&ok, "user=alice\npassword=s3cr=t\n").unwrap();
        assert_eq!(
            read_auth_file(ok.to_str().unwrap()).unwrap(),
            ("alice".into(), "s3cr=t".into())
        );
        let bad = dir.path().join("bad.ini");
        std::fs::write(&bad, "user=alice\n").unwrap();
        let err = read_auth_file(bad.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("Malformed Auth File"));
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::*;

    /// F-06: unparseable or scheme-less endpoints are still masked.
    #[test]
    fn redacted_endpoint_masks_userinfo_without_a_valid_url() {
        assert_eq!(
            redacted_endpoint("elastic:s3cr3t@host:9200/idx"),
            "elastic:***@host:9200/idx"
        );
        assert_eq!(
            redacted_endpoint("http://u:p w@bad host/idx"),
            "http://u:***@bad host/idx"
        );
        assert_eq!(redacted_endpoint("/tmp/dump.json"), "/tmp/dump.json");
        assert_eq!(
            redacted_endpoint("https://u:pw@h:9200/i"),
            "https://u:***@h:9200/i"
        );
        assert!(looks_like_url_without_scheme("elastic:pw@host:9200/idx"));
        assert!(looks_like_url_without_scheme("localhost:9200/idx"));
        assert!(!looks_like_url_without_scheme("/tmp/x.json"));
        assert!(!looks_like_url_without_scheme("dump.json"));
    }
}

/// L-503 (TST-API-082): URL parsing and redaction properties.
#[cfg(test)]
mod url_props {
    use super::*;
    use proptest::prelude::*;

    fn cases() -> u32 {
        std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10_000)
    }

    /// Passwords start with a marker that appears nowhere else in the URL.
    fn password() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            prop_oneof![
                Just(':'),
                Just('@'),
                Just('%'),
                Just('#'),
                Just('/'),
                Just('?'),
                Just(' '),
                Just('é'),
                Just('\\'),
                proptest::char::range('0', '9'),
                proptest::char::range('A', 'Z'),
            ],
            0..10,
        )
        .prop_map(|v| format!("Qz{}", v.into_iter().collect::<String>()))
        .prop_flat_map(|p| ("[0-9]{0,3}/?", Just(p)).prop_map(|(d, p)| format!("{d}{p}")))
    }

    fn pct(s: &str) -> String {
        percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: cases(), ..ProptestConfig::default() })]

        /// No redacted rendering ever contains the password, raw or
        /// percent-encoded, whatever the URL looks like; parsing never panics
        /// and its errors never quote the password either.
        #[test]
        fn redaction_never_shows_the_password(
            scheme in prop_oneof![Just("http://"), Just("https://"), Just("HTTP://"), Just(""), Just("http:/")],
            user in "[a-z]{0,6}",
            pass in password(),
            encode in any::<bool>(),
            host in prop_oneof![Just("localhost".to_string()), Just("10.0.0.1".to_string()), Just("[::1]".to_string()), "[a-z]{1,8}", Just("bad host".to_string())],
            port in prop_oneof![Just(String::new()), (1u16..65535).prop_map(|p| format!(":{p}"))],
            path in prop_oneof![Just(String::new()), "/[a-z]{1,6}", "/[a-z]{1,4}/[a-z]{1,4}", Just("/i?x=1".to_string())],
        ) {
            let p = if encode { pct(&pass) } else { pass.clone() };
            let raw = format!("{scheme}{user}:{p}@{host}{port}{path}");
            let red = redacted_endpoint(&raw);
            for secret in [pass.as_str(), pct(&pass).as_str(), "Qz"] {
                prop_assert!(!red.contains(secret), "{raw} -> {red}");
            }
            for dir in [Direction::Input, Direction::Output] {
                if let Err(e) = parse(&raw, dir, None) {
                    let msg = scrub_args(&format!("{e:#}"), std::slice::from_ref(&raw));
                    prop_assert!(!msg.contains("Qz"), "{raw} -> {msg}");
                }
            }
        }

        /// Arbitrary strings never make parsing or redaction panic.
        #[test]
        fn arbitrary_endpoints_never_panic(raw in "\\PC{0,40}", idx in proptest::option::of("\\PC{0,10}")) {
            let _ = redacted_endpoint(&raw);
            let _ = parse(&raw, Direction::Input, idx.as_deref());
            let _ = parse(&raw, Direction::Output, idx.as_deref());
            let _ = scrub_args(&raw, std::slice::from_ref(&raw));
        }
    }
}
