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
            if *p == "frozen" {
                cx.problem(
                    "phase 'frozen': it only holds searchable snapshots, which OpenSearch ISM cannot express"
                        .into(),
                );
                continue;
            }
            present.push((p, def));
        }
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
        "set_priority" => match c.get("priority") {
            Some(p) if !p.is_null() => out.push(json!({"index_priority": {"priority": p}})),
            _ => cx.notes.push(format!(
                "phase '{phase}': set_priority null (reset) dropped"
            )),
        },
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
            if let Some(n) = c.get("max_num_segments") {
                out.push(json!({"force_merge": {"max_num_segments": n}}));
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
        "readonly" => out.push(json!({"read_only": {}})),
        "delete" => {
            if c.get("delete_searchable_snapshot").is_some() {
                cx.notes.push("delete: delete_searchable_snapshot ignored (no searchable snapshots on OpenSearch)".into());
            }
            out.push(json!({"delete": {}}));
        }
        "migrate" => cx.notes.push(format!(
            "phase '{phase}': migrate dropped (OpenSearch has no data tiers)"
        )),
        "searchable_snapshot" | "downsample" | "wait_for_snapshot" | "unfollow" | "freeze" => {
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
}
