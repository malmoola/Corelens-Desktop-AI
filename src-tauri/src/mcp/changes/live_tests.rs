//! Tier 2 against the fake devices: a proposal through the same
//! `execute` an MCP client and the chat reach, the engineer's decision
//! made the way the window makes it, and what the device ends up holding.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::*;
use crate::mcp::fake_device::{FakeDevice, FakeKind};
use crate::mcp::{execute, Activity, Host, Source};

struct Lab {
    dev: FakeDevice,
    state: AppState,
    hub: ChangeHub,
    shown: Arc<Mutex<Vec<Change>>>,
}

async fn lab_as(kind: FakeKind) -> Lab {
    let dev = FakeDevice::start_as(kind).await;
    let state = crate::mcp::roundtrip::app_state_as("SW1", true);
    Lab { dev, state, hub: ChangeHub::default(), shown: Arc::new(Mutex::new(Vec::new())) }
}

async fn lab() -> Lab {
    lab_as(FakeKind::Ios).await
}

impl Lab {
    async fn call(&self, tool: &str, arguments: Value) -> corelens_mcp::Outcome {
        let emit = |_: &Activity| {};
        let shown = Arc::clone(&self.shown);
        let emit_change = move |c: &Change| shown.lock().unwrap().push(c.clone());
        let host = Host { state: &self.state, templates: None, ssh_port: self.dev.port, emit: &emit, changes: Some(&self.hub), emit_change: &emit_change, source: Source::Mcp { client: Some("test-client 1".into()) }, app: None };
        execute(&host, tool, arguments).await
    }

    async fn rollback(&self, id: &str, seq: u32, by_timer: bool) -> Change {
        let emit = |_: &Activity| {};
        let emit_change = |_: &Change| {};
        let host = Host { state: &self.state, templates: None, ssh_port: self.dev.port, emit: &emit, changes: Some(&self.hub), emit_change: &emit_change, source: Source::Operator, app: None };
        rollback_with(&host, id, seq, by_timer).await.unwrap()
    }

    /// The next proposal shown to the engineer, once it is waiting.
    async fn pending(&self) -> Change {
        for _ in 0..600 {
            let found = self.shown.lock().unwrap().iter().rev().find(|c| c.status == ChangeStatus::Pending && self.hub.is_waiting(&c.id)).cloned();
            if let Some(c) = found {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("no proposal was shown");
    }

    /// Proposes, and decides the way the engineer would, in the window.
    async fn propose_and(&self, arguments: Value, decide: impl FnOnce(&Change) -> (Vec<u32>, Vec<String>)) -> (corelens_mcp::Outcome, Change) {
        let (outcome, decided) = tokio::join!(self.call("propose_config_change", arguments), async {
            let c = self.pending().await;
            let (approve, typed) = decide(&c);
            decide_as_operator(&self.state, &self.hub, &c.id, approve, &typed, None).unwrap();
            c.id
        });
        let stored = store::get(&self.state.db.lock().unwrap(), &decided).unwrap().unwrap();
        (outcome, stored)
    }

    fn running_text(&self) -> String {
        self.dev.running().join("\n")
    }
}

fn all(c: &Change) -> (Vec<u32>, Vec<String>) {
    (c.targets.iter().map(|t| t.seq).collect(), c.targets.iter().map(|t| t.device.clone()).collect())
}

fn description_on_spare() -> Value {
    json!({ "target": "SW1", "commands": ["interface Gi1/0/24", " description LAB-PORT"], "reason": "label the spare port", "verify_commands": ["show interfaces status"] })
}

#[tokio::test(flavor = "multi_thread")]
async fn an_approved_change_is_snapshotted_pushed_verified_and_not_saved() {
    let lab = lab().await;
    let before = lab.dev.running();
    let (outcome, c) = lab.propose_and(description_on_spare(), all).await;
    assert!(!outcome.is_error, "{}", outcome.text);
    assert_eq!(c.status, ChangeStatus::Done, "{c:?}");
    let t = &c.targets[0];
    assert_eq!(t.status, TargetStatus::Verified, "{}", t.message);
    assert_eq!(c.proposer, "test-client 1 (MCP)");
    // Shown before approval: the predicted diff against what was running.
    assert!(t.predicted.iter().any(|d| d.mark == "+" && d.text.contains("description LAB-PORT")), "{:?}", t.predicted);
    assert!(t.predicted.iter().any(|d| d.mark == "-" && d.text.contains("description spare")));
    // Done: the device holds it, a copy was kept first, nothing was saved.
    let running = lab.running_text();
    assert!(running.contains("interface GigabitEthernet1/0/24\n description LAB-PORT"), "{running}");
    assert_eq!(lab.dev.startup(), before, "a change never saves");
    let asked = lab.dev.asked();
    let copy = asked.iter().position(|a| a.starts_with("copy running-config flash:corelens-")).expect("a snapshot on flash first");
    let conf = asked.iter().position(|a| a == "configure terminal").unwrap();
    assert!(copy < conf, "{asked:?}");
    assert!(!asked.iter().any(|a| a.contains("write") || a.contains("startup-config")), "{asked:?}");
    assert!(t.verification.iter().all(|v| v.ok) && t.verification.len() == 2, "{:?}", t.verification);
    assert!(t.actual.iter().any(|d| d.mark == "+" && d.text.contains("LAB-PORT")));
    assert!(!t.snapshot.contains(crate::mcp::fake_device::PASSWORD), "the stored snapshot is redacted");
    assert!(outcome.text.contains("Verified") && outcome.text.contains("Not saved"), "{}", outcome.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_change_sends_nothing_to_the_device() {
    let lab = lab().await;
    let before = lab.dev.running();
    let (outcome, c) = lab.propose_and(description_on_spare(), |_| (Vec::new(), Vec::new())).await;
    assert_eq!(c.status, ChangeStatus::Denied);
    assert!(outcome.text.contains("denied"), "{}", outcome.text);
    assert_eq!(lab.dev.running(), before);
    assert!(!lab.dev.asked().iter().any(|a| a.starts_with("configure") || a.starts_with("copy")), "{:?}", lab.dev.asked());
}

#[tokio::test(flavor = "multi_thread")]
async fn no_decision_in_time_is_a_denial() {
    let lab = lab().await;
    db::set_setting(&lab.state.db.lock().unwrap(), "testProposalTimeoutMs", Some("300")).unwrap();
    let before = lab.dev.running();
    let outcome = lab.call("propose_config_change", description_on_spare()).await;
    assert!(outcome.text.contains("not decided in time"), "{}", outcome.text);
    let c = store::list(&lab.state.db.lock().unwrap(), None, 10).unwrap().remove(0);
    assert_eq!((c.status, c.decided_by.as_str()), (ChangeStatus::TimedOut, "timeout"));
    assert_eq!(lab.dev.running(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_verification_is_offered_for_rollback_and_the_rollback_restores_the_snapshot() {
    let lab = lab().await;
    let before = lab.dev.running();
    let mut args = description_on_spare();
    args["verify_commands"] = json!(["show interfaces bogus"]);
    let (outcome, c) = lab.propose_and(args, all).await;
    assert_eq!(c.status, ChangeStatus::Failed);
    assert_eq!(c.targets[0].status, TargetStatus::VerifyFailed, "{}", c.targets[0].message);
    assert!(outcome.text.contains("FAILED"), "{}", outcome.text);
    assert_ne!(lab.dev.running(), before, "a failed verification leaves the change for the engineer to judge");
    let back = lab.rollback(&c.id, 0, false).await;
    assert_eq!(back.targets[0].status, TargetStatus::RolledBack, "{}", back.targets[0].message);
    assert_eq!(lab.dev.running(), before);
    assert!(lab.dev.asked().iter().any(|a| a.starts_with("configure replace flash:corelens-") && a.ends_with(" force")), "{:?}", lab.dev.asked());
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_copy_on_flash_the_rollback_is_the_reverse_commands() {
    let lab = lab().await;
    lab.dev.fill_flash();
    let before = lab.dev.running();
    let mut args = description_on_spare();
    args["commands"] = json!(["interface Gi1/0/24", " description LAB-PORT", " no shutdown"]);
    args["verify_commands"] = json!(["show interfaces bogus"]);
    let (_, c) = lab.propose_and(args, all).await;
    assert_eq!(c.targets[0].checkpoint, Some(Checkpoint::None));
    let back = lab.rollback(&c.id, 0, false).await;
    assert_eq!(back.targets[0].status, TargetStatus::RolledBack, "{}", back.targets[0].message);
    assert_eq!(lab.dev.running(), before);
    assert!(!lab.dev.asked().iter().any(|a| a.starts_with("configure replace")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_line_stops_the_push_rolls_back_what_was_entered_and_the_batch() {
    let lab = lab().await;
    lab.dev.refuse("description BAD");
    let before = lab.dev.running();
    let args = json!({
        "reason": "two steps",
        "batch": [
            { "target": "SW1", "commands": ["interface Gi1/0/24", " description FIRST", "interface Gi1/0/1", " description BAD"] },
            { "target": "sw1.example.test", "commands": ["interface Gi1/0/24", " description SECOND"] }
        ]
    });
    let (outcome, c) = lab.propose_and(args, all).await;
    assert_eq!(c.status, ChangeStatus::Failed);
    assert_eq!(c.targets[0].status, TargetStatus::PushFailed, "{}", c.targets[0].message);
    assert!(c.targets[0].message.contains("rolled back"), "{}", c.targets[0].message);
    assert_eq!(c.targets[1].status, TargetStatus::NotRun);
    assert_eq!(lab.dev.running(), before, "what was entered before the refusal came out again");
    assert!(!lab.dev.asked().iter().any(|a| a.contains("SECOND")), "the batch stopped");
    assert!(outcome.text.contains("NotRun"), "{}", outcome.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hard_block_needs_the_device_name_typed_and_says_why() {
    let lab = lab().await;
    lab.dev.set_management_address("127.0.0.1");
    let before = lab.dev.running();
    let args = json!({ "target": "SW1", "commands": ["interface Vlan10", " shutdown"], "reason": "test", "verify_commands": [] });
    let (_, c) = tokio::join!(lab.call("propose_config_change", args), async {
        let c = lab.pending().await;
        let t = &c.targets[0];
        assert!(t.assessment.hard_blocks.iter().any(|h| h.risk.contains("127.0.0.1") && h.risk.contains("Vlan10")), "{:?}", t.assessment);
        // The click without the name, and with the wrong one, are refused.
        assert!(decide_as_operator(&lab.state, &lab.hub, &c.id, vec![0], &[], None).unwrap_err().contains("type its name"));
        assert!(decide_as_operator(&lab.state, &lab.hub, &c.id, vec![0], &["sw1".into()], None).is_err());
        assert!(lab.hub.is_waiting(&c.id), "a refused click decides nothing");
        // The engineer cancels.
        decide_as_operator(&lab.state, &lab.hub, &c.id, Vec::new(), &[], Some("not now".into())).unwrap();
        c
    });
    let stored = store::get(&lab.state.db.lock().unwrap(), &c.id).unwrap().unwrap();
    assert_eq!(stored.status, ChangeStatus::Denied);
    assert_eq!(lab.dev.running(), before, "Vlan10 untouched");
    assert!(!lab.dev.asked().iter().any(|a| a == "configure terminal"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reload_is_held_back_and_could_never_run_from_configuration_mode() {
    let lab = lab().await;
    let (_, c) = lab.propose_and(json!({ "target": "SW1", "commands": ["reload"], "reason": "test" }), |_| (Vec::new(), Vec::new())).await;
    let t = &c.targets[0];
    assert!(t.assessment.hard_blocks.iter().any(|h| h.risk.contains("restarts")), "{:?}", t.assessment);
    assert_eq!(c.status, ChangeStatus::Denied);
}

#[tokio::test(flavor = "multi_thread")]
async fn saving_is_its_own_approval_and_never_part_of_a_change() {
    let lab = lab().await;
    let refused = lab.call("propose_config_change", json!({ "target": "SW1", "commands": ["interface Gi1/0/24", " description X", "write memory"], "reason": "x" })).await;
    assert!(refused.is_error && refused.text.contains("propose_save_config"), "{}", refused.text);
    let (_, c) = lab.propose_and(description_on_spare(), all).await;
    assert_eq!(c.targets[0].status, TargetStatus::Verified);
    assert_ne!(lab.dev.startup(), lab.dev.running());
    let (saved, _) = tokio::join!(lab.call("propose_save_config", json!({ "target": "SW1", "reason": "keep the label" })), async {
        let c = lab.pending().await;
        assert_eq!(c.kind, Kind::Save);
        assert!(c.targets[0].predicted.iter().any(|d| d.text.contains("LAB-PORT")), "the dialog shows what saving writes");
        decide_as_operator(&lab.state, &lab.hub, &c.id, vec![0], &[], None).unwrap();
    });
    assert!(saved.text.contains("Saved"), "{}", saved.text);
    assert_eq!(lab.dev.startup(), lab.dev.running());
}

#[tokio::test(flavor = "multi_thread")]
async fn two_proposals_at_once_take_turns_and_the_second_sees_the_first_moved_the_config() {
    let lab = lab().await;
    let a = json!({ "target": "SW1", "commands": ["interface Gi1/0/24", " description FROM-A"], "reason": "a" });
    let b = json!({ "target": "SW1", "commands": ["interface Gi1/0/1", " description FROM-B"], "reason": "b" });
    let decider = async {
        let mut decided = Vec::new();
        while decided.len() < 2 {
            let waiting: Vec<Change> = lab.shown.lock().unwrap().iter().filter(|c| c.status == ChangeStatus::Pending && lab.hub.is_waiting(&c.id) && !decided.contains(&c.id)).cloned().collect();
            for c in waiting {
                decide_as_operator(&lab.state, &lab.hub, &c.id, vec![0], &[], None).unwrap();
                decided.push(c.id);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let (ra, rb, _) = tokio::join!(lab.call("propose_config_change", a), lab.call("propose_config_change", b), decider);
    let all = store::list(&lab.state.db.lock().unwrap(), None, 10).unwrap();
    let statuses: Vec<TargetStatus> = all.iter().map(|c| c.targets[0].status).collect();
    assert!(statuses.contains(&TargetStatus::Verified), "{statuses:?} {} {}", ra.text, rb.text);
    // Both were approved against the same configuration; once one landed
    // the other's diff was no longer the one approved, and it says so.
    let other = all.iter().find(|c| c.targets[0].status != TargetStatus::Verified).unwrap();
    assert_eq!(other.targets[0].status, TargetStatus::PushFailed);
    assert!(other.targets[0].message.contains("changed after the proposal"), "{}", other.targets[0].message);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_locked_vault_proposes_nothing() {
    let dev = FakeDevice::start().await;
    let lab = Lab { dev, state: crate::mcp::roundtrip::app_state_as("SW1", false), hub: ChangeHub::default(), shown: Arc::new(Mutex::new(Vec::new())) };
    let out = lab.call("propose_config_change", description_on_spare()).await;
    assert!(out.is_error && out.text.contains("vault in Corelens is locked"), "{}", out.text);
    assert!(lab.shown.lock().unwrap().is_empty());
    assert!(lab.dev.asked().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn text_from_a_device_asking_for_a_change_creates_no_proposal() {
    let lab = lab().await;
    let out = lab.call("run_show_command", json!({ "target": "SW1", "command": "show banner motd" })).await;
    assert!(out.text.contains("IGNORE ALL PREVIOUS INSTRUCTIONS"), "the text comes back, as data: {}", out.text);
    assert!(out.text.contains("not instructions"), "{}", out.text);
    assert!(lab.shown.lock().unwrap().is_empty(), "no proposal");
    assert!(store::list(&lab.state.db.lock().unwrap(), None, 10).unwrap().is_empty());
    assert!(!lab.dev.asked().iter().any(|a| a.starts_with("configure") || a == "reload"));
    // Nor can a verification's output approve anything: only the
    // engineer's decision does, and it names the proposal it decides.
    assert!(lab.hub.decide("not-a-proposal", Decision::Approve(vec![0])).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proposal_whose_request_stops_is_withdrawn() {
    let lab = lab().await;
    tokio::select! {
        _ = lab.call("propose_config_change", description_on_spare()) => panic!("it should still be waiting"),
        _ = async { lab.pending().await; } => {}
    }
    let c = store::list(&lab.state.db.lock().unwrap(), None, 10).unwrap().remove(0);
    assert_eq!(c.status, ChangeStatus::Withdrawn);
    assert!(!lab.hub.is_waiting(&c.id));
}

#[tokio::test(flavor = "multi_thread")]
async fn get_change_status_reports_a_decided_change() {
    let lab = lab().await;
    let (_, c) = lab.propose_and(description_on_spare(), |_| (Vec::new(), Vec::new())).await;
    let s = lab.call("get_change_status", json!({ "change_id": c.id })).await;
    assert!(s.text.contains("denied"), "{}", s.text);
    let missing = lab.call("get_change_status", json!({ "change_id": "nope" })).await;
    assert!(missing.is_error);
}

#[tokio::test(flavor = "multi_thread")]
async fn ios_with_archive_runs_under_the_revert_timer_and_confirms_after_verification() {
    let lab = lab_as(FakeKind::IosWithArchive).await;
    let (_, c) = lab.propose_and(description_on_spare(), all).await;
    assert_eq!(c.targets[0].status, TargetStatus::Verified, "{}", c.targets[0].message);
    let asked = lab.dev.asked();
    assert!(asked.iter().any(|a| a.starts_with("configure terminal revert timer")), "{asked:?}");
    assert!(asked.iter().any(|a| a == "configure confirm"), "{asked:?}");
    assert!(!lab.dev.unconfirmed());
}

#[tokio::test(flavor = "multi_thread")]
async fn junos_commits_confirmed_and_confirms_only_after_verification() {
    let lab = lab_as(FakeKind::Junos).await;
    let args = json!({ "target": "SW1", "commands": ["set interfaces ge-0/0/5 description LAB-PORT"], "reason": "label" });
    let (_, c) = lab.propose_and(args, all).await;
    assert_eq!(c.targets[0].status, TargetStatus::Verified, "{}", c.targets[0].message);
    let asked = lab.dev.asked();
    let confirmed = asked.iter().position(|a| a.starts_with("commit confirmed 5")).expect("commit confirmed");
    assert!(asked[confirmed + 1..].iter().any(|a| a == "commit"), "{asked:?}");
    assert!(!lab.dev.unconfirmed());
    assert!(lab.dev.running().contains(&"set interfaces ge-0/0/5 description LAB-PORT".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn junos_left_unconfirmed_rolls_itself_back_and_the_record_says_so() {
    let lab = lab_as(FakeKind::Junos).await;
    let before = lab.dev.running();
    let args = json!({ "target": "SW1", "commands": ["set interfaces ge-0/0/5 description LAB-PORT"], "reason": "label", "verify_commands": ["show interfaces bogus"] });
    let (_, c) = lab.propose_and(args, all).await;
    assert_eq!(c.targets[0].status, TargetStatus::DeviceRollingBack, "{}", c.targets[0].message);
    assert!(lab.dev.unconfirmed() && c.targets[0].confirm_by_ms.is_some());
    lab.dev.expire_confirm();
    let after = lab.rollback(&c.id, 0, true).await;
    assert_eq!(after.targets[0].status, TargetStatus::RolledBack);
    assert!(after.targets[0].message.contains("confirm timer"), "{}", after.targets[0].message);
    assert_eq!(lab.dev.running(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn nxos_rolls_back_to_its_checkpoint() {
    let lab = lab_as(FakeKind::NxOs).await;
    let before = lab.dev.running();
    let mut args = description_on_spare();
    args["verify_commands"] = json!(["show interfaces bogus"]);
    let (_, c) = lab.propose_and(args, all).await;
    assert_eq!(c.targets[0].status, TargetStatus::VerifyFailed);
    let back = lab.rollback(&c.id, 0, false).await;
    assert_eq!(back.targets[0].status, TargetStatus::RolledBack, "{}", back.targets[0].message);
    assert_eq!(lab.dev.running(), before);
    let asked = lab.dev.asked();
    assert!(asked.iter().any(|a| a.starts_with("checkpoint corelens-")) && asked.iter().any(|a| a.starts_with("rollback running-config checkpoint corelens-")), "{asked:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn aoscx_rolls_back_to_its_checkpoint_and_saves_with_write_memory() {
    let lab = lab_as(FakeKind::AosCx).await;
    let before = lab.dev.running();
    let mut args = description_on_spare();
    args["verify_commands"] = json!(["show interfaces bogus"]);
    let (_, c) = lab.propose_and(args, all).await;
    let back = lab.rollback(&c.id, 0, false).await;
    assert_eq!(back.targets[0].status, TargetStatus::RolledBack, "{}", back.targets[0].message);
    assert_eq!(lab.dev.running(), before);
    assert!(lab.dev.asked().iter().any(|a| a.starts_with("copy running-config checkpoint corelens-")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_record_exports_as_markdown_and_csv_without_secrets() {
    let lab = lab().await;
    let (_, c) = lab.propose_and(description_on_spare(), all).await;
    let md = export_md("Example project", std::slice::from_ref(&c));
    let csv = export_csv(std::slice::from_ref(&c));
    for text in [&md, &csv] {
        assert!(!text.contains(crate::mcp::fake_device::PASSWORD) && !text.contains(crate::mcp::fake_device::COMMUNITY), "{text}");
    }
    assert!(md.contains("description LAB-PORT") && md.contains("test-client 1 (MCP)") && md.contains("label the spare port"), "{md}");
    assert_eq!(csv.lines().count(), 2, "{csv}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_line_the_device_took_and_ignored_fails_the_took_effect_check() {
    let lab = lab().await;
    lab.dev.ignore("description LAB-PORT");
    let (_, c) = lab.propose_and(description_on_spare(), all).await;
    let t = &c.targets[0];
    assert_eq!(t.status, TargetStatus::VerifyFailed);
    let took = t.verification.iter().find(|v| v.command == "every line took effect").unwrap();
    assert!(!took.ok && took.output.contains("description LAB-PORT"), "{took:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_configuration_changed_by_someone_else_after_the_proposal_is_not_pushed() {
    let lab = lab().await;
    let (_, c) = tokio::join!(lab.call("propose_config_change", description_on_spare()), async {
        let c = lab.pending().await;
        lab.dev.edit_running("ntp server 192.0.2.123");
        decide_as_operator(&lab.state, &lab.hub, &c.id, vec![0], &[], None).unwrap();
        c
    });
    let stored = store::get(&lab.state.db.lock().unwrap(), &c.id).unwrap().unwrap();
    assert_eq!(stored.targets[0].status, TargetStatus::PushFailed);
    assert!(stored.targets[0].message.contains("changed after the proposal"));
    assert!(!lab.dev.asked().iter().any(|a| a == "configure terminal"));
}
