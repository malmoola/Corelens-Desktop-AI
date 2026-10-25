//! Every check of every playbook, on recorded output: silent on the
//! healthy fixture, firing on the broken one. The fixtures are
//! reconstructed from the vendors' command references, not captured from
//! devices (resources/diagnose/fixtures/README.md).

use std::path::{Path, PathBuf};

use corelens_diagnose::model::{DeviceInput, Output, Scope, Stored};
use corelens_diagnose::parse::Templates;
use corelens_diagnose::playbook::Depth;
use corelens_diagnose::{diagnose, Library, Run};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources")
}

fn templates() -> Templates {
    Templates::load(&root().join("templates/ntc")).unwrap()
}

fn read(dir: &Path, id: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(format!("{id}.txt"))).ok()
}

/// The device as a run would have read it, from one fixture set (the
/// broken set falls back to the healthy file where it has none).
fn device(lib: &Library, platform: &str, broken: bool) -> DeviceInput {
    let pb = lib.playbook(platform).unwrap();
    let base = root().join("diagnose/fixtures").join(platform);
    let mut outputs = Vec::new();
    for c in pb.commands_for(Depth::Full, true) {
        let text = if broken { read(&base.join("broken"), &c.id).or_else(|| read(&base.join("healthy"), &c.id)) } else { read(&base.join("healthy"), &c.id) };
        let text = text.unwrap_or_else(|| panic!("{platform}: no healthy fixture for {}", c.id));
        outputs.push(Output { command_id: c.id.clone(), command: c.line(Some(&Default::default())).unwrap_or_else(|| c.command.clone()), text: Some(text), error: None });
    }
    let stored = pb.drift.as_ref().and_then(|d| read(&base.join("healthy"), &d.running)).map(|text| Stored { name: "SW1-running.txt".into(), text });
    DeviceInput { name: "SW1".into(), address: "192.0.2.1".into(), platform: platform.into(), outputs, stored, ..Default::default() }
}

fn run<'a>(devices: &'a [DeviceInput], t: &'a Templates, redact: corelens_diagnose::rules::Redact<'a>) -> Run<'a> {
    Run {
        id: "t".into(),
        scope: Scope::Project,
        depth: Depth::Full,
        symptom: None,
        started_ms: 0,
        finished_ms: 0,
        cancelled: false,
        devices,
        links: &[],
        previous: None,
        templates: Some(t),
        redact,
        path: Some(corelens_diagnose::PathPlan { src: "a".into(), dst: "b".into(), chain: vec![("SW1".into(), String::new(), String::new())], vlan: None }),
        path_note: String::new(),
        probe: Vec::new(),
    }
}

#[test]
fn every_check_is_silent_on_healthy_output_and_fires_on_broken() {
    let lib = Library::load().unwrap();
    let t = templates();
    let same = |s: &str| s.to_string();
    let mut problems = Vec::new();
    for pb in &lib.playbooks {
        let healthy = [device(&lib, &pb.platform, false)];
        let r = diagnose(&lib, &run(&healthy, &t, &same));
        for f in &r.findings {
            problems.push(format!("{} healthy: {} fired: {}", pb.platform, f.check, f.says));
        }
        for n in &r.not_checked {
            problems.push(format!("{} healthy: {} not checked: {}", pb.platform, n.check, n.why));
        }
        let broken = [device(&lib, &pb.platform, true)];
        let r = diagnose(&lib, &run(&broken, &t, &same));
        for c in &pb.checks {
            if !r.findings.iter().any(|f| f.check == c.id) {
                problems.push(format!("{} broken: {} did not fire", pb.platform, c.id));
            }
        }
        if let Some(d) = &pb.drift {
            for id in ["drift.since_backup"].iter().chain(d.startup.as_ref().map(|_| &"drift.unsaved")) {
                if !r.findings.iter().any(|f| f.check == *id) {
                    problems.push(format!("{} broken: {id} did not fire", pb.platform));
                }
            }
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}
