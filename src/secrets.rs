//! Process-wide registry of secret values (TST-BRK-CLI-007). Every
//! credential the run was given is registered once at start-up; log lines and
//! the final error message are scrubbed of them, so a server that echoes a
//! request (headers, query string) back in an error body cannot make a secret
//! reach stderr.

use std::sync::RwLock;

use base64::Engine;

static SECRETS: RwLock<Registry> = RwLock::new(Registry::new());

/// Shortest value treated as a secret (shorter ones would mangle messages).
/// A shorter password is still hidden inside its `Authorization: Basic`
/// token, which is always longer.
pub const MIN_LEN: usize = 4;

/// A set of secret strings, each kept in every form it can take in a
/// message. The process-wide registry is one of these; tests and the fuzz
/// target use their own.
#[derive(Default)]
pub struct Registry {
    values: Vec<String>,
}

impl Registry {
    pub const fn new() -> Self {
        Self { values: Vec::new() }
    }

    /// Add `value` in the forms it can take in a message: as is, trimmed,
    /// percent-encoded (`%XX` for every non-alphanumeric byte, as a URL's
    /// userinfo, and as a form field), JSON-escaped (a server echoing it
    /// inside a JSON string) and `{:?}`-escaped.
    pub fn add(&mut self, value: &str) {
        let trimmed = value.trim();
        if trimmed.len() < MIN_LEN {
            return;
        }
        let mut forms = vec![value.to_string(), trimmed.to_string()];
        forms.push(
            percent_encoding::utf8_percent_encode(trimmed, percent_encoding::NON_ALPHANUMERIC)
                .to_string(),
        );
        forms.push(url::form_urlencoded::byte_serialize(trimmed.as_bytes()).collect());
        if let Ok(mut u) = url::Url::parse("http://h/")
            && u.set_password(Some(trimmed)).is_ok()
            && let Some(p) = u.password()
        {
            forms.push(p.to_string());
        }
        if let Ok(json) = serde_json::to_string(trimmed) {
            forms.push(json[1..json.len() - 1].to_string());
        }
        let debug = format!("{trimmed:?}");
        forms.push(debug[1..debug.len() - 1].to_string());
        for v in forms {
            if v.len() >= MIN_LEN && !self.values.contains(&v) {
                self.values.push(v);
            }
        }
        // longest first, so a secret containing another is replaced whole
        self.values.sort_by_key(|v| std::cmp::Reverse(v.len()));
    }

    /// Replace every secret in `msg` with `***`. Repeats until none is
    /// left: a replacement can create a new match (`x***` inside `xx***`),
    /// and each round shortens the text, so this ends.
    pub fn scrub(&self, msg: &str) -> String {
        let mut out = msg.to_string();
        loop {
            let before = out.len();
            for v in &self.values {
                if out.contains(v.as_str()) {
                    out = out.replace(v.as_str(), "***");
                }
            }
            if out.len() == before {
                return out;
            }
        }
    }
}

/// Register `value` (in every form, see [`Registry::add`]).
pub fn register(value: &str) {
    SECRETS
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .add(value);
}

/// Register a basic-auth password and the `Authorization: Basic` token it makes.
pub fn register_basic(user: Option<&str>, password: &str) {
    register(password);
    let token = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{password}", user.unwrap_or_default()));
    register(&token);
}

/// Register a header value when its name looks credential-bearing
/// (`Authorization: Bearer X` registers `X` too).
pub fn register_header(name: &str, value: &str) {
    if is_sensitive_name(name) {
        register(value);
        if let Some((_, rest)) = value.split_once(' ') {
            register(rest);
        }
    }
}

/// Names of headers and query parameters that carry credentials.
pub fn is_sensitive_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "auth",
        "key",
        "token",
        "secret",
        "pass",
        "cookie",
        "session",
        "signature",
        "credential",
    ]
    .iter()
    .any(|k| n.contains(k))
}

/// Replace every registered secret in `msg` with `***`.
pub fn scrub(msg: &str) -> String {
    SECRETS.read().unwrap_or_else(|e| e.into_inner()).scrub(msg)
}

#[cfg(any(test, feature = "fuzz"))]
/// Every way a password can be rendered in a log line or error, with
/// the exact text of the password in that rendering.
pub fn renderings(user: &str, pw: &str) -> Vec<(String, String)> {
    let Ok(mut url) = url::Url::parse("http://h:9200/idx") else {
        return Vec::new();
    };
    let _ = url.set_username(user);
    let _ = url.set_password(Some(pw));
    let in_url = url.password().unwrap_or(pw).to_string();
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pw}"));
    let pct =
        percent_encoding::utf8_percent_encode(pw, percent_encoding::NON_ALPHANUMERIC).to_string();
    let form: String = url::form_urlencoded::byte_serialize(pw.as_bytes()).collect();
    let json = serde_json::to_string(pw).unwrap_or_default();
    let json = json
        .get(1..json.len().saturating_sub(1))
        .unwrap_or_default()
        .to_string();
    let debug = format!("{pw:?}");
    let debug = debug
        .get(1..debug.len().saturating_sub(1))
        .unwrap_or_default()
        .to_string();
    vec![
        (format!("raw {pw} end"), pw.to_string()),
        (format!("x{pw}{pw}y"), pw.to_string()),
        (format!("url {url} end"), in_url),
        (format!("pct {pct}"), pct),
        (format!("form {form}"), form),
        (
            format!("json {}", serde_json::json!({ "reason": pw })),
            json,
        ),
        (format!("debug {pw:?}"), debug),
        (format!("Authorization: Basic {basic}"), basic),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_secrets_are_scrubbed_in_every_form() {
        register_basic(Some("elastic"), "Canary-Pw-1/x");
        register_header("Authorization", "Bearer Canary-Tok-2");
        register_header("X-Opaque-Id", "not-a-secret");
        register("ab"); // too short: ignored
        let basic = base64::engine::general_purpose::STANDARD.encode("elastic:Canary-Pw-1/x");
        let msg = format!(
            "pw Canary-Pw-1/x enc Canary%2DPw%2D1%2Fx basic {basic} tok Canary-Tok-2 id not-a-secret ab"
        );
        let out = scrub(&msg);
        assert!(!out.contains("Canary"), "{out}");
        assert!(!out.contains(&basic), "{out}");
        assert!(
            out.contains("not-a-secret") && out.ends_with(" ab"),
            "{out}"
        );
    }

    /// L-645 / D-4: a password with `: @ % # / ?`, spaces, quotes, `\\`,
    /// unicode or `*` never survives scrubbing, in any form a message can
    /// carry it (raw, URL userinfo, percent/form-encoded, JSON, `{:?}`,
    /// Basic token).
    #[test]
    fn hostile_passwords_are_scrubbed_in_every_form() {
        let samples = [
            "FAKE-pw: @%#/?",
            "FAKE pw with spaces",
            "FAKE-\"quoted\"\\back",
            "FAKE-ünïcødé-€-😀",
            "FAKE-tab\tnl\ncr\rx",
            "x***",
            "FAKE%41-already-encoded",
        ];
        for pw in samples {
            let mut r = Registry::new();
            r.add(pw);
            r.add(&base64::engine::general_purpose::STANDARD.encode(format!("fakeuser:{pw}")));
            for (msg, needle) in super::renderings("fakeuser", pw) {
                let out = r.scrub(&msg);
                assert!(
                    !out.contains(&needle),
                    "{pw:?} survived as {needle:?} in {out:?}"
                );
            }
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]
        /// Fuzz the same property over generated passwords (fake prefix, so
        /// nothing here resembles a real credential).
        #[test]
        fn generated_passwords_never_survive_scrubbing(
            tail in "[ -~\\t\\r\\nüé€😀]{0,23}[!-~üé€😀]",
        ) {
            let pw = format!("FAKE-{tail}");
            let mut r = Registry::new();
            r.add(&pw);
            r.add(&base64::engine::general_purpose::STANDARD.encode(format!("u:{pw}")));
            for (msg, needle) in super::renderings("u", &pw) {
                let out = r.scrub(&msg);
                proptest::prop_assert!(!out.contains(&needle), "{:?} as {:?} in {:?}", pw, needle, out);
            }
        }
    }
}
