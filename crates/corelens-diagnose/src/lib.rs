//! Diagnose network: a read-only sweep, judged by playbooks that are data.
//!
//! What to ask each platform and what in the answer is a problem lives in
//! `resources/diagnose/` — one playbook per platform, the analyses across
//! devices, the root causes — each versioned and each check proved on a
//! healthy and a broken recorded output (`resources/diagnose/fixtures/`).
//! This crate reads them and turns a run's captured outputs into a report:
//! findings, ranked causes with their evidence, what passed, what could not
//! be checked, the change since the last run, a path's first broken hop,
//! and a fix plan. It changes nothing and reaches nothing: Corelens asks
//! the devices (read-only, through the same allowlist as every other
//! command) and turns plan steps into proposals the engineer approves.

pub mod analyses;
pub mod correlate;
pub mod model;
pub mod parse;
pub mod path;
pub mod plan;
pub mod playbook;
pub mod report;
pub mod rules;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use model::{DeviceInput, DeviceResult, NotChecked, Passed, PathResult, Report, Scope, TopoLink};
use playbook::{Depth, Playbook};

/// The playbooks shipped with this build, read from `resources/diagnose`.
const PLAYBOOKS: &[(&str, &str)] = &[
    ("cisco_ios", include_str!("../../../resources/diagnose/cisco_ios.yaml")),
    ("cisco_nxos", include_str!("../../../resources/diagnose/cisco_nxos.yaml")),
    ("aruba_aoscx", include_str!("../../../resources/diagnose/aruba_aoscx.yaml")),
    ("juniper_junos", include_str!("../../../resources/diagnose/juniper_junos.yaml")),
    ("fortinet", include_str!("../../../resources/diagnose/fortinet.yaml")),
    ("paloalto_panos", include_str!("../../../resources/diagnose/paloalto_panos.yaml")),
];
const ANALYSES: &str = include_str!("../../../resources/diagnose/analyses.yaml");
const CAUSES: &str = include_str!("../../../resources/diagnose/causes.yaml");

pub struct Library {
    pub playbooks: Vec<Playbook>,
    pub analyses: analyses::Analyses,
    pub causes: correlate::Causes,
}

impl Library {
    /// Reads and checks every file; the first error names the file.
    pub fn load() -> Result<Library, String> {
        let mut playbooks = Vec::new();
        for (name, text) in PLAYBOOKS {
            let pb: Playbook = serde_yaml_ng::from_str(text).map_err(|e| format!("{name}.yaml: {e}"))?;
            if pb.platform != *name {
                return Err(format!("{name}.yaml says it is {}", pb.platform));
            }
            pb.validate()?;
            playbooks.push(pb);
        }
        let analyses: analyses::Analyses = serde_yaml_ng::from_str(ANALYSES).map_err(|e| format!("analyses.yaml: {e}"))?;
        let causes: correlate::Causes = serde_yaml_ng::from_str(CAUSES).map_err(|e| format!("causes.yaml: {e}"))?;
        let lib = Library { playbooks, analyses, causes };
        lib.check_references()?;
        Ok(lib)
    }

    /// This build's library. Its files are compiled in, and a test loads
    /// them, so this cannot fail in a build that passed its tests.
    pub fn embedded() -> &'static Library {
        static L: OnceLock<Library> = OnceLock::new();
        L.get_or_init(|| Library::load().expect("the diagnose playbooks are checked by the crate's tests"))
    }

    pub fn playbook(&self, platform: &str) -> Option<&Playbook> {
        self.playbooks.iter().find(|p| p.platform == platform)
    }

    fn known_check(&self, id: &str) -> bool {
        self.analyses.get(id).is_some() || self.playbooks.iter().any(|p| p.checks.iter().any(|c| c.id == id))
    }

    fn check_references(&self) -> Result<(), String> {
        for a in &self.analyses.analyses {
            if let Some(fix) = &a.fix {
                if !self.playbooks.iter().any(|p| p.fix(fix).is_some()) {
                    return Err(format!("analyses.yaml: {} names fix {fix}, which no playbook has", a.id));
                }
            }
        }
        for c in &self.causes.causes {
            if !self.known_check(&c.anchor) {
                return Err(format!("causes.yaml: {} is anchored on {}, which no playbook or analysis defines", c.id, c.anchor));
            }
            for e in c.explains.iter().map(|e| &e.check).chain(c.contradicted_by.iter().map(|x| &x.check)).chain(c.rules_out.iter().map(|r| &r.passed)) {
                if !self.known_check(e) {
                    return Err(format!("causes.yaml: {} refers to {e}, which no playbook or analysis defines", c.id));
                }
            }
            if let Some(f) = c.fix.as_deref().filter(|f| *f != "none") {
                if !self.playbooks.iter().any(|p| p.fix(f).is_some()) {
                    return Err(format!("causes.yaml: {} names fix {f}, which no playbook has", c.id));
                }
            }
        }
        Ok(())
    }

    /// `platform@version` for each playbook.
    pub fn versions(&self) -> Vec<String> {
        self.playbooks.iter().map(|p| format!("{}@{}", p.platform, p.version)).collect()
    }
}

/// A path run's endpoints and the devices between them.
pub struct PathPlan {
    pub src: String,
    pub dst: String,
    pub chain: Vec<(String, String, String)>,
    /// The VLAN the source sits in, when known.
    pub vlan: Option<u16>,
}

/// Everything one run was given.
pub struct Run<'a> {
    pub id: String,
    pub scope: Scope,
    pub depth: Depth,
    pub symptom: Option<String>,
    pub started_ms: i64,
    pub finished_ms: i64,
    pub cancelled: bool,
    pub devices: &'a [DeviceInput],
    pub links: &'a [TopoLink],
    pub previous: Option<&'a Report>,
    pub templates: Option<&'a parse::Templates>,
    pub redact: rules::Redact<'a>,
    pub path: Option<PathPlan>,
    /// Said in the path result: why there is none, what the probe saw.
    pub path_note: String,
    pub probe: Vec<String>,
}

/// The running configuration against the startup one and the last
/// backup. Returns the findings and the drift checks that found nothing.
fn drift(lib: &Library, pb: &Playbook, d: &playbook::DriftSpec, dev: &DeviceInput, redact: rules::Redact<'_>) -> (Vec<model::Finding>, Vec<String>) {
    let ignore: Vec<regex::Regex> = d.ignore.iter().filter_map(|p| regex::Regex::new(p).ok()).collect();
    let text = |id: &str| dev.outputs.iter().find(|o| o.command_id == id).and_then(|o| o.text.clone()).filter(|t| !parse::refused(t));
    let mut found = Vec::new();
    let mut ran = Vec::new();
    let Some(running) = text(&d.running) else { return (found, ran) };
    // Each line is redacted as the configuration line it is, before it is
    // marked: a redactor that knows `snmp-server community …` does not
    // know `+ snmp-server community …`.
    let add = |id: &str, fields: BTreeMap<String, String>, diff: Vec<(char, String)>, found: &mut Vec<model::Finding>| {
        let evidence = diff.iter().take(12).map(|(sign, l)| format!("{sign} {}", redact(l))).collect();
        if let Some(mut f) = analyses::make(&lib.analyses, id, &dev.name, fields, evidence) {
            if id == "drift.unsaved" {
                f.fix = d.save_fix.clone();
            }
            found.push(f);
        }
    };
    if let Some(startup) = d.startup.as_deref().and_then(text) {
        let diff = analyses::config_diff(&startup, &running, &ignore);
        if diff.is_empty() {
            ran.push("drift.unsaved".to_string());
        } else {
            add("drift.unsaved", [("lines".to_string(), diff.len().to_string())].into(), diff, &mut found);
        }
    }
    if let Some(stored) = &dev.stored {
        let diff = analyses::config_diff(&stored.text, &running, &ignore);
        if diff.is_empty() {
            ran.push("drift.since_backup".to_string());
        } else {
            add("drift.since_backup", [("lines".to_string(), diff.len().to_string()), ("backup".to_string(), stored.name.clone())].into(), diff, &mut found);
        }
    }
    let _ = pb;
    (found, ran)
}

/// The report for one run.
pub fn diagnose(lib: &Library, run: &Run<'_>) -> Report {
    let mut findings = Vec::new();
    let mut not_checked = Vec::new();
    let mut passed_by_check: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut passed_on: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut tables: BTreeMap<String, analyses::DeviceTables> = BTreeMap::new();
    let mut results = Vec::new();
    let mut roles = BTreeMap::new();
    let mut used: BTreeSet<String> = BTreeSet::new();
    for dev in run.devices {
        roles.insert(dev.name.clone(), dev.role.clone());
        let mut res = DeviceResult { name: dev.name.clone(), address: dev.address.clone(), platform: dev.platform.clone(), ..Default::default() };
        if let Some(why) = &dev.unreachable {
            res.error = Some((run.redact)(why));
            not_checked.push(NotChecked { device: dev.name.clone(), check: "*".into(), why: (run.redact)(why) });
            results.push(res);
            continue;
        }
        res.reached = true;
        res.commands_run = dev.outputs.iter().filter(|o| o.text.is_some()).count();
        res.commands_failed = dev.outputs.iter().filter(|o| o.text.is_none()).count();
        let Some(pb) = lib.playbook(&dev.platform) else {
            not_checked.push(NotChecked { device: dev.name.clone(), check: "*".into(), why: format!("Corelens has no diagnose playbook for its platform ({})", if dev.platform.is_empty() { "not identified" } else { &dev.platform }) });
            results.push(res);
            continue;
        };
        used.insert(format!("{}@{}", pb.platform, pb.version));
        let eval = rules::evaluate(pb, dev, run.templates, run.depth, run.path.is_some(), run.redact);
        findings.extend(eval.findings);
        if let Some(d) = &pb.drift {
            let (found, ran) = drift(lib, pb, d, dev, run.redact);
            findings.extend(found);
            for id in ran {
                passed_on.entry(dev.name.clone()).or_default().insert(id.clone());
                passed_by_check.entry(id).or_default().insert(dev.name.clone());
            }
        }
        not_checked.extend(eval.not_checked);
        for p in eval.passed {
            passed_by_check.entry(p.clone()).or_default().insert(dev.name.clone());
            passed_on.entry(dev.name.clone()).or_default().insert(p);
        }
        tables.insert(dev.name.clone(), eval.tables);
        results.push(res);
    }
    // Only the previous run's tables for devices also reached now are
    // compared: a device not reached this time has no "now".
    let before: Option<BTreeMap<String, analyses::DeviceTables>> = run.previous.map(|p| p.tables.clone());
    let (cross, ran) = analyses::run(&lib.analyses, &analyses::Input { now: &tables, before: before.as_ref(), links: run.links, roles: &roles });
    findings.extend(cross);
    for id in ran {
        passed_by_check.entry(id).or_insert_with(|| tables.keys().cloned().collect());
    }
    findings.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.device.cmp(&b.device)).then(a.check.cmp(&b.check)));
    let ranked = correlate::correlate(&lib.causes, &findings, &passed_on, run.symptom.as_deref());
    let platform_of: BTreeMap<String, String> = run.devices.iter().map(|d| (d.name.clone(), d.platform.clone())).collect();
    let plan = plan::build(&ranked, &|dev: &str| platform_of.get(dev).and_then(|p| lib.playbook(p)));
    let mut passed = Vec::new();
    for (check, devices) in passed_by_check {
        let (title, layer, says) = if let Some(a) = lib.analyses.get(&check) {
            (a.title.clone(), a.layer, a.passed.clone())
        } else if let Some(c) = lib.playbooks.iter().flat_map(|p| &p.checks).find(|c| c.id == check) {
            (c.title.clone(), c.layer, c.passed.clone().unwrap_or_else(|| "nothing found".into()))
        } else {
            continue;
        };
        passed.push(Passed { check, title, layer, devices: devices.into_iter().collect(), says });
    }
    passed.sort_by(|a, b| a.layer.cmp(&b.layer).then(a.title.cmp(&b.title)));
    let path = run.path.as_ref().map(|p| {
        let mut r = path::walk(&p.chain, &tables, &findings, &passed_on, p.vlan, &p.src, &p.dst);
        r.probe.clone_from(&run.probe);
        r.note.clone_from(&run.path_note);
        r
    });
    let path = path.or_else(|| match &run.scope {
        Scope::Path { src, dst } => Some(PathResult { src: src.clone(), dst: dst.clone(), probe: run.probe.clone(), note: run.path_note.clone(), ..Default::default() }),
        _ => None,
    });
    let mut report = Report {
        id: run.id.clone(),
        scope: run.scope.clone(),
        depth: run.depth,
        symptom: run.symptom.clone(),
        started_ms: run.started_ms,
        finished_ms: run.finished_ms,
        cancelled: run.cancelled,
        summary: Vec::new(),
        devices: results,
        causes: ranked.into_iter().map(|r| r.cause).collect(),
        findings,
        passed,
        not_checked,
        plan,
        diff: None,
        path,
        playbooks: used.into_iter().collect(),
        tables: tables.into_iter().map(|(d, t)| (d, t.into_iter().collect())).collect(),
    };
    report.diff = run.previous.map(|p| report::diff(&report, p));
    report.summary = report::summary(&report);
    report
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_shipped_library_loads_and_every_reference_resolves() {
        let lib = super::Library::load().unwrap();
        assert_eq!(lib.playbooks.len(), super::PLAYBOOKS.len());
    }
}
