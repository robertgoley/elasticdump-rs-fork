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

/// `spec` for an error message: long patterns (a 1 MB regex) are cut.
fn shorten(spec: &str) -> String {
    const MAX: usize = 80;
    if spec.chars().count() <= MAX {
        return spec.to_string();
    }
    let head: String = spec.chars().take(MAX).collect();
    format!("{head}... ({} bytes)", spec.len())
}

/// Compile a user-supplied regex (`--renameIndex`, `--templateRegex`) with
/// regex's default size limit (TST-BRK-CLI-004). `flag` names the option in
/// the error; `invalid` describes a syntax error. Exceeding the size limit
/// gets a hint instead of the bare "compiled regex exceeds size limit".
pub fn compile_regex(pattern: &str, flag: &str, invalid: &str) -> Result<Regex> {
    Regex::new(pattern).map_err(|e| match e {
        regex::Error::CompiledTooBig(limit) => anyhow!(
            "{flag}: pattern too large (regex size limit); simplify it (the compiled regex would exceed {limit} bytes)"
        ),
        regex::Error::Syntax(msg) if pattern.len() > 200 => {
            // regex's message repeats the whole pattern; keep only its last line
            let last = msg.lines().last().unwrap_or_default().trim();
            anyhow!("{flag}: {invalid}: {last}")
        }
        e => anyhow!("{flag}: {invalid}: {e}"),
    })
}

/// A `$N` / `$name` / `${N}` / `${name}` reference in a replacement (the
/// syntax of `regex::Regex::replacen`, including `$$` for a literal `$`).
fn replacement_refs(rep: &str) -> Vec<&str> {
    let b = rep.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'$' {
            i += 1;
            continue;
        }
        match b.get(i + 1) {
            Some(b'$') => i += 2,
            Some(b'{') => match rep[i + 2..].find('}') {
                Some(end) => {
                    out.push(&rep[i + 2..i + 2 + end]);
                    i += 3 + end;
                }
                None => i += 1,
            },
            _ => {
                let len = b[i + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_')
                    .count();
                if len > 0 {
                    out.push(&rep[i + 1..i + 1 + len]);
                }
                i += 1 + len;
            }
        }
    }
    out
}

/// Every capture reference in `rep` must exist in `re`: regex silently
/// expands an unknown group (`$9` with one group, a `${nmae}` typo, or `$1a`,
/// which is the group *named* `1a`) to nothing.
fn check_replacement(re: &Regex, rep: &str, flag: &str) -> Result<()> {
    for r in replacement_refs(rep) {
        let known = match r.parse::<usize>() {
            Ok(n) => n < re.captures_len(),
            Err(_) => re.capture_names().flatten().any(|n| n == r),
        };
        if !known {
            let groups = re.captures_len() - 1;
            let mut names: Vec<&str> = re.capture_names().flatten().collect();
            names.sort_unstable();
            let named = if names.is_empty() {
                String::new()
            } else {
                format!(" (named: {})", names.join(", "))
            };
            // `$1x` reads as the group named `1x`: suggest `${1}x`
            let digits = r.bytes().take_while(u8::is_ascii_digit).count();
            let hint = if digits > 0 && digits < r.len() {
                format!(
                    ". Write ${{{}}}{} for group {} followed by '{}'",
                    &r[..digits],
                    &r[digits..],
                    &r[..digits],
                    &r[digits..]
                )
            } else {
                String::new()
            };
            bail!(
                "{flag}: the replacement refers to capture group '{r}', which the regex does not have; it has {groups} group(s){named}{hint} (use $$ for a literal $)"
            );
        }
    }
    Ok(())
}

/// A rename that would merge several source indices into one target
/// (TST-BRK-CLI-003). Fatal: it never becomes a per-document rejection.
#[derive(Debug)]
pub struct MergeRefused(pub String);

impl std::fmt::Display for MergeRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MergeRefused {}

#[derive(Debug, Default)]
pub struct IndexRenamer {
    rules: Vec<Rule>,
    pub prefix: String,
    pub suffix: String,
    cache: Mutex<HashMap<String, String>>,
    /// `--allowMerge`: several source indices may be renamed to one target.
    allow_merge: bool,
    /// Target name → the first source index renamed to it.
    targets: Mutex<HashMap<String, String>>,
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
            let flag = format!("--renameIndex '{}'", shorten(spec));
            let re = compile_regex(
                &re,
                &flag,
                "invalid regex (look-around and backreferences are not supported)",
            )?;
            check_replacement(&re, &rep, &flag)?;
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
            allow_merge: false,
            targets: Mutex::new(HashMap::new()),
        })
    }

    /// `--allowMerge`: accept several source indices renamed to one target.
    pub fn with_allow_merge(mut self, allow: bool) -> Self {
        self.allow_merge = allow;
        self
    }

    /// Target name of `name` before validation (rules, then prefix/suffix).
    fn target_of(&self, name: &str) -> String {
        let base = self
            .rename_raw(name)
            .map_or_else(|| name.to_string(), |(n, _)| n);
        format!("{}{}{}", self.prefix, base, self.suffix)
    }

    fn merge_error(&self, target: &str, sources: &[&str]) -> anyhow::Error {
        anyhow::Error::new(MergeRefused(format!(
            "renaming would merge source indices {} into one index '{target}' (documents with the same _id would overwrite each other); fix the --renamePrefix/--renameIndex rules, or pass --allowMerge to merge them on purpose",
            sources
                .iter()
                .map(|s| format!("'{s}'"))
                .collect::<Vec<_>>()
                .join(" and ")
        )))
    }

    /// Refuse a many-to-one renaming among `names` (the source indices known
    /// before anything is written) unless `--allowMerge` was given.
    pub fn check_merges<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Result<()> {
        if self.allow_merge || self.rules.is_empty() {
            return Ok(());
        }
        let mut by_target: std::collections::BTreeMap<String, Vec<&str>> = Default::default();
        for n in names {
            by_target.entry(self.target_of(n)).or_default().push(n);
        }
        match by_target.iter().find(|(_, v)| v.len() > 1) {
            Some((t, v)) => Err(self.merge_error(t, v)),
            None => Ok(()),
        }
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
        if !self.allow_merge && !self.rules.is_empty() {
            let mut targets = self.targets.lock().unwrap();
            match targets.get(&out) {
                Some(first) if first != name => {
                    return Err(self.merge_error(&out, &[first, name]));
                }
                Some(_) => {}
                None => {
                    targets.insert(out.clone(), name.to_string());
                }
            }
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
    // Elasticsearch/OpenSearch: `!name.toLowerCase(Locale.ROOT).equals(name)`,
    // which also refuses titlecase letters such as 'ǅ' (found by L-504).
    if name.to_lowercase() != name {
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
    fn titlecase_letters_are_not_lowercase() {
        assert!(validate_index_name("aǅ").is_err());
        assert!(validate_index_name("aǆ").is_ok());
        assert!(validate_index_name("éß😀").is_ok());
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

    /// L-643 (TST-BRK-CLI-004): oversized patterns and references to missing groups.
    #[test]
    fn regex_abuse_is_refused_with_a_clear_message() {
        let new = |x: &str| IndexRenamer::new(&[], &[x.to_string()], "", "").map(|_| ());
        let e = new("(a{1000}){1000}:x").unwrap_err().to_string();
        assert!(
            e.contains("pattern too large (regex size limit); simplify it"),
            "{e}"
        );
        let mb = format!("{}:x", "a".repeat(1 << 20));
        let e = new(&mb).unwrap_err().to_string();
        assert!(
            e.contains("regex size limit") && e.contains("1048578 bytes"),
            "{e}"
        );
        assert!(
            e.len() < 400,
            "the pattern is not echoed: {} bytes",
            e.len()
        );
        let e = new(r"(?-u:\xff):x").unwrap_err().to_string();
        assert!(e.contains("invalid UTF-8"), "{e}");
        for (spec, group) in [
            ("^(.*)$:$9", "'9'"),
            ("^(.*)$:${2}", "'2'"),
            ("^(?P<name>.*)$:${nmae}", "'nmae'"),
            ("^(?P<name>.*)$:$nam", "'nam'"),
            ("^(.*)$:$1x", "'1x'"),
            ("^a$:$0$1", "'1'"),
        ] {
            let e = new(spec).unwrap_err().to_string();
            assert!(e.contains(&format!("capture group {group}")), "{spec}: {e}");
            assert!(e.starts_with("--renameIndex '"), "{spec}: {e}");
        }
        // valid references, `$$` and a lone `$` are accepted and map as before
        let n = r(
            &[],
            &[
                "^(?P<name>[a-z]+)-(\\d+)$:${name}_$2-${1}-$$-$0$",
                "^q(.*)$:r$1",
            ],
            "",
            "",
        );
        assert_eq!(n.index("ab-12").unwrap(), "ab_12-ab-$-ab-12$");
        assert_eq!(n.index("qz").unwrap(), "rz");
        let n = r(&[], &["^x(.*)$:y$1-$ {z"], "", "");
        assert!(n.index("xa").is_err(), "space is not a valid index char");
        assert_eq!(replacement_refs("a$$b${x}c$_1-$-${"), vec!["x", "_1"]);
    }

    #[test]
    fn many_to_one_renames_are_refused_unless_merging_is_allowed() {
        let r = IndexRenamer::new(&[], &["^(a|b)-(.*):x-$2".to_string()], "", "").unwrap();
        let e = r.check_merges(["a-1", "b-1", "c"]).unwrap_err().to_string();
        assert!(e.contains("'a-1' and 'b-1'") && e.contains("'x-1'"), "{e}");
        assert!(e.contains("--allowMerge"), "{e}");
        r.check_merges(["a-1", "b-2", "x-3"]).unwrap();
        // at write time: the second source mapping to a used target fails
        assert_eq!(r.index("a-1").unwrap(), "x-1");
        assert_eq!(r.index("a-1").unwrap(), "x-1");
        let e = r.index("b-1").unwrap_err().to_string();
        assert!(e.contains("'a-1' and 'b-1'"), "{e}");
        // a source keeping its own name that equals another's target also collides
        let e = r.index("x-1").unwrap_err().to_string();
        assert!(e.contains("into one index 'x-1'"), "{e}");
        // --allowMerge
        let r = IndexRenamer::new(&[], &["^(a|b)-(.*):x-$2".to_string()], "", "")
            .unwrap()
            .with_allow_merge(true);
        r.check_merges(["a-1", "b-1"]).unwrap();
        assert_eq!(r.index("a-1").unwrap(), "x-1");
        assert_eq!(r.index("b-1").unwrap(), "x-1");
        // prefix/suffix only (no rules) never merges
        let r = IndexRenamer::new(&[], &[], "p-", "").unwrap();
        r.check_merges(["a", "b"]).unwrap();
    }

    #[test]
    fn aliases_and_patterns() {
        let n = r(&["logstash-:logstash-srctag-"], &[], "p_", "");
        assert_eq!(n.alias("logstash-all").unwrap(), "logstash-srctag-all");
        assert_eq!(n.pattern("logstash-*").unwrap(), "logstash-srctag-*");
        assert_eq!(n.pattern("other-*").unwrap(), "other-*");
    }
}

/// L-504 (TST-API-083): rename rules against an independent model.
#[cfg(test)]
mod props {
    use super::*;
    use proptest::prelude::*;

    fn cases() -> u32 {
        std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10_000)
    }

    #[derive(Debug, Clone)]
    enum M {
        Prefix(String, String),
        /// `^<lit>(.*)$` -> `<new>$1`
        Anchored(String, String),
        /// `<lit>` -> `<new>` (first occurrence)
        Literal(String, String),
    }

    fn lit() -> impl Strategy<Value = String> {
        "[a-c.-]{1,3}"
    }

    fn rule() -> impl Strategy<Value = M> {
        prop_oneof![
            (lit(), "[a-z]{0,3}-?").prop_map(|(a, b)| M::Prefix(a, b)),
            (lit(), "[a-z]{0,3}-?").prop_map(|(a, b)| M::Anchored(a, b)),
            (lit(), "[x-z]{0,2}").prop_map(|(a, b)| M::Literal(a, b)),
        ]
    }

    /// The model: prefix rules first, then regex rules, each in order; the
    /// first that matches wins; then --prefix/--suffix.
    fn model(rules: &[M], name: &str, pre: &str, suf: &str) -> String {
        let prefixes = rules.iter().filter(|r| matches!(r, M::Prefix(..)));
        let regexes = rules.iter().filter(|r| !matches!(r, M::Prefix(..)));
        let hit = prefixes.chain(regexes).find_map(|r| match r {
            M::Prefix(a, b) | M::Anchored(a, b) => name
                .strip_prefix(a.as_str())
                .map(|rest| format!("{b}{rest}")),
            M::Literal(a, b) => name
                .find(a.as_str())
                .map(|i| format!("{}{b}{}", &name[..i], &name[i + a.len()..])),
        });
        format!("{pre}{}{suf}", hit.unwrap_or_else(|| name.to_string()))
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: cases(), ..ProptestConfig::default() })]

        #[test]
        fn renames_agree_with_the_model(
            rules in proptest::collection::vec(rule(), 0..5),
            names in proptest::collection::vec("[a-c.-]{1,8}", 1..6),
            pre in prop_oneof![Just(""), Just("p-")],
            suf in prop_oneof![Just(""), Just("-s")],
        ) {
            let mut prefix_specs = Vec::new();
            let mut regex_specs = Vec::new();
            for r in &rules {
                match r {
                    M::Prefix(a, b) => prefix_specs.push(format!("{a}:{b}")),
                    M::Anchored(a, b) => regex_specs.push(format!("^{}(.*)$:{b}${{1}}", regex::escape(a))),
                    M::Literal(a, b) => regex_specs.push(format!("{}:{b}", regex::escape(a))),
                }
            }
            let r = IndexRenamer::new(&prefix_specs, &regex_specs, pre, suf)
                .unwrap()
                .with_allow_merge(true);
            for n in &names {
                let want = model(&rules, n, pre, suf);
                match r.index(n) {
                    Ok(got) => prop_assert_eq!(&got, &want),
                    Err(e) => prop_assert!(
                        validate_index_name(&want).is_err(),
                        "{n}: refused ({e}) but the model gives the valid '{want}'"
                    ),
                }
                if validate_index_name(&want).is_err() {
                    prop_assert!(r.index(n).is_err(), "{n} -> invalid '{want}' accepted");
                }
            }
        }
    }
}
