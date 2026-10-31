//! Optional ILM → ISM translation (`--convertIlmToIsm`).
//!
//! ILM is a fixed phase pipeline (hot → warm → cold → frozen → delete, each
//! with `min_age` and a fixed set of actions); ISM is a state machine. The
//! common subset maps one ISM state per ILM phase with an age transition to
//! the next one. Everything outside that subset refuses the whole policy
//! (naming the phase and action) unless `--lenientPolicyConversion` is
//! given, in which case it is dropped with a warning. Every approximation
//! is reported.

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

const PHASES: &[&str] = &["hot", "warm", "cold", "frozen", "delete"];

/// ILM's execution order of actions within each phase.
fn action_order(phase: &str) -> &'static [&'static str] {
    match phase {
        "hot" => &[
            "set_priority",
            "unfollow",
            "rollover",
            "readonly",
            "downsample",
            "shrink",
            "forcemerge",
            "searchable_snapshot",
        ],
        "warm" => &[
            "set_priority",
            "unfollow",
            "readonly",
            "downsample",
            "allocate",
            "migrate",
            "shrink",
            "forcemerge",
        ],
        "cold" => &[
            "set_priority",
            "unfollow",
            "readonly",
            "downsample",
            "searchable_snapshot",
            "allocate",
            "migrate",
            "freeze",
        ],
        "frozen" => &["unfollow", "searchable_snapshot"],
        "delete" => &["wait_for_snapshot", "delete"],
        _ => &[],
    }
}

/// Result of translating one policy.
#[derive(Debug)]
pub struct Converted {
    /// ISM policy object (the value of `"policy"` in a PUT body).
    pub policy: Value,
    /// Approximations and (lenient mode) dropped parts.
    pub notes: Vec<String>,
}

/// What the target ISM supports, where it differs between OpenSearch versions.
#[derive(Debug, Clone, Copy)]
pub struct IsmCaps {
    /// `shrink` (OpenSearch >= 2.0).
    pub shrink: bool,
}

impl Default for IsmCaps {
    fn default() -> Self {
        Self { shrink: true }
    }
}

struct Ctx {
    lenient: bool,
    caps: IsmCaps,
    notes: Vec<String>,
    refused: Vec<String>,
    /// Refused in both modes: dropping them would change what the policy deletes or when.
    fatal: Vec<String>,
}

impl Ctx {
    fn problem(&mut self, msg: String) {
        if self.lenient {
            self.notes.push(format!("DROPPED {msg}"));
        } else {
            self.refused.push(msg);
        }
    }
}

fn is_zero_age(age: &str) -> bool {
    let digits: String = age
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.parse::<f64>().is_ok_and(|n| n == 0.0)
}

/// A time value both ILM and ISM take for an age: whole number and unit (`30d`, `12h`,
/// `0ms`) or `0`. Fractions (`1.5h`) are refused by OpenSearch ("fractional time values are
/// not supported", found live by L-651) and by Elasticsearch.
fn is_duration(age: &str) -> bool {
    if age == "0" {
        return true;
    }
    let unit_at = age.find(|c: char| !c.is_ascii_digit()).unwrap_or(age.len());
    let (num, unit) = age.split_at(unit_at);
    !num.is_empty() && ["nanos", "micros", "ms", "s", "m", "h", "d"].contains(&unit)
}

/// Translate the ILM policy object (`{"phases":{..},"_meta":{..}}`).
pub fn convert(name: &str, ilm: &Value, lenient: bool) -> Result<Converted> {
    convert_for(name, ilm, lenient, IsmCaps::default())
}

/// [`convert`] for a target with the given ISM capabilities.
pub fn convert_for(name: &str, ilm: &Value, lenient: bool, caps: IsmCaps) -> Result<Converted> {
    let mut cx = Ctx {
        lenient,
        caps,
        notes: Vec::new(),
        refused: Vec::new(),
        fatal: Vec::new(),
    };
    let Some(phases) = ilm.get("phases").and_then(Value::as_object) else {
        bail!("ILM policy '{name}' has no phases");
    };
    for p in phases.keys() {
        if !PHASES.contains(&p.as_str()) {
            cx.problem(format!("unknown phase '{p}'"));
        }
    }
    let mut present: Vec<(&str, &Value)> = Vec::new();
    for p in PHASES {
        if let Some(def) = phases.get(*p) {
            // L-651: a min_age that is not a string used to count as 0 (the next phase,
            // e.g. delete, at once); refused in both modes
            if let Some(age) = def
                .get("min_age")
                .filter(|a| !a.as_str().is_some_and(is_duration))
            {
                cx.fatal.push(format!(
                    "phase '{p}' min_age {age}: not a whole-number duration (e.g. 7d, 12h, 0ms)"
                ));
            }
            if *p == "frozen" {
                cx.problem(
                    "phase 'frozen': it only holds searchable snapshots, which OpenSearch ISM cannot express"
                        .into(),
                );
                continue;
            }
            if let Some(o) = def.as_object() {
                for k in o.keys().filter(|k| *k != "min_age" && *k != "actions") {
                    cx.problem(format!("phase '{p}' key '{k}': not an ILM phase setting"));
                }
            }
            present.push((p, def));
        }
    }

    if !cx.fatal.is_empty() {
        bail!(
            "ILM policy '{name}' cannot be converted to ISM: {} (not dropped by --lenientPolicyConversion; fix the policy)",
            cx.fatal.join("; ")
        );
    }

    let mut states: Vec<Value> = Vec::new();
    let mut rolled_over = false;
    // a non-zero min_age on the first phase becomes a waiting state
    if let Some((first, def)) = present.first() {
        if let Some(age) = def.get("min_age").and_then(Value::as_str) {
            if !is_zero_age(age) {
                states.push(json!({
                    "name": "pending",
                    "actions": [],
                    "transitions": [{"state_name": first, "conditions": {"min_index_age": age}}]
                }));
                cx.notes.push(format!(
                    "phase '{first}' min_age {age} became a 'pending' state with a min_index_age transition"
                ));
            }
        }
    }
    for (i, (phase, def)) in present.iter().enumerate() {
        let empty = Map::new();
        let actions_in = def
            .get("actions")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let order = action_order(phase);
        for a in actions_in.keys() {
            if !order.contains(&a.as_str()) {
                cx.problem(format!(
                    "phase '{phase}' action '{a}': not an ILM action of this phase"
                ));
            }
        }
        let mut actions = Vec::new();
        for a in order {
            let Some(cfg) = actions_in.get(*a) else {
                continue;
            };
            convert_action(&mut cx, phase, a, cfg, &mut actions, &mut rolled_over);
        }
        if (*phase == "warm" || *phase == "cold") && !actions_in.contains_key("migrate") {
            cx.notes.push(format!(
                "phase '{phase}': ILM's implicit data-tier migration is dropped (OpenSearch has no data tiers)"
            ));
        }
        let mut state = json!({"name": phase, "actions": actions, "transitions": []});
        if let Some((next, next_def)) = present.get(i + 1) {
            let age = next_def
                .get("min_age")
                .and_then(Value::as_str)
                .unwrap_or("0ms");
            let t = if is_zero_age(age) {
                json!({"state_name": next})
            } else if rolled_over {
                cx.notes.push(format!(
                    "phase '{next}' min_age {age} counts from rollover in ILM: mapped to min_rollover_age"
                ));
                json!({"state_name": next, "conditions": {"min_rollover_age": age}})
            } else {
                json!({"state_name": next, "conditions": {"min_index_age": age}})
            };
            state["transitions"] = json!([t]);
        }
        states.push(state);
    }
    if states.is_empty() {
        // Not droppable: an ISM policy needs at least one state (L-507: lenient
        // mode used to panic here on `{"phases":{}}` or a frozen-only policy).
        cx.refused
            .push("the policy has no convertible phases".into());
        if cx.lenient {
            bail!(
                "ILM policy '{name}' cannot be converted to ISM: {}",
                cx.refused.join("; ")
            );
        }
    }

    let mut description =
        format!("Converted from Elasticsearch ILM policy '{name}' by elasticdump-rs");
    if let Some(meta) = ilm.get("_meta").and_then(Value::as_object) {
        if let Some(d) = meta.get("description").and_then(Value::as_str) {
            description = d.to_string();
        }
        let other: Vec<&String> = meta.keys().filter(|k| *k != "description").collect();
        if !other.is_empty() {
            cx.notes.push(format!(
                "_meta keys dropped (ISM has no _meta): {}",
                other
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    if ilm.get("deprecated").is_some() {
        cx.notes.push("'deprecated' flag dropped".into());
    }
    if !cx.refused.is_empty() {
        bail!(
            "ILM policy '{name}' cannot be converted to ISM safely: {} (--lenientPolicyConversion drops these parts instead)",
            cx.refused.join("; ")
        );
    }
    let default_state = states[0]["name"].clone();
    Ok(Converted {
        policy: json!({
            "description": description,
            "default_state": default_state,
            "states": states,
        }),
        notes: cx.notes,
    })
}

fn convert_action(
    cx: &mut Ctx,
    phase: &str,
    action: &str,
    cfg: &Value,
    out: &mut Vec<Value>,
    rolled_over: &mut bool,
) {
    let empty = Map::new();
    let c = cfg.as_object().unwrap_or(&empty);
    let unknown = |cx: &mut Ctx, allowed: &[&str]| {
        for k in c.keys() {
            if !allowed.contains(&k.as_str()) {
                cx.problem(format!(
                    "phase '{phase}' action '{action}' option '{k}': no ISM equivalent"
                ));
            }
        }
    };
    match action {
        "set_priority" => {
            unknown(cx, &["priority"]);
            match c.get("priority") {
                Some(p) if !p.is_null() => out.push(json!({"index_priority": {"priority": p}})),
                _ => cx
                    .notes
                    .push(format!("phase '{phase}': set_priority null (reset) dropped")),
            }
        }
        "rollover" => {
            let map = [
                ("max_age", "min_index_age"),
                ("max_docs", "min_doc_count"),
                ("max_size", "min_size"),
                ("max_primary_shard_size", "min_primary_shard_size"),
            ];
            unknown(cx, &map.map(|(k, _)| k));
            let mut r = Map::new();
            for (ilm, ism) in map {
                if let Some(v) = c.get(ilm) {
                    r.insert(ism.into(), v.clone());
                }
            }
            if r.is_empty() {
                cx.problem(format!(
                    "phase '{phase}' action 'rollover': no convertible condition"
                ));
                return;
            }
            out.push(json!({ "rollover": r }));
            *rolled_over = true;
            cx.notes.push(
                "rollover: ISM needs index.plugins.index_state_management.rollover_alias on managed indices (ILM used index.lifecycle.rollover_alias or a data stream); index settings are not rewritten"
                    .into(),
            );
        }
        "allocate" => {
            unknown(cx, &["number_of_replicas", "include", "exclude", "require"]);
            if let Some(n) = c.get("number_of_replicas") {
                out.push(json!({"replica_count": {"number_of_replicas": n}}));
            }
            let mut alloc = Map::new();
            for k in ["require", "include", "exclude"] {
                if let Some(v) = c
                    .get(k)
                    .filter(|v| v.as_object().is_some_and(|o| !o.is_empty()))
                {
                    alloc.insert(k.into(), v.clone());
                }
            }
            if !alloc.is_empty() {
                alloc.insert("wait_for".into(), json!(false));
                out.push(json!({ "allocation": alloc }));
            }
        }
        "forcemerge" => {
            unknown(cx, &["max_num_segments"]);
            match c.get("max_num_segments") {
                Some(n) => out.push(json!({"force_merge": {"max_num_segments": n}})),
                None => cx.problem(format!(
                    "phase '{phase}' action 'forcemerge': no max_num_segments"
                )),
            }
        }
        "shrink" => {
            unknown(
                cx,
                &[
                    "number_of_shards",
                    "max_primary_shard_size",
                    "allow_write_after_shrink",
                ],
            );
            if !cx.caps.shrink {
                cx.problem(format!(
                    "phase '{phase}' action 'shrink': the target OpenSearch has no ISM shrink action (added in 2.0)"
                ));
                return;
            }
            if c.get("allow_write_after_shrink").and_then(Value::as_bool) == Some(true) {
                cx.problem(format!(
                    "phase '{phase}' action 'shrink' allow_write_after_shrink: no ISM equivalent"
                ));
            }
            if let Some(n) = c.get("number_of_shards") {
                out.push(json!({"shrink": {"num_new_shards": n}}));
            } else if let Some(s) = c.get("max_primary_shard_size") {
                out.push(json!({"shrink": {"max_shard_size": s}}));
            } else {
                cx.problem(format!("phase '{phase}' action 'shrink': no target size"));
            }
        }
        "readonly" => {
            unknown(cx, &[]);
            out.push(json!({"read_only": {}}))
        }
        "delete" => {
            unknown(cx, &["delete_searchable_snapshot"]);
            if c.get("delete_searchable_snapshot").is_some() {
                cx.notes.push("delete: delete_searchable_snapshot ignored (no searchable snapshots on OpenSearch)".into());
            }
            out.push(json!({"delete": {}}));
        }
        "migrate" => {
            unknown(cx, &["enabled"]);
            cx.notes.push(format!(
                "phase '{phase}': migrate dropped (OpenSearch has no data tiers)"
            ))
        }
        "wait_for_snapshot" => cx.problem(format!(
            "phase '{phase}' action 'wait_for_snapshot': no ISM equivalent (without it the index is deleted without waiting for snapshot policy {})",
            c.get("policy").map_or_else(|| "?".into(), Value::to_string)
        )),
        "searchable_snapshot" | "downsample" | "unfollow" | "freeze" => {
            cx.problem(format!(
                "phase '{phase}' action '{action}': no ISM equivalent"
            ));
        }
        other => cx.problem(format!("phase '{phase}' action '{other}': unknown action")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrink_is_refused_for_targets_without_ism_shrink() {
        let ilm = json!({"phases": {"hot": {"actions": {"shrink": {"number_of_shards": 1}, "readonly": {}}}}});
        let old = IsmCaps { shrink: false };
        let e = convert_for("p", &ilm, false, old).unwrap_err().to_string();
        assert!(
            e.contains("phase 'hot' action 'shrink'") && e.contains("2.0"),
            "{e}"
        );
        let c = convert_for("p", &ilm, true, old).unwrap();
        assert_eq!(c.policy["states"][0]["actions"], json!([{"read_only": {}}]));
        assert!(convert("p", &ilm, false).is_ok());
    }

    #[test]
    fn policies_without_a_convertible_phase_are_refused_in_both_modes() {
        for ilm in [
            json!({"phases": {}}),
            json!({"phases": {"frozen": {"actions": {"searchable_snapshot": {"snapshot_repository": "r"}}}}}),
            json!({"phases": {"bogus": {"actions": {}}}}),
        ] {
            for lenient in [false, true] {
                let e = convert("p", &ilm, lenient).unwrap_err();
                assert!(format!("{e:#}").contains("no convertible phases"), "{e:#}");
            }
        }
    }

    #[test]
    fn hot_warm_delete_with_rollover() {
        let ilm = json!({
            "_meta": {"description": "logs", "owner": "me"},
            "phases": {
                "hot": {"min_age": "0ms", "actions": {"rollover": {"max_age": "1d", "max_primary_shard_size": "50gb"}, "set_priority": {"priority": 100}}},
                "warm": {"min_age": "7d", "actions": {"forcemerge": {"max_num_segments": 1}, "allocate": {"number_of_replicas": 0, "require": {"box": "warm"}}, "shrink": {"number_of_shards": 1}, "readonly": {}}},
                "delete": {"min_age": "30d", "actions": {"delete": {"delete_searchable_snapshot": true}}}
            }
        });
        let c = convert("p", &ilm, false).unwrap();
        assert_eq!(
            c.policy,
            json!({
                "description": "logs",
                "default_state": "hot",
                "states": [
                    {"name": "hot", "actions": [
                        {"index_priority": {"priority": 100}},
                        {"rollover": {"min_index_age": "1d", "min_primary_shard_size": "50gb"}}
                    ], "transitions": [{"state_name": "warm", "conditions": {"min_rollover_age": "7d"}}]},
                    {"name": "warm", "actions": [
                        {"read_only": {}},
                        {"replica_count": {"number_of_replicas": 0}},
                        {"allocation": {"require": {"box": "warm"}, "wait_for": false}},
                        {"shrink": {"num_new_shards": 1}},
                        {"force_merge": {"max_num_segments": 1}}
                    ], "transitions": [{"state_name": "delete", "conditions": {"min_rollover_age": "30d"}}]},
                    {"name": "delete", "actions": [{"delete": {}}], "transitions": []}
                ]
            })
        );
        let notes = c.notes.join("\n");
        for n in [
            "rollover_alias",
            "data-tier",
            "_meta keys dropped",
            "owner",
            "min_rollover_age",
            "delete_searchable_snapshot",
        ] {
            assert!(notes.contains(n), "{n}: {notes}");
        }
    }

    #[test]
    fn without_rollover_uses_index_age_and_pending_state() {
        let ilm = json!({"phases": {
            "warm": {"min_age": "2d", "actions": {"readonly": {}, "migrate": {"enabled": false}}},
            "delete": {"min_age": "10d", "actions": {"delete": {}}}
        }});
        let c = convert("p", &ilm, false).unwrap();
        assert_eq!(c.policy["default_state"], "pending");
        assert_eq!(
            c.policy["states"][0],
            json!({"name": "pending", "actions": [], "transitions": [{"state_name": "warm", "conditions": {"min_index_age": "2d"}}]})
        );
        assert_eq!(
            c.policy["states"][1]["transitions"],
            json!([{"state_name": "delete", "conditions": {"min_index_age": "10d"}}])
        );
        assert!(c.notes.iter().any(|n| n.contains("migrate dropped")));
        assert!(c.policy["description"].as_str().unwrap().contains("'p'"));
    }

    #[test]
    fn refusals_name_phase_and_action_and_lenient_drops() {
        let cases = [
            (
                json!({"phases": {"cold": {"actions": {"searchable_snapshot": {"snapshot_repository": "r"}}}}}),
                "phase 'cold' action 'searchable_snapshot'",
            ),
            (
                json!({"phases": {"frozen": {"actions": {}}}}),
                "phase 'frozen'",
            ),
            (
                json!({"phases": {"hot": {"actions": {"rollover": {"max_primary_shard_docs": 1}}}}}),
                "max_primary_shard_docs",
            ),
            (
                json!({"phases": {"hot": {"actions": {"rollover": {"max_age": "1d", "min_docs": 1}}}}}),
                "min_docs",
            ),
            (
                json!({"phases": {"warm": {"actions": {"allocate": {"total_shards_per_node": 1}}}}}),
                "total_shards_per_node",
            ),
            (
                json!({"phases": {"warm": {"actions": {"forcemerge": {"max_num_segments": 1, "index_codec": "best_compression"}}}}}),
                "index_codec",
            ),
            (
                json!({"phases": {"hot": {"actions": {"shrink": {"number_of_shards": 1, "allow_write_after_shrink": true}}}}}),
                "allow_write_after_shrink",
            ),
            (
                json!({"phases": {"hot": {"actions": {"downsample": {"fixed_interval": "1h"}}}}}),
                "downsample",
            ),
            (
                json!({"phases": {"delete": {"actions": {"wait_for_snapshot": {"policy": "s"}, "delete": {}}}}}),
                "wait_for_snapshot",
            ),
            (
                json!({"phases": {"hot": {"actions": {"unfollow": {}}}}}),
                "unfollow",
            ),
            (
                json!({"phases": {"cold": {"actions": {"freeze": {}}}}}),
                "freeze",
            ),
        ];
        for (ilm, needle) in cases {
            let e = convert("bad", &ilm, false).unwrap_err().to_string();
            assert!(
                e.contains(needle)
                    && e.contains("'bad'")
                    && e.contains("--lenientPolicyConversion"),
                "{needle}: {e}"
            );
        }
        let ilm = json!({"phases": {
            "hot": {"actions": {"rollover": {"max_age": "1d"}}},
            "cold": {"min_age": "5d", "actions": {"searchable_snapshot": {"snapshot_repository": "r"}, "readonly": {}}},
            "frozen": {"min_age": "9d", "actions": {}}
        }});
        let c = convert("p", &ilm, true).unwrap();
        assert_eq!(c.policy["states"].as_array().unwrap().len(), 2);
        assert_eq!(c.policy["states"][1]["actions"], json!([{"read_only": {}}]));
        assert!(c.notes.iter().filter(|n| n.starts_with("DROPPED")).count() >= 2);
    }

    #[test]
    fn unsafe_min_ages_and_unknown_options_are_refused() {
        // a min_age that is not a duration is never dropped: as 0 it would delete at once
        for age in [
            json!(30),
            json!("soon"),
            json!("7 d"),
            json!(".5d"),
            json!(null),
        ] {
            let ilm = json!({"phases": {"hot": {"actions": {}}, "delete": {"min_age": age, "actions": {"delete": {}}}}});
            for lenient in [false, true] {
                let e = format!("{:#}", convert("p", &ilm, lenient).unwrap_err());
                assert!(
                    e.contains("phase 'delete' min_age") && e.contains("not dropped"),
                    "{age}: {e}"
                );
            }
        }
        for age in ["0", "0ms", "30d", "12h", "100micros"] {
            let ilm = json!({"phases": {"delete": {"min_age": age, "actions": {"delete": {}}}}});
            assert!(convert("p", &ilm, false).is_ok(), "{age}");
        }
        // options the converter does not know, on actions it does, and phase keys
        for (ilm, needle) in [
            (
                json!({"phases": {"hot": {"actions": {"set_priority": {"priority": 1, "future_option": 1}}}}}),
                "action 'set_priority' option 'future_option'",
            ),
            (
                json!({"phases": {"hot": {"actions": {"readonly": {"future_option": 1}}}}}),
                "action 'readonly' option 'future_option'",
            ),
            (
                json!({"phases": {"delete": {"actions": {"delete": {"future_option": 1}}}}}),
                "action 'delete' option 'future_option'",
            ),
            (
                json!({"phases": {"warm": {"actions": {"migrate": {"enabled": false, "future_option": 1}}}}}),
                "action 'migrate' option 'future_option'",
            ),
            (
                json!({"phases": {"warm": {"actions": {"forcemerge": {}}}}}),
                "action 'forcemerge': no max_num_segments",
            ),
            (
                json!({"phases": {"hot": {"future_key": 1, "actions": {}}}}),
                "phase 'hot' key 'future_key'",
            ),
            (
                json!({"phases": {"hot": {"actions": {"future_action": {}}}}}),
                "phase 'hot' action 'future_action'",
            ),
        ] {
            let e = convert("p", &ilm, false).unwrap_err().to_string();
            assert!(e.contains(needle), "{needle}: {e}");
            let c = convert("p", &ilm, true).unwrap();
            assert!(
                c.notes
                    .iter()
                    .any(|n| n.starts_with("DROPPED") && n.contains(needle)),
                "{needle}: {:?}",
                c.notes
            );
        }
        // lenient drops wait_for_snapshot, and says what that means
        let ilm = json!({"phases": {"delete": {"min_age": "30d", "actions": {"wait_for_snapshot": {"policy": "nightly"}, "delete": {}}}}});
        let c = convert("p", &ilm, true).unwrap();
        assert!(
            c.notes
                .iter()
                .any(|n| n.contains("deleted without waiting for snapshot policy \"nightly\"")),
            "{:?}",
            c.notes
        );
    }

    /// TST-BRK-ILM-001 over generated policies: any mix of phases (known, unknown), every
    /// ILM action (and an unknown one) in every phase, convertible and unconvertible
    /// options, odd min_ages. An independent model says what must be refused.
    mod generated {
        use super::*;
        use proptest::prelude::*;

        const PHASE_POOL: &[&str] = &["hot", "warm", "cold", "frozen", "delete", "bogus"];
        const ACTION_POOL: &[&str] = &[
            "set_priority",
            "unfollow",
            "rollover",
            "readonly",
            "downsample",
            "shrink",
            "forcemerge",
            "searchable_snapshot",
            "allocate",
            "migrate",
            "freeze",
            "wait_for_snapshot",
            "delete",
            "future_action",
        ];
        const AGES: &[&str] = &["0ms", "0", "1d", "7d", "12h", "90m"];

        /// What the converter maps, per phase.
        fn supported(phase: &str, action: &str) -> bool {
            let s: &[&str] = match phase {
                "hot" => &[
                    "set_priority",
                    "rollover",
                    "readonly",
                    "shrink",
                    "forcemerge",
                ],
                "warm" => &[
                    "set_priority",
                    "readonly",
                    "allocate",
                    "migrate",
                    "shrink",
                    "forcemerge",
                ],
                "cold" => &["set_priority", "readonly", "allocate", "migrate"],
                "delete" => &["delete"],
                _ => &[],
            };
            s.contains(&action)
        }

        /// The options generated for an action: (name, value, convertible).
        fn options(action: &str) -> Vec<(&'static str, Value, bool)> {
            let mut o = match action {
                "rollover" => vec![
                    ("max_age", json!("1d"), true),
                    ("max_docs", json!(10), true),
                    ("max_size", json!("5gb"), true),
                    ("max_primary_shard_size", json!("1gb"), true),
                    ("max_primary_shard_docs", json!(5), false),
                    ("min_age", json!("1h"), false),
                    ("min_docs", json!(1), false),
                    ("min_size", json!("1gb"), false),
                    ("min_primary_shard_size", json!("1gb"), false),
                    ("min_primary_shard_docs", json!(1), false),
                ],
                "allocate" => vec![
                    ("number_of_replicas", json!(1), true),
                    ("require", json!({"box": "w"}), true),
                    ("include", json!({"zone": "a"}), true),
                    ("exclude", json!({"zone": "b"}), true),
                    ("total_shards_per_node", json!(1), false),
                ],
                "forcemerge" => vec![
                    ("max_num_segments", json!(1), true),
                    ("index_codec", json!("best_compression"), false),
                ],
                "shrink" => vec![
                    ("number_of_shards", json!(1), true),
                    ("allow_write_after_shrink", json!(true), false),
                ],
                "set_priority" => vec![("priority", json!(50), true)],
                "delete" => vec![("delete_searchable_snapshot", json!(true), true)],
                "migrate" => vec![("enabled", json!(false), true)],
                "searchable_snapshot" => vec![("snapshot_repository", json!("r"), true)],
                "downsample" => vec![("fixed_interval", json!("1h"), true)],
                "wait_for_snapshot" => vec![("policy", json!("s"), true)],
                _ => vec![],
            };
            o.push(("future_option", json!(1), false));
            o
        }

        /// Expected refusals as (phase, needles that its message must contain).
        fn expected(ilm: &Value) -> (Vec<Vec<String>>, bool, bool) {
            let mut out = Vec::new();
            let mut fatal = false;
            let mut convertible = false;
            for (p, def) in ilm["phases"].as_object().unwrap() {
                let ph = format!("phase '{p}'");
                if !PHASES.contains(&p.as_str()) {
                    out.push(vec![format!("unknown phase '{p}'")]);
                    continue;
                }
                if def
                    .get("min_age")
                    .is_some_and(|a| !a.as_str().is_some_and(|a| AGES.contains(&a)))
                {
                    fatal = true;
                }
                if p == "frozen" {
                    out.push(vec![ph]);
                    continue;
                }
                convertible = true;
                if def.get("future_key").is_some() {
                    out.push(vec![ph.clone(), "'future_key'".into()]);
                }
                for (a, cfg) in def["actions"].as_object().unwrap() {
                    let pa = vec![ph.clone(), format!("action '{a}'")];
                    if !supported(p, a) {
                        out.push(pa);
                        continue;
                    }
                    let cfg = cfg.as_object().unwrap();
                    for (o, _, ok) in options(a) {
                        if !ok && cfg.contains_key(o) {
                            let mut n = pa.clone();
                            n.push(o.to_string());
                            out.push(n);
                        }
                    }
                    let has = |k: &str| cfg.contains_key(k);
                    let missing = match a.as_str() {
                        "rollover" => {
                            !["max_age", "max_docs", "max_size", "max_primary_shard_size"]
                                .iter()
                                .any(|k| has(k))
                        }
                        "forcemerge" => !has("max_num_segments"),
                        "shrink" => !has("number_of_shards"),
                        _ => false,
                    };
                    if missing {
                        out.push(pa);
                    }
                }
            }
            (out, fatal, convertible)
        }

        fn policy() -> impl Strategy<Value = Value> {
            let action = (0..ACTION_POOL.len(), any::<u16>());
            let phase = (
                0..PHASE_POOL.len(),
                0..AGES.len() + 4,
                proptest::bool::weighted(0.05),
                proptest::collection::vec(action, 0..5),
            );
            proptest::collection::vec(phase, 0..6).prop_map(|phases| {
                let mut m = Map::new();
                for (p, age, extra, actions) in phases {
                    let mut def = Map::new();
                    match age {
                        i if i < AGES.len() => {
                            def.insert("min_age".into(), json!(AGES[i]));
                        }
                        i if i == AGES.len() => {}
                        i if i == AGES.len() + 1 => {
                            def.insert("min_age".into(), json!(5));
                        }
                        i if i == AGES.len() + 2 => {
                            def.insert("min_age".into(), json!("1.5h"));
                        }
                        _ => {
                            def.insert("min_age".into(), json!("soon"));
                        }
                    }
                    if extra {
                        def.insert("future_key".into(), json!(1));
                    }
                    let mut acts = Map::new();
                    for (a, mask) in actions {
                        let a = ACTION_POOL[a];
                        let opts = options(a);
                        // convertible options often, the others rarely
                        let cfg: Map<String, Value> = opts
                            .into_iter()
                            .enumerate()
                            .filter(|(i, (_, _, ok))| {
                                let bit = (mask >> (i % 16)) & 1 == 1;
                                if *ok {
                                    bit || mask & 0x8000 == 0 && *i == 0
                                } else {
                                    bit && mask & 0x0300 == 0
                                }
                            })
                            .map(|(_, (k, v, _))| (k.to_string(), v))
                            .collect();
                        acts.insert(a.into(), Value::Object(cfg));
                    }
                    def.insert("actions".into(), Value::Object(acts));
                    m.insert(PHASE_POOL[p].into(), Value::Object(def));
                }
                json!({ "phases": m })
            })
        }

        const ISM_ACTIONS: &[&str] = &[
            "index_priority",
            "rollover",
            "read_only",
            "replica_count",
            "allocation",
            "force_merge",
            "shrink",
            "delete",
        ];

        /// The ISM structure OpenSearch validates: states in phase order, unique, forward
        /// transitions with age conditions only, one-key actions it has, delete last.
        fn check_ism(policy: &Value) -> Result<(), String> {
            let states = policy["states"].as_array().ok_or("no states")?;
            let names: Vec<&str> = states.iter().filter_map(|s| s["name"].as_str()).collect();
            if names.is_empty() || names.len() != states.len() {
                return Err(format!("bad state names {names:?}"));
            }
            let order = ["pending", "hot", "warm", "cold", "delete"];
            let rank: Vec<usize> = names
                .iter()
                .map(|n| {
                    order
                        .iter()
                        .position(|o| o == n)
                        .ok_or(format!("state {n}"))
                })
                .collect::<Result<_, _>>()?;
            if !rank.windows(2).all(|w| w[0] < w[1]) {
                return Err(format!("states out of order {names:?}"));
            }
            if policy["default_state"] != json!(names[0]) {
                return Err("default_state is not the first state".into());
            }
            for (i, st) in states.iter().enumerate() {
                for a in st["actions"].as_array().ok_or("actions")? {
                    let o = a.as_object().ok_or("action")?;
                    if o.len() != 1 || !ISM_ACTIONS.contains(&o.keys().next().unwrap().as_str()) {
                        return Err(format!("action {a}"));
                    }
                    if o.contains_key("delete") && names[i] != "delete" {
                        return Err("delete outside the delete state".into());
                    }
                }
                let ts = st["transitions"].as_array().ok_or("transitions")?;
                if ts.len() != usize::from(i + 1 < states.len()) {
                    return Err(format!("state {} has {} transitions", names[i], ts.len()));
                }
                for t in ts {
                    if t["state_name"] != json!(names[i + 1]) {
                        return Err(format!("transition {t}"));
                    }
                    if let Some(c) = t.get("conditions") {
                        let c = c.as_object().ok_or("conditions")?;
                        if c.len() != 1
                            || !c.iter().all(|(k, v)| {
                                (k == "min_index_age" || k == "min_rollover_age")
                                    && v.as_str().is_some_and(is_duration)
                            })
                        {
                            return Err(format!("conditions {t}"));
                        }
                    }
                }
            }
            let text = policy.to_string();
            for k in [
                "searchable_snapshot",
                "downsample",
                "wait_for_snapshot",
                "unfollow",
                "freeze",
                "future_",
                "total_shards_per_node",
                "index_codec",
                "max_primary_shard_docs",
                "min_primary_shard_docs",
                "allow_write_after_shrink",
                "\"min_docs\"",
                "\"min_age\"",
                "\"phases\"",
                "migrate",
                "\"set_priority\"",
                "\"forcemerge\"",
            ] {
                if text.contains(k) {
                    return Err(format!("ILM-only {k} in {text}"));
                }
            }
            Ok(())
        }

        fn cases() -> u32 {
            std::env::var("PROPTEST_CASES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4096)
        }

        proptest! {
            #![proptest_config(ProptestConfig { cases: cases(), ..ProptestConfig::default() })]

            #[test]
            fn every_combination_is_refused_or_dropped_exactly(ilm in policy()) {
                let (problems, fatal, convertible) = expected(&ilm);
                let strict = convert("gen", &ilm, false);
                let lenient = convert("gen", &ilm, true);
                if fatal {
                    for r in [&strict, &lenient] {
                        let e = format!("{:#}", r.as_ref().unwrap_err());
                        prop_assert!(e.contains("min_age") && e.contains("'gen'"), "{e}");
                    }
                    return Ok(());
                }
                match &strict {
                    Ok(c) => {
                        prop_assert!(problems.is_empty() && convertible, "{ilm}: accepted {problems:?}");
                        prop_assert!(!c.notes.iter().any(|n| n.starts_with("DROPPED")));
                        check_ism(&c.policy).map_err(|e| TestCaseError::fail(format!("{ilm}: {e}")))?;
                        let l = lenient.as_ref().unwrap();
                        prop_assert_eq!(&l.policy, &c.policy);
                    }
                    Err(e) => {
                        let e = format!("{e:#}");
                        prop_assert!(!problems.is_empty() || !convertible, "{ilm}: refused {e}");
                        prop_assert!(e.contains("'gen'"), "{e}");
                        for p in &problems {
                            prop_assert!(p.iter().all(|n| e.contains(n.as_str())), "{ilm}: {p:?} not named in {e}");
                        }
                        if convertible {
                            prop_assert!(e.contains("--lenientPolicyConversion"), "{e}");
                        }
                    }
                }
                match &lenient {
                    Ok(c) => {
                        prop_assert!(convertible, "{ilm}");
                        let dropped: Vec<&String> = c.notes.iter().filter(|n| n.starts_with("DROPPED")).collect();
                        prop_assert_eq!(dropped.len(), problems.len(), "{} {:?} {:?}", ilm, dropped, problems);
                        for p in &problems {
                            prop_assert!(
                                dropped.iter().any(|d| p.iter().all(|n| d.contains(n.as_str()))),
                                "{ilm}: {p:?} not in {dropped:?}"
                            );
                        }
                        check_ism(&c.policy).map_err(|e| TestCaseError::fail(format!("{ilm}: {e}")))?;
                    }
                    Err(e) => {
                        prop_assert!(!convertible, "{ilm}: lenient refused {e:#}");
                        let msg = format!("{e:#}");
                        prop_assert!(msg.contains("no convertible phases"), "{}", msg);
                    }
                }
            }
        }
    }
}
