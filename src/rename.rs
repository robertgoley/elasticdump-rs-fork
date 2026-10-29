//! Index renaming on restore and copy (extension): `--renamePrefix old:new`
//! and `--renameIndex '<regex>:<replacement>'`.
//!
//! * Rules are tried in order (`--renamePrefix` rules first, then
//!   `--renameIndex`, each in command-line order); the first rule that
//!   matches wins. Names no rule matches keep their name.
//! * The renamed name then gets `--prefix` / `--suffix`.
//! * Rules rename *source* names, so they need a host-only `--output`
//!   (no explicit single `--output-index`).
//! * Each resulting name is validated against the index naming rules and
//!   each rename is logged once.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{Result, anyhow, bail};
use regex::Regex;

#[derive(Debug, Clone)]
enum Rule {
    Prefix { from: String, to: String },
    Regex { re: Regex, rep: String, raw: String },
}

impl Rule {
    fn apply(&self, name: &str) -> Option<String> {
        match self {
            Rule::Prefix { from, to } => {
                name.strip_prefix(from.as_str()).map(|r| format!("{to}{r}"))
            }
            Rule::Regex { re, rep, .. } => re
                .is_match(name)
                .then(|| re.replacen(name, 1, rep.as_str()).into_owned()),
        }
    }

    fn describe(&self) -> String {
        match self {
            Rule::Prefix { from, to } => format!("--renamePrefix {from}:{to}"),
            Rule::Regex { raw, .. } => format!("--renameIndex {raw}"),
        }
    }
}

/// Split `a:b` at the *last* colon that is not inside the replacement's
/// `${..}` (regexes may contain `:` themselves, e.g. `(?:x)`).
fn split_spec(spec: &str, flag: &str) -> Result<(String, String)> {
    let idx = spec
        .rfind(':')
        .ok_or_else(|| anyhow!("--{flag} '{spec}' must be '<from>:<to>'"))?;
    Ok((spec[..idx].to_string(), spec[idx + 1..].to_string()))
}

#[derive(Debug, Default)]
pub struct IndexRenamer {
    rules: Vec<Rule>,
    pub prefix: String,
    pub suffix: String,
    cache: Mutex<HashMap<String, String>>,
}

impl Clone for IndexRenamer {
    fn clone(&self) -> Self {
        Self {
            rules: self.rules.clone(),
            prefix: self.prefix.clone(),
            suffix: self.suffix.clone(),
            cache: Mutex::new(HashMap::new()),
        }
    }
}

impl IndexRenamer {
    pub fn new(
        prefixes: &[String],
        regexes: &[String],
        prefix: &str,
        suffix: &str,
    ) -> Result<Self> {
        let mut rules = Vec::new();
        for spec in prefixes {
            let (from, to) = split_spec(spec, "renamePrefix")?;
            if from.is_empty() {
                bail!(
                    "--renamePrefix '{spec}': the old prefix must not be empty (use --prefix to prepend)"
                );
            }
            rules.push(Rule::Prefix { from, to });
        }
        for spec in regexes {
            let (re, rep) = split_spec(spec, "renameIndex")?;
            let re = Regex::new(&re).map_err(|e| {
                anyhow!("--renameIndex '{spec}': invalid regex (look-around and backreferences are not supported): {e}")
            })?;
            rules.push(Rule::Regex {
                re,
                rep,
                raw: spec.clone(),
            });
        }
        Ok(Self {
            rules,
            prefix: prefix.to_string(),
            suffix: suffix.to_string(),
            cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn has_rules(&self) -> bool {
        !self.rules.is_empty()
    }

    /// Apply the first matching rule (no prefix/suffix, no validation).
    pub fn rename_raw(&self, name: &str) -> Option<(String, String)> {
        self.rules
            .iter()
            .find_map(|r| r.apply(name).map(|n| (n, r.describe())))
    }

    /// Final index name for source index `name`: rename, then prefix and
    /// suffix, then validate. Logged once per name.
    pub fn index(&self, name: &str) -> Result<String> {
        if let Some(hit) = self.cache.lock().unwrap().get(name) {
            return Ok(hit.clone());
        }
        let (base, rule) = match self.rename_raw(name) {
            Some((n, r)) => (n, Some(r)),
            None => (name.to_string(), None),
        };
        let out = format!("{}{}{}", self.prefix, base, self.suffix);
        // Every name taken from a file or a server response is validated,
        // renamed or not: `*`, `_all`, `a,b`, `.` or `..` must never reach a
        // URL path or a bulk/alias action (INJ-F2).
        if out == name && rule.is_none() {
            validate_index_name(&out)
                .map_err(|e| anyhow!("invalid index name '{name}' in the input: {e}"))?;
        } else {
            validate_index_name(&out).map_err(|e| {
                anyhow!(
                    "renaming index '{name}' to '{out}'{}: {e}",
                    rule.as_deref()
                        .map(|r| format!(" ({r})"))
                        .unwrap_or_default()
                )
            })?;
        }
        if let Some(r) = &rule {
            log::info!("Renaming index '{name}' -> '{out}' ({r})");
        }
        self.cache
            .lock()
            .unwrap()
            .insert(name.to_string(), out.clone());
        Ok(out)
    }

    /// Rename an alias name (`--renameAliases`): rules only, no prefix/suffix.
    pub fn alias(&self, name: &str) -> Result<String> {
        match self.rename_raw(name) {
            Some((out, rule)) => {
                validate_index_name(&out)
                    .map_err(|e| anyhow!("renaming alias '{name}' to '{out}' ({rule}): {e}"))?;
                log::info!("Renaming alias '{name}' -> '{out}' ({rule})");
                Ok(out)
            }
            None => Ok(name.to_string()),
        }
    }

    /// Rename an index pattern (`--renamePatterns`): rules only; `*` and `?`
    /// stay allowed.
    pub fn pattern(&self, pattern: &str) -> Result<String> {
        match self.rename_raw(pattern) {
            Some((out, rule)) => {
                validate_pattern(&out).map_err(|e| {
                    anyhow!("renaming pattern '{pattern}' to '{out}' ({rule}): {e}")
                })?;
                log::info!("Renaming index pattern '{pattern}' -> '{out}' ({rule})");
                Ok(out)
            }
            None => Ok(pattern.to_string()),
        }
    }
}

const ILLEGAL: &[char] = &['\\', '/', '*', '?', '"', '<', '>', '|', ' ', ',', '#', ':'];

/// Elasticsearch/OpenSearch index naming rules.
pub fn validate_index_name(name: &str) -> Result<()> {
    check_name(name, ILLEGAL)
}

fn validate_pattern(p: &str) -> Result<()> {
    let illegal: Vec<char> = ILLEGAL
        .iter()
        .copied()
        .filter(|c| *c != '*' && *c != '?')
        .collect();
    check_name(p, &illegal)
}

fn check_name(name: &str, illegal: &[char]) -> Result<()> {
    if name.is_empty() {
        bail!("index names must not be empty");
    }
    if name == "." || name == ".." {
        bail!("index names cannot be '.' or '..'");
    }
    if name.starts_with(['-', '_', '+']) {
        bail!("index names must not start with '-', '_' or '+'");
    }
    if name.chars().any(|c| c.is_uppercase()) {
        bail!("index names must be lowercase");
    }
    if let Some(c) = name.chars().find(|c| illegal.contains(c)) {
        bail!("index names must not contain '{c}'");
    }
    if name.len() > 255 {
        bail!(
            "index names are limited to 255 bytes ({} given)",
            name.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(p: &[&str], x: &[&str], pre: &str, suf: &str) -> IndexRenamer {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        IndexRenamer::new(&s(p), &s(x), pre, suf).unwrap()
    }

    #[test]
    fn prefix_rule_first_match_wins_then_prefix_suffix() {
        let n = r(
            &["logstash-:logstash-srctag-", "logstash-:never-"],
            &[],
            "",
            "",
        );
        assert_eq!(
            n.index("logstash-2026-01-01").unwrap(),
            "logstash-srctag-2026-01-01"
        );
        assert_eq!(n.index("other").unwrap(), "other");
        let n = r(&["logstash-:ls-"], &[], "p_", "_s");
        assert_eq!(n.index("logstash-x").unwrap(), "p_ls-x_s");
        assert_eq!(n.index("y").unwrap(), "p_y_s");
    }

    #[test]
    fn regex_rule_with_captures() {
        let n = r(&[], &["^logstash-(.*)$:logstash-srctag-$1"], "", "");
        assert_eq!(
            n.index("logstash-2026.01.01").unwrap(),
            "logstash-srctag-2026.01.01"
        );
        assert_eq!(n.index("metrics-1").unwrap(), "metrics-1");
        // colons inside the regex are fine: the last colon splits
        let n = r(&[], &["^(?:a|b)-(\\d+)$:c-${1}"], "", "");
        assert_eq!(n.index("a-12").unwrap(), "c-12");
        // prefix rules are tried before regex rules
        let n = r(&["a-:x-"], &["^a-(.*)$:y-$1"], "", "");
        assert_eq!(n.index("a-1").unwrap(), "x-1");
    }

    #[test]
    fn invalid_results_and_specs_fail_clearly() {
        let n = r(&["logs-:Logs-"], &[], "", "");
        let e = n.index("logs-1").unwrap_err().to_string();
        assert!(e.contains("lowercase") && e.contains("logs-1"), "{e}");
        let n = r(&[], &["^(.*)$:_$1"], "", "");
        assert!(n.index("a").unwrap_err().to_string().contains("start with"));
        let n = r(&["a:b c"], &[], "", "");
        assert!(n.index("a1").is_err());
        let long = format!("a:{}", "x".repeat(300));
        assert!(r(&[long.as_str()], &[], "", "").index("a").is_err());
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(IndexRenamer::new(&s(&["nocolon"]), &[], "", "").is_err());
        assert!(IndexRenamer::new(&s(&[":x"]), &[], "", "").is_err());
        assert!(IndexRenamer::new(&[], &s(&["(?=x):y"]), "", "").is_err());
    }

    #[test]
    fn aliases_and_patterns() {
        let n = r(&["logstash-:logstash-srctag-"], &[], "p_", "");
        assert_eq!(n.alias("logstash-all").unwrap(), "logstash-srctag-all");
        assert_eq!(n.pattern("logstash-*").unwrap(), "logstash-srctag-*");
        assert_eq!(n.pattern("other-*").unwrap(), "other-*");
    }
}
