//! Findings into likely root causes.
//!
//! A cause is anchored on one finding and explains others that share a
//! device, a port or a VLAN with it — an HSRP pair both active because the
//! VLAN between them is not on the trunk is one problem, not two. Each
//! explained finding raises the confidence; a finding that points
//! elsewhere lowers it; a check that passed and rules something out is
//! said. The rules are data (`resources/diagnose/causes.yaml`); a symptom
//! the engineer gave raises the causes in the layers it is about.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::analyses::canon_port;
use crate::model::{Cause, Finding};
use crate::playbook::Layer;
use crate::rules::fill;

/// `field in the other finding` → `field in the anchor`; `device` is the
/// finding's device.
pub type Join = BTreeMap<String, String>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Explains {
    pub check: String,
    #[serde(default)]
    pub join: Join,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contradicts {
    pub check: String,
    #[serde(default)]
    pub join: Join,
    pub lowers: f64,
    pub says: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesOut {
    /// A check that ran on the anchor's device and found nothing.
    pub passed: String,
    pub says: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CauseSpec {
    pub id: String,
    pub anchor: String,
    pub title: String,
    pub confidence: f64,
    pub why: String,
    #[serde(default)]
    pub explains: Vec<Explains>,
    #[serde(default)]
    pub contradicted_by: Vec<Contradicts>,
    #[serde(default)]
    pub rules_out: Vec<RulesOut>,
    /// The fix; the anchor's own when not given. `none` for a cause that
    /// has no fix a command can make.
    #[serde(default)]
    pub fix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Symptom {
    pub words: Vec<String>,
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Causes {
    pub format: u32,
    pub version: u32,
    pub causes: Vec<CauseSpec>,
    #[serde(default)]
    pub symptoms: Vec<Symptom>,
}

fn value(f: &Finding, field: &str) -> String {
    let v = if field == "device" { f.device.clone() } else { f.fields.get(field).cloned().unwrap_or_default() };
    let l = v.to_ascii_lowercase();
    if field.contains("port") || field == "interface" {
        canon_port(&l)
    } else {
        l
    }
}

fn joined(join: &Join, other: &Finding, anchor: &Finding) -> bool {
    join.iter().all(|(theirs, ours)| {
        let a = value(anchor, ours);
        !a.is_empty() && value(other, theirs) == a
    })
}

/// The layers a symptom is about.
pub fn symptom_layers(causes: &Causes, symptom: Option<&str>) -> BTreeSet<Layer> {
    let Some(s) = symptom.map(str::to_lowercase) else { return BTreeSet::new() };
    causes.symptoms.iter().filter(|x| x.words.iter().any(|w| s.contains(&w.to_lowercase()))).flat_map(|x| x.layers.iter().copied()).collect()
}

/// A cause with the fix id it takes, for the plan.
pub struct Ranked {
    pub cause: Cause,
    pub fix: Option<String>,
    pub anchor: Finding,
}

/// The causes these findings point to, most likely first.
pub fn correlate(spec: &Causes, findings: &[Finding], passed: &BTreeMap<String, BTreeSet<String>>, symptom: Option<&str>) -> Vec<Ranked> {
    let boost = symptom_layers(spec, symptom);
    let mut out: Vec<Ranked> = Vec::new();
    let mut explained: BTreeSet<String> = BTreeSet::new();
    for c in &spec.causes {
        for anchor in findings.iter().filter(|f| f.check == c.anchor) {
            if explained.contains(&anchor.id) && out.iter().any(|r| r.cause.findings.contains(&anchor.id)) {
                continue;
            }
            let mut confidence = c.confidence;
            let mut members = vec![anchor.id.clone()];
            let mut supporting: Vec<String> = anchor.evidence.iter().map(|e| format!("{}: {e}", anchor.device)).collect();
            for e in &c.explains {
                for other in findings.iter().filter(|f| f.check == e.check && f.id != anchor.id && joined(&e.join, f, anchor)) {
                    if !members.contains(&other.id) {
                        members.push(other.id.clone());
                        supporting.push(format!("{} — {}", other.device, other.says));
                        confidence += 0.05;
                    }
                }
            }
            let mut ruling_out = Vec::new();
            for x in &c.contradicted_by {
                for other in findings.iter().filter(|f| f.check == x.check && joined(&x.join, f, anchor)) {
                    confidence -= x.lowers;
                    ruling_out.push(format!("Against: {} ({} — {})", fill(&x.says, &anchor.device, &anchor.fields), other.device, other.says));
                }
            }
            for r in &c.rules_out {
                if passed.get(&anchor.device).is_some_and(|p| p.contains(&r.passed)) {
                    ruling_out.push(fill(&r.says, &anchor.device, &anchor.fields));
                }
            }
            if boost.contains(&anchor.layer) {
                confidence += 0.05;
            }
            let confidence = (confidence.clamp(0.05, 0.97) * 100.0).round() / 100.0;
            let severity = members.iter().filter_map(|id| findings.iter().find(|f| &f.id == id)).map(|f| f.severity).max().unwrap_or(anchor.severity);
            explained.extend(members.iter().cloned());
            out.push(Ranked {
                cause: Cause {
                    id: format!("{}|{}", c.id, anchor.id),
                    title: fill(&c.title, &anchor.device, &anchor.fields),
                    severity,
                    confidence,
                    why: fill(&c.why, &anchor.device, &anchor.fields),
                    findings: members,
                    supporting,
                    ruling_out,
                },
                fix: match c.fix.as_deref() {
                    Some("none") => None,
                    Some(f) => Some(f.to_string()),
                    None => anchor.fix.clone(),
                },
                anchor: anchor.clone(),
            });
        }
    }
    // A finding explained by a stronger cause is not also the anchor of a
    // weaker one of its own.
    out.sort_by(|a, b| b.cause.severity.cmp(&a.cause.severity).then(b.cause.confidence.partial_cmp(&a.cause.confidence).unwrap_or(std::cmp::Ordering::Equal)));
    let mut kept: Vec<Ranked> = Vec::new();
    for r in out {
        if kept.iter().any(|k| k.cause.findings.contains(&r.anchor.id)) {
            continue;
        }
        kept.push(r);
    }
    kept
}
