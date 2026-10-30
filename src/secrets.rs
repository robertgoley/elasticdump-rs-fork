//! Process-wide registry of secret values (TST-BRK-CLI-007). Every
//! credential the run was given is registered once at start-up; log lines and
//! the final error message are scrubbed of them, so a server that echoes a
//! request (headers, query string) back in an error body cannot make a secret
//! reach stderr.

use std::sync::RwLock;

use base64::Engine;

static SECRETS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Shortest value treated as a secret (shorter ones would mangle messages).
const MIN_LEN: usize = 4;

/// Register `value` (and its percent-encoded form).
pub fn register(value: &str) {
    let value = value.trim();
    if value.len() < MIN_LEN {
        return;
    }
    let enc = percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC)
        .to_string();
    let mut s = SECRETS.write().unwrap_or_else(|e| e.into_inner());
    for v in [value.to_string(), enc] {
        if !s.contains(&v) {
            s.push(v);
        }
    }
    // longest first, so a secret containing another is replaced whole
    s.sort_by_key(|v| std::cmp::Reverse(v.len()));
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
    let s = SECRETS.read().unwrap_or_else(|e| e.into_inner());
    let mut out = msg.to_string();
    for v in s.iter() {
        if out.contains(v.as_str()) {
            out = out.replace(v.as_str(), "***");
        }
    }
    out
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
}
