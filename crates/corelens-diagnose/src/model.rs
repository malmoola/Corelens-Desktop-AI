//! What a run is given and what it gives back.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::playbook::{Depth, FixKind, Layer, Severity};

/// What was asked to be diagnosed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Scope {
    Project,
    Site { site: String },
    Devices { devices: Vec<String> },
    /// Between two endpoints: a device's name or an address.
    Path { src: String, dst: String },
}

impl Scope {
    /// The same scope is the same key, so runs of it can be compared.
    pub fn key(&self) -> String {
        match self {
            Scope::Project => "project".into(),
            Scope::Site { site } => format!("site:{}", site.to_lowercase()),
            Scope::Devices { devices } => {
                let mut d: Vec<String> = devices.iter().map(|x| x.to_lowercase()).collect();
                d.sort();
                d.dedup();
                format!("devices:{}", d.join(","))
            }
            Scope::Path { src, dst } => format!("path:{}>{}", src.to_lowercase(), dst.to_lowercase()),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Scope::Project => "the whole project".into(),
            Scope::Site { site } => format!("site {site}"),
            Scope::Devices { devices } if devices.len() == 1 => devices[0].clone(),
            Scope::Devices { devices } => format!("{} devices ({})", devices.len(), devices.join(", ")),
            Scope::Path { src, dst } => format!("the path from {src} to {dst}"),
        }
    }
}

/// One command's answer from one device.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Output {
    pub command_id: String,
    pub command: String,
    /// The text, when the device answered.
    pub text: Option<String>,
    /// Why there is none: refused, timed out, the session dropped.
    pub error: Option<String>,
}

/// One device as the run reached it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceInput {
    pub name: String,
    pub address: String,
    /// The ntc platform its playbook is filed under; empty when unknown.
    pub platform: String,
    pub role: String,
    pub site: String,
    /// Why nothing could be asked at all (unreachable, no login, no
    /// playbook); `None` once logged in.
    pub unreachable: Option<String>,
    pub outputs: Vec<Output>,
    /// The newest configuration Corelens backed up from it, when there is one.
    #[serde(default)]
    pub stored: Option<Stored>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stored {
    /// The capture's name, as get_config lists it.
    pub name: String,
    pub text: String,
}

/// A link of the project's diagram, with the ports it says.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TopoLink {
    pub a: String,
    pub a_port: String,
    pub b: String,
    pub b_port: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// Stable across runs: the check, the device and the key fields.
    pub id: String,
    pub check: String,
    pub title: String,
    pub layer: Layer,
    pub severity: Severity,
    /// The device it is on; for a finding about a link, the one a fix
    /// would change.
    pub device: String,
    pub says: String,
    pub fields: BTreeMap<String, String>,
    /// Lines from the device's own output (redacted) that show it.
    pub evidence: Vec<String>,
    /// The fix template this finding's root cause would use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Passed {
    pub check: String,
    pub title: String,
    pub layer: Layer,
    pub devices: Vec<String>,
    pub says: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotChecked {
    pub device: String,
    /// The check, or `*` for everything on that device.
    pub check: String,
    pub why: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cause {
    pub id: String,
    pub title: String,
    pub severity: Severity,
    /// 0 to 1.
    pub confidence: f64,
    pub why: String,
    /// The findings this explains, the anchor first.
    pub findings: Vec<String>,
    pub supporting: Vec<String>,
    pub ruling_out: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus {
    /// Ready to become a proposal.
    Ready,
    /// A proposal waits in Corelens for the engineer.
    Proposed,
    /// No command is written: the engineer chooses or the evidence did not
    /// give a value safe to put into one.
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    pub seq: u32,
    pub cause: String,
    pub device: String,
    pub kind: FixKind,
    pub title: String,
    pub commands: Vec<String>,
    pub verify: Vec<String>,
    pub impact: String,
    pub rollback: String,
    /// Filled by Corelens from the change layer's own assessment.
    pub risk: String,
    pub status: StepStatus,
    pub note: String,
    pub proposal_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceResult {
    pub name: String,
    pub address: String,
    pub platform: String,
    pub reached: bool,
    pub error: Option<String>,
    pub commands_run: usize,
    pub commands_failed: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDiff {
    pub previous_id: String,
    pub previous_ms: i64,
    pub new: Vec<String>,
    /// Findings of the previous run not found this time, as it said them.
    pub resolved: Vec<String>,
    pub unchanged: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HopCheck {
    pub what: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hop {
    pub device: String,
    pub in_port: String,
    pub out_port: String,
    pub checks: Vec<HopCheck>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathResult {
    pub src: String,
    pub dst: String,
    pub hops: Vec<Hop>,
    /// The index of the first hop where it breaks.
    pub first_break: Option<usize>,
    /// What this computer's ping and traceroute saw.
    pub probe: Vec<String>,
    pub note: String,
}

/// The tables the analyses read, per device, kept with the run so the
/// next one can say what moved.
pub type Tables = BTreeMap<String, BTreeMap<String, Vec<BTreeMap<String, String>>>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub id: String,
    pub scope: Scope,
    pub depth: Depth,
    pub symptom: Option<String>,
    pub started_ms: i64,
    pub finished_ms: i64,
    pub cancelled: bool,
    pub summary: Vec<String>,
    pub devices: Vec<DeviceResult>,
    pub causes: Vec<Cause>,
    pub findings: Vec<Finding>,
    pub passed: Vec<Passed>,
    pub not_checked: Vec<NotChecked>,
    pub plan: Vec<PlanStep>,
    pub diff: Option<RunDiff>,
    pub path: Option<PathResult>,
    /// The playbooks used, `platform@version`, so a report says what it was
    /// judged by.
    pub playbooks: Vec<String>,
    pub tables: Tables,
}

impl Report {
    pub fn finding(&self, id: &str) -> Option<&Finding> {
        self.findings.iter().find(|f| f.id == id)
    }
}
