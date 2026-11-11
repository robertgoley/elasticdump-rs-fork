//! L-810a: environment variable names after the rename to searchdump-rs.
//!
//! Every variable of ours is now `SEARCHDUMP_*`. The old `ELASTICDUMP_*` names (elasticdump-rs,
//! before the rename) still work as deprecated aliases: the new name wins when both are set, and
//! using an old name prints a one-time deprecation warning that names the new one.

use std::collections::HashSet;
use std::ffi::OsString;
use std::sync::Mutex;

/// Prefix of every environment variable this tool reads.
pub const PREFIX: &str = "SEARCHDUMP_";
/// Deprecated prefix (before L-810a), still accepted.
pub const OLD_PREFIX: &str = "ELASTICDUMP_";

/// The deprecated alias of `name` (`SEARCHDUMP_X` -> `ELASTICDUMP_X`), if it has one.
pub fn old_name(name: &str) -> Option<String> {
    name.strip_prefix(PREFIX)
        .map(|rest| format!("{OLD_PREFIX}{rest}"))
}

fn warn_once(old: &str, new: &str) {
    static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let mut g = WARNED.lock().unwrap_or_else(|e| e.into_inner());
    if g.get_or_insert_with(HashSet::new).insert(old.to_string()) {
        let msg = format!("{old} is deprecated; use {new} (the old name still works for now)");
        if log::log_enabled!(log::Level::Warn) {
            log::warn!("{msg}");
        } else {
            eprintln!("warning: {msg}");
        }
    }
}

/// Look `name` up through `get`, then its deprecated `ELASTICDUMP_*` alias (with a one-time
/// warning). Names outside `SEARCHDUMP_*` are looked up as they are.
pub fn lookup(get: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    if let Some(v) = get(name) {
        return Some(v);
    }
    let old = old_name(name)?;
    let v = get(&old)?;
    warn_once(&old, name);
    Some(v)
}

/// `std::env::var(name)` with the `ELASTICDUMP_*` fallback (non-UTF-8 values count as unset).
pub fn var(name: &str) -> Option<String> {
    lookup(&|k| std::env::var(k).ok(), name)
}

/// `std::env::var_os(name)` with the `ELASTICDUMP_*` fallback.
pub fn var_os(name: &str) -> Option<OsString> {
    if let Some(v) = std::env::var_os(name) {
        return Some(v);
    }
    let old = old_name(name)?;
    let v = std::env::var_os(&old)?;
    warn_once(&old, name);
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_name_wins_old_name_is_an_alias() {
        let env = |k: &str| match k {
            "SEARCHDUMP_INPUT_USERNAME" => Some("new".to_string()),
            "ELASTICDUMP_INPUT_USERNAME" => Some("old".to_string()),
            "ELASTICDUMP_INPUT_PASSWORD" => Some("oldpw".to_string()),
            "OTHER" => Some("x".to_string()),
            _ => None,
        };
        assert_eq!(
            lookup(&env, "SEARCHDUMP_INPUT_USERNAME").as_deref(),
            Some("new")
        );
        assert_eq!(
            lookup(&env, "SEARCHDUMP_INPUT_PASSWORD").as_deref(),
            Some("oldpw")
        );
        assert_eq!(lookup(&env, "SEARCHDUMP_OUTPUT_PASSWORD"), None);
        assert_eq!(lookup(&env, "OTHER").as_deref(), Some("x"));
        assert_eq!(old_name("AWS_REGION"), None);
        assert_eq!(
            old_name("SEARCHDUMP_S3_SSE_C_KEY_FILE").as_deref(),
            Some("ELASTICDUMP_S3_SSE_C_KEY_FILE")
        );
    }
}
