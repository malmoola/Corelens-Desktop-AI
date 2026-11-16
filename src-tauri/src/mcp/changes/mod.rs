//! Tier 2: configuration changes, proposed by a model and decided by the
//! engineer in this window.
//!
//! **Only a model's tool call creates a proposal, and only a click here
//! approves one.** `propose_config_change` and `propose_save_config` are
//! tools like the rest — the same layer serves an MCP client and the
//! in-app chat — and the call waits for a decision made through
//! [`change_decide`], a command the page sends and no tool can reach.
//! Nothing a device prints can start or approve a change: device output is
//! data that goes back to the model, and the model can only ask.
//!
//! A proposal reads each device's running configuration first, so the
//! dialog shows the exact commands, a diff against what is running, the
//! risk, and the rollback the platform offers. Lines that can cut the
//! device off (`risk::HardBlock`) need its name typed as well. A decision
//! not made in time (10 minutes unless Settings says otherwise) is a
//! denial. An approved change runs device by device, in order: snapshot,
//! the platform's checkpoint, the push, a check that every line took
//! effect, the verification commands, the confirm. The first failure
//! stops the batch; a refused line is rolled back at once, a failed
//! verification is offered for rollback in one click. Saving is never part
//! of a change: it is its own proposal and its own approval.

pub mod diff;
pub mod platform;
pub mod risk;
pub mod store;
#[cfg(test)]
mod live_tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use corelens_discover::ssh::{connect_first_accepted, Device, SshOptions};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{oneshot, Notify};

use self::diff::DiffLine;
use self::platform::{Checkpoint, Cli, Exchange, Platform};
use self::risk::Assessment;
use super::{redact, show, tools, Host, Raw};
use crate::commands::{open_project, AppState};
use crate::db;
use crate::vault_commands::db_err;

type CmdResult<T> = Result<T, String>;

/// The app setting for how long a proposal waits, in minutes.
pub const TIMEOUT_SETTING: &str = "aiProposalTimeoutMinutes";
pub const DEFAULT_TIMEOUT_MIN: i64 = 10;
pub const MAX_TIMEOUT_MIN: i64 = 60;
/// How long a change's commands may take, a commit included.
const COMMAND: Duration = Duration::from_secs(180);
const CONNECT: Duration = Duration::from_secs(10);

pub const EVENT: &str = "corelens://change";

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Change,
    Save,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeStatus {
    Pending,
    Denied,
    TimedOut,
    Withdrawn,
    Running,
    Done,
    Failed,
    Interrupted,
}

impl ChangeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeStatus::Pending => "pending",
            ChangeStatus::Denied => "denied",
            ChangeStatus::TimedOut => "timedOut",
            ChangeStatus::Withdrawn => "withdrawn",
            ChangeStatus::Running => "running",
            ChangeStatus::Done => "done",
            ChangeStatus::Failed => "failed",
            ChangeStatus::Interrupted => "interrupted",
        }
    }

    fn open(self) -> bool {
        matches!(self, ChangeStatus::Pending | ChangeStatus::Running)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetStatus {
    Pending,
    Denied,
    /// Approved, not reached: the batch stopped before it.
    NotRun,
    Running,
    /// A line was refused; what was entered was rolled back.
    PushFailed,
    Verified,
    /// Pushed, and a check failed; rollback is offered.
    VerifyFailed,
    /// Waiting for the device to roll back by itself (an unconfirmed
    /// commit or revert timer).
    DeviceRollingBack,
    RolledBack,
    RollbackFailed,
    Saved,
    SaveFailed,
    Interrupted,
}

/// One check after a push.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub command: String,
    pub ok: bool,
    pub output: String,
}

/// One device's part of a change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub seq: u32,
    pub device: String,
    pub address: String,
    pub platform: Platform,
    pub platform_name: String,
    pub commands: Vec<String>,
    pub verify_commands: Vec<String>,
    pub assessment: Assessment,
    /// The predicted diff against the running configuration read for the
    /// proposal.
    pub predicted: Vec<DiffLine>,
    /// Whether `predicted` is a prediction from the configuration model
    /// (Cisco-style platforms) or the lines as typed.
    pub predicted_is_model: bool,
    pub rollback_plan: String,
    pub status: TargetStatus,
    pub message: String,
    pub checkpoint: Option<Checkpoint>,
    /// The running configuration before the push, redacted.
    pub snapshot: String,
    /// The diff the device's own configuration shows after the push.
    pub actual: Vec<DiffLine>,
    pub push: Vec<Exchange>,
    pub verification: Vec<Check>,
    pub rollback: Vec<Exchange>,
    /// When the device will roll back by itself unless confirmed.
    pub confirm_by_ms: Option<i64>,
    pub updated_ms: i64,
}

/// A change, as the record keeps it and the page shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub id: String,
    pub project_id: Option<String>,
    pub kind: Kind,
    /// `mcp` or `chat`.
    pub source: String,
    /// Who asked, as a person reads it: the MCP client's name, or the
    /// chat's provider and model.
    pub proposer: String,
    pub reason: String,
    pub created_ms: i64,
    pub deadline_ms: i64,
    pub status: ChangeStatus,
    pub decided_ms: Option<i64>,
    /// `operator`, `timeout`, `withdrawn`.
    pub decided_by: String,
    pub note: String,
    pub targets: Vec<Target>,
}

impl Change {
    /// Closed because the app stopped while it was open.
    pub fn interrupted(&mut self) {
        self.status = ChangeStatus::Interrupted;
        self.note = "Corelens closed while this was open; nothing further was done.".into();
        for t in &mut self.targets {
            if matches!(t.status, TargetStatus::Pending | TargetStatus::Running) {
                t.status = TargetStatus::Interrupted;
            }
        }
    }
}

/// What the engineer decided.
#[derive(Debug)]
pub enum Decision {
    /// The `seq`s approved; the rest are denied.
    Approve(Vec<u32>),
    Deny(String),
}

/// The proposals waiting for a decision, and a lock per device so two
/// changes never reach the same device at once.
#[derive(Default)]
pub struct ChangeHub {
    waiting: Mutex<HashMap<String, oneshot::Sender<Decision>>>,
    devices: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Woken whenever a change moves, for `get_change_status`.
    moved: Notify,
}

impl ChangeHub {
    async fn device_lock(&self, address: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.devices.lock().await;
        Arc::clone(map.entry(address.to_string()).or_default())
    }

    /// Hands a decision to the call waiting for it.
    pub fn decide(&self, id: &str, decision: Decision) -> Result<(), String> {
        let tx = self.waiting.lock().map_err(|_| "busy")?.remove(id).ok_or("That proposal is no longer waiting for a decision.")?;
        tx.send(decision).map_err(|_| "That proposal is no longer waiting for a decision.".to_string())
    }

    pub fn is_waiting(&self, id: &str) -> bool {
        self.waiting.lock().map(|w| w.contains_key(id)).unwrap_or(false)
    }
}

/// Withdraws a proposal whose call went away before a decision (the
/// chat's Stop): it is closed and the dialog for it disappears.
struct Waiting<'a, 'h> {
    hub: &'a ChangeHub,
    id: String,
    armed: bool,
    host: &'a Host<'h>,
}

impl Drop for Waiting<'_, '_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(mut w) = self.hub.waiting.lock() {
            w.remove(&self.id);
        }
        let state = self.host.state;
        let loaded = state.db.lock().ok().and_then(|c| store::get(&c, &self.id).ok().flatten());
        if let Some(mut c) = loaded {
            if c.status == ChangeStatus::Pending {
                c.status = ChangeStatus::Withdrawn;
                c.decided_by = "withdrawn".into();
                c.decided_ms = Some(now_ms());
                c.note = "The request was stopped before a decision.".into();
                for t in &mut c.targets {
                    t.status = TargetStatus::Denied;
                }
                record(self.host, &c);
            }
        }
    }
}

/// Stores a change and tells the page.
fn record(host: &Host<'_>, c: &Change) {
    if let Ok(conn) = host.state.db.lock() {
        let _ = store::put(&conn, c);
    }
    (host.emit_change)(c);
    if let Some(hub) = host.changes {
        hub.moved.notify_waiters();
    }
}

fn gate(text: &str, secrets: &redact::Secrets) -> String {
    redact::gate(text, secrets).unwrap_or_else(|_| redact::WITHHELD.to_string())
}

fn gate_exchanges(ex: Vec<Exchange>, secrets: &redact::Secrets) -> Vec<Exchange> {
    ex.into_iter().map(|e| Exchange { command: gate(&e.command, secrets), output: gate(&e.output, secrets) }).collect()
}

fn timeout_minutes(state: &AppState) -> i64 {
    state
        .db
        .lock()
        .ok()
        .and_then(|c| db::all_settings(&c).ok())
        .and_then(|s| s.get(TIMEOUT_SETTING).and_then(|v| v.parse::<i64>().ok()))
        .unwrap_or(DEFAULT_TIMEOUT_MIN)
        .clamp(1, MAX_TIMEOUT_MIN)
}

/// How long a proposal waits, in milliseconds. A test can make it short.
fn timeout_ms(state: &AppState) -> i64 {
    #[cfg(test)]
    if let Some(ms) = state.db.lock().ok().and_then(|c| db::all_settings(&c).ok()).and_then(|s| s.get("testProposalTimeoutMs").and_then(|v| v.parse::<i64>().ok())) {
        return ms;
    }
    timeout_minutes(state) * 60_000
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchItem {
    target: String,
    commands: Vec<String>,
    #[serde(default)]
    verify_commands: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposeArgs {
    target: Option<String>,
    commands: Option<Vec<String>>,
    reason: String,
    #[serde(default)]
    verify_commands: Vec<String>,
    batch: Option<Vec<BatchItem>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveArgs {
    target: String,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    change_id: String,
}

/// The lines as the device should get them: one per entry (an entry
/// holding several lines is split), the framing the procedure types
/// itself left out, and a save refused with where to ask for one.
pub fn normalise(platform: Platform, commands: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for entry in commands {
        for line in entry.split('\n') {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || platform.framing(line) {
                continue;
            }
            let l = line.trim().to_ascii_lowercase();
            let saves = l == "write" || l == "wr" || l == "wr mem" || l.starts_with("write mem") || l.starts_with("write file") || l.starts_with("copy running-config startup-config") || l.starts_with("copy run start") || l.starts_with("execute cfg save") || (l.starts_with("save config") && platform != Platform::PanOs);
            if saves {
                return Err(format!("`{}` saves the configuration; saving is its own approval: call propose_save_config after the change.", line.trim()));
            }
            if l.starts_with("do ") {
                return Err(format!("`{}` runs an exec command from configuration mode; propose only configuration lines.", line.trim()));
            }
            out.push(line.to_string());
        }
    }
    if out.is_empty() {
        return Err("There are no configuration lines to propose.".into());
    }
    if out.len() > 200 {
        return Err(format!("{} lines is more than one proposal takes (200); split it.", out.len()));
    }
    Ok(out)
}

/// A device ready to be worked on: logged in, privileged, identified.
struct Session {
    device: Device,
    platform: Platform,
    label: String,
    address: String,
}

async fn open_session(host: &Host<'_>, project_id: &str, target: &str) -> Result<Session, String> {
    let (device, address, chain) = super::login_for(host.state, project_id, target)?;
    let store = crate::discovery::load_host_keys(host.state)?;
    let logins: Vec<_> = chain.iter().map(|(_, c)| c.clone()).collect();
    for (id, _) in chain.iter().take(1) {
        crate::vault_commands::note_use(host.state, id, "AI change (approved in Corelens)", &address);
    }
    let options = SshOptions { port: host.ssh_port, connect_timeout: CONNECT, command_timeout: COMMAND, ..SshOptions::default() };
    let connected = tokio::time::timeout(Duration::from_secs(90), connect_first_accepted(&address, &logins, options, Arc::clone(&store), None)).await;
    crate::discovery::persist_host_keys_to(host.state, &store);
    let (mut dev, which) = match connected {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => return Err(show::explain(&address, host.ssh_port, &e)),
        Err(_) => return Err(format!("{} ({address}) did not finish logging in within 90s. Is the VPN still connected?", device.label())),
    };
    let mut version = String::new();
    for spelling in corelens_discover::dialect::VERSION_COMMANDS {
        let out = dev.run(spelling).await.unwrap_or_default();
        if corelens_discover::dialect::identifies(&out) {
            version = out;
            break;
        }
    }
    let family = corelens_discover::dialect::family_of(&version);
    let Some(platform) = Platform::from_family(family) else {
        dev.close().await;
        return Err(format!("{} ({address}) is not a platform Corelens can change. Supported: Cisco IOS/IOS-XE, NX-OS, Aruba AOS-CX, Junos, FortiOS, PAN-OS.", device.label()));
    };
    // Configuration mode needs privilege on the platforms that have enable.
    if platform.cisco_like() && !dev.prompt.enabled {
        let enable = logins.get(which).and_then(|c| c.enable_password.clone());
        let _ = dev.enable(enable.as_ref()).await;
    }
    Ok(Session { device: dev, platform, label: device.label().to_string(), address })
}

async fn read_running(s: &mut Session) -> Result<String, String> {
    let out = s.device.run(s.platform.running_command()).await.map_err(|e| e.to_string())?;
    if let Some(why) = platform::refused(&out) {
        return Err(format!("{} would not show its configuration: {why}", s.label));
    }
    Ok(out.replace("\r\n", "\n"))
}

/// One device's proposal: read what is running and say what would change.
async fn plan_target(host: &Host<'_>, project_id: &str, seq: u32, item: &BatchItem, secrets: &redact::Secrets) -> Result<Target, String> {
    let mut s = open_session(host, project_id, &item.target).await?;
    let read = read_running(&mut s).await;
    s.device.close().await;
    let running = read?;
    let commands = normalise(s.platform, &item.commands)?;
    for v in &item.verify_commands {
        show::precheck(v).map_err(|e| format!("verify command {e}"))?;
    }
    let path = if s.platform.cisco_like() { risk::management_path(&running, &s.address) } else { risk::ManagementPath { address: s.address.clone(), ..Default::default() } };
    let assessment = risk::assess(s.platform, &commands, &path);
    let (predicted, predicted_is_model) = if s.platform.cisco_like() {
        let before = diff::parse(&running);
        let after = diff::apply(&before, &commands);
        (diff::tree_diff(&before, &after), true)
    } else {
        (commands.iter().map(|c| DiffLine { mark: "+".into(), text: c.clone() }).collect(), false)
    };
    let predicted = predicted.into_iter().map(|d| DiffLine { mark: d.mark, text: gate(&d.text, secrets) }).collect();
    Ok(Target {
        seq,
        device: s.label,
        address: s.address,
        platform: s.platform,
        platform_name: s.platform.name().into(),
        commands,
        verify_commands: item.verify_commands.clone(),
        assessment,
        predicted,
        predicted_is_model,
        rollback_plan: s.platform.rollback_plan().into(),
        status: TargetStatus::Pending,
        message: String::new(),
        checkpoint: None,
        snapshot: gate(&running, secrets),
        actual: Vec::new(),
        push: Vec::new(),
        verification: Vec::new(),
        rollback: Vec::new(),
        confirm_by_ms: None,
        updated_ms: now_ms(),
    })
}

/// Waits for the engineer, the clock, or the call going away.
async fn await_decision(host: &Host<'_>, hub: &ChangeHub, change: &mut Change) -> Option<Decision> {
    let (tx, rx) = oneshot::channel();
    if let Ok(mut w) = hub.waiting.lock() {
        w.insert(change.id.clone(), tx);
    }
    record(host, change);
    let mut guard = Waiting { hub, id: change.id.clone(), armed: true, host };
    let left = Duration::from_millis((change.deadline_ms - now_ms()).max(0) as u64);
    let decided = tokio::time::timeout(left, rx).await;
    guard.armed = false;
    if let Ok(mut w) = hub.waiting.lock() {
        w.remove(&change.id);
    }
    change.decided_ms = Some(now_ms());
    match decided {
        Ok(Ok(d)) => {
            change.decided_by = "operator".into();
            Some(d)
        }
        _ => {
            change.status = ChangeStatus::TimedOut;
            change.decided_by = "timeout".into();
            change.note = format!("No decision within {} minute(s); treated as denied.", (change.deadline_ms - change.created_ms) / 60_000);
            for t in &mut change.targets {
                t.status = TargetStatus::Denied;
            }
            record(host, change);
            None
        }
    }
}

/// `propose_config_change`.
pub async fn propose(host: &Host<'_>, project_id: &str, arguments: &Value) -> Raw {
    propose_as(host, project_id, arguments, new_id()).await
}

/// A proposal whose id the caller chose — a diagnosis's fix plan names the
/// change of each step before the step waits for its decision.
pub async fn propose_as(host: &Host<'_>, project_id: &str, arguments: &Value, id: String) -> Raw {
    let mut raw = Raw { result: Err(String::new()), device: String::new(), address: String::new(), command: String::new() };
    raw.result = propose_inner(host, project_id, arguments, &mut raw, id).await;
    raw
}

/// A fresh change id.
pub fn fresh_id() -> String {
    new_id()
}

async fn propose_inner(host: &Host<'_>, project_id: &str, arguments: &Value, raw: &mut Raw, id: String) -> Result<String, String> {
    let hub = host.changes.ok_or("Configuration changes are not available from here.")?;
    let args: ProposeArgs = tools::args("propose_config_change", arguments)?;
    let items: Vec<BatchItem> = match (args.batch, args.target, args.commands) {
        (Some(b), None, None) if !b.is_empty() => b,
        (None, Some(target), Some(commands)) => vec![BatchItem { target, commands, verify_commands: args.verify_commands }],
        _ => return Err("propose_config_change: give either target and commands, or batch (not both).".into()),
    };
    if args.reason.trim().is_empty() {
        return Err("propose_config_change: say why in reason.".into());
    }
    if host.state.vault_key.lock().map(|k| k.is_none()).unwrap_or(true) {
        return Err("The vault in Corelens is locked, so no login can be used and nothing can be proposed. The engineer unlocks it in the Corelens window, then ask again.".into());
    }
    let secrets = super::vault_secrets(host.state);
    let mut targets = Vec::new();
    for (i, item) in items.iter().enumerate() {
        targets.push(plan_target(host, project_id, i as u32, item, &secrets).await?);
    }
    raw.device = targets.iter().map(|t| t.device.clone()).collect::<Vec<_>>().join(", ");
    raw.address = targets.iter().map(|t| t.address.clone()).collect::<Vec<_>>().join(", ");
    raw.command = format!("{} line(s)", targets.iter().map(|t| t.commands.len()).sum::<usize>());
    let created = now_ms();
    let mut change = Change {
        id,
        project_id: Some(project_id.to_string()),
        kind: Kind::Change,
        source: host.source.kind().into(),
        proposer: host.source.describe(),
        reason: args.reason.trim().to_string(),
        created_ms: created,
        deadline_ms: created + timeout_ms(host.state),
        status: ChangeStatus::Pending,
        decided_ms: None,
        decided_by: String::new(),
        note: String::new(),
        targets,
    };
    crate::applog::info("change", &format!("change {} proposed by {} for {} device(s); waiting for a decision", change.id, change.source, change.targets.len()));
    let Some(decision) = await_decision(host, hub, &mut change).await else {
        crate::applog::info("change", &format!("change {} timed out", change.id));
        return Ok(summary(&change));
    };
    let approved = match decision {
        Decision::Deny(note) => {
            change.status = ChangeStatus::Denied;
            change.note = if note.trim().is_empty() { "Denied by the engineer.".into() } else { note };
            for t in &mut change.targets {
                t.status = TargetStatus::Denied;
            }
            record(host, &change);
            crate::applog::info("change", &format!("change {} denied", change.id));
            return Ok(summary(&change));
        }
        Decision::Approve(seqs) => seqs,
    };
    change.status = ChangeStatus::Running;
    for t in &mut change.targets {
        if !approved.contains(&t.seq) {
            t.status = TargetStatus::Denied;
        }
    }
    record(host, &change);
    crate::applog::info("change", &format!("change {} approved for {} of {} device(s)", change.id, approved.len(), change.targets.len()));
    let mut stopped = false;
    for i in 0..change.targets.len() {
        if change.targets[i].status == TargetStatus::Denied {
            continue;
        }
        if stopped {
            change.targets[i].status = TargetStatus::NotRun;
            change.targets[i].message = "Not run: the batch stopped at an earlier device's failure.".into();
            continue;
        }
        let ok = run_target(host, hub, project_id, &mut change, i, &secrets).await;
        if !ok {
            stopped = true;
        }
    }
    change.status = if stopped { ChangeStatus::Failed } else { ChangeStatus::Done };
    record(host, &change);
    crate::applog::info("change", &format!("change {} finished: {}", change.id, change.status.as_str()));
    Ok(summary(&change))
}

/// One approved device: snapshot, checkpoint, push, check, confirm.
/// `false` stops the batch.
async fn run_target(host: &Host<'_>, hub: &ChangeHub, project_id: &str, change: &mut Change, i: usize, secrets: &redact::Secrets) -> bool {
    let target = change.targets[i].device.clone();
    let lock = hub.device_lock(&change.targets[i].address).await;
    let _held = lock.lock().await;
    change.targets[i].status = TargetStatus::Running;
    change.targets[i].message = "Logging in…".into();
    record(host, change);
    let mut s = match open_session(host, project_id, &target).await {
        Ok(s) => s,
        Err(e) => {
            let t = &mut change.targets[i];
            t.status = TargetStatus::PushFailed;
            t.message = format!("Nothing was changed: {}", gate(&e, secrets));
            return false;
        }
    };
    let result = run_on(host, &mut s, change, i, secrets).await;
    s.device.close().await;
    result
}

async fn run_on(host: &Host<'_>, s: &mut Session, change: &mut Change, i: usize, secrets: &redact::Secrets) -> bool {
    let before = match read_running(s).await {
        Ok(r) => r,
        Err(e) => {
            change.targets[i].status = TargetStatus::PushFailed;
            change.targets[i].message = format!("Nothing was changed: {}", gate(&e, secrets));
            return false;
        }
    };
    // What the engineer approved was a diff against the configuration
    // read for the proposal. If it has moved since, the diff is not the
    // one approved.
    let snapshot_now = gate(&before, secrets);
    if !diff::line_diff(&change.targets[i].snapshot, &snapshot_now).is_empty() {
        let t = &mut change.targets[i];
        t.status = TargetStatus::PushFailed;
        t.message = "Nothing was changed: the running configuration changed after the proposal was made, so the diff approved is no longer the diff that would apply. Propose it again.".into();
        return false;
    }
    let platform = s.platform;
    let commands = change.targets[i].commands.clone();
    let mut cli = Cli::new(&mut s.device);
    change.targets[i].message = "Saving a checkpoint…".into();
    record(host, change);
    let (mut checkpoint, prep) = platform::prepare(&mut cli, platform, &change.id, &before).await;
    change.targets[i].push.extend(gate_exchanges(prep, secrets));
    change.targets[i].checkpoint = Some(checkpoint.clone());
    change.targets[i].message = "Pushing…".into();
    record(host, change);
    let pushed = platform::push(&mut cli, platform, &checkpoint, &commands).await;
    match pushed {
        Err(failed) => {
            change.targets[i].push.extend(gate_exchanges(failed.transcript, secrets));
            // Whatever was entered before the refusal comes out again.
            let now = read_running_cli(&mut cli, platform).await.unwrap_or_else(|_| before.clone());
            let reverse = if platform.cisco_like() { diff::reverse(&diff::parse(&before), &diff::parse(&now)) } else { Vec::new() };
            let t = &mut change.targets[i];
            if diff::line_diff(&before, &now).is_empty() {
                t.status = TargetStatus::PushFailed;
                t.message = format!("A line was refused and nothing was left changed: {}", gate(&failed.why, secrets));
            } else {
                match platform::rollback(&mut cli, platform, &checkpoint, &reverse).await {
                    Ok(ex) => {
                        t.rollback.extend(gate_exchanges(ex, secrets));
                        t.status = TargetStatus::PushFailed;
                        t.message = format!("A line was refused, and what had been entered was rolled back: {}", gate(&failed.why, secrets));
                    }
                    Err(rb) => {
                        t.rollback.extend(gate_exchanges(rb.transcript, secrets));
                        t.status = TargetStatus::RollbackFailed;
                        t.message = format!("A line was refused ({}), and rolling back what had been entered failed: {}. Check the device by hand.", gate(&failed.why, secrets), gate(&rb.why, secrets));
                    }
                }
            }
            crate::applog::warn("change", &format!("change {} on device {}: push failed", change.id, i + 1));
            return false;
        }
        Ok(ex) => {
            change.targets[i].push.extend(gate_exchanges(ex, secrets));
        }
    }
    if let Checkpoint::JunosConfirm { .. } | Checkpoint::Flash { revert_timer: true, .. } = checkpoint {
        change.targets[i].confirm_by_ms = Some(now_ms() + i64::from(platform::CONFIRM_MINUTES) * 60_000);
    }
    change.targets[i].message = "Verifying…".into();
    record(host, change);
    let after_raw = read_running_cli(&mut cli, platform).await.unwrap_or_default();
    change.targets[i].actual = diff::line_diff(&change.targets[i].snapshot, &gate(&after_raw, secrets));
    // Every line took effect, where the configuration can be read that way.
    let mut checks = Vec::new();
    if platform.cisco_like() {
        match diff::took_effect(&diff::parse(&after_raw), &commands) {
            Ok(()) => checks.push(Check { command: "every line took effect".into(), ok: true, output: format!("All {} line(s) are in the running configuration.", commands.len()) }),
            Err(missing) => checks.push(Check { command: "every line took effect".into(), ok: false, output: format!("Not in the running configuration: {}", gate(&missing.join("; "), secrets)) }),
        }
    }
    for v in change.targets[i].verify_commands.clone() {
        let out = cli.run(&v).await;
        let (ok, text) = match out {
            Ok(o) => (platform::refused(&o).is_none(), o),
            Err(e) => (false, e),
        };
        checks.push(Check { command: v, ok, output: gate(&text, secrets) });
    }
    let _ = cli.take();
    let passed = checks.iter().all(|c| c.ok);
    change.targets[i].verification = checks;
    if !passed {
        let t = &mut change.targets[i];
        if t.confirm_by_ms.is_some() {
            t.status = TargetStatus::DeviceRollingBack;
            t.message = format!("Verification failed. Not confirmed, so the device rolls back by itself within {} minutes; Roll back now does it at once.", platform::CONFIRM_MINUTES);
            schedule_expiry_check(host, change.id.clone(), t.seq, t.confirm_by_ms.unwrap_or_default());
        } else {
            t.status = TargetStatus::VerifyFailed;
            t.message = "Verification failed. The change is still on the device; Roll back puts it back as it was.".into();
        }
        crate::applog::warn("change", &format!("change {} on device {}: verification failed", change.id, i + 1));
        return false;
    }
    match platform::confirm(&mut cli, platform, &mut checkpoint).await {
        Ok(ex) => {
            let t = &mut change.targets[i];
            t.push.extend(gate_exchanges(ex, secrets));
            t.checkpoint = Some(checkpoint);
            t.confirm_by_ms = None;
            t.status = TargetStatus::Verified;
            t.message = match platform.save_command() {
                Some(_) => "Pushed and verified. Not saved: saving is a separate approval.".into(),
                None => "Pushed, verified and committed.".into(),
            };
            true
        }
        Err(f) => {
            let t = &mut change.targets[i];
            t.push.extend(gate_exchanges(f.transcript, secrets));
            t.status = TargetStatus::DeviceRollingBack;
            t.message = format!("Verified, but confirming failed ({}); the device rolls back by itself when its timer runs out.", gate(&f.why, secrets));
            false
        }
    }
}

async fn read_running_cli(cli: &mut Cli<'_>, platform: Platform) -> Result<String, String> {
    let out = cli.run(platform.running_command()).await?;
    let _ = cli.take();
    Ok(out.replace("\r\n", "\n"))
}

/// After an unconfirmed commit's timer, reads the device and records
/// whether it rolled itself back.
fn schedule_expiry_check(host: &Host<'_>, id: String, seq: u32, at_ms: i64) {
    let Some(app) = host.app.clone() else { return };
    tauri::async_runtime::spawn(async move {
        let wait = (at_ms - now_ms()).max(0) as u64 + 30_000;
        tokio::time::sleep(Duration::from_millis(wait)).await;
        let _ = rollback_target(&app, &id, seq, true).await;
    });
}

/// The one-line outcome a model reads, per device.
pub fn summary(c: &Change) -> String {
    let mut out = format!("Change {} ({}): {}.", c.id, if c.kind == Kind::Save { "save" } else { "configuration change" }, describe_status(c.status));
    if !c.note.is_empty() {
        out.push_str(&format!(" {}", c.note));
    }
    for t in &c.targets {
        out.push_str(&format!("\n- {} ({}): {}. {}", t.device, t.platform_name, describe_target(t.status), t.message));
        for v in &t.verification {
            out.push_str(&format!("\n  verify `{}`: {}\n{}", v.command, if v.ok { "ok" } else { "FAILED" }, indent(&v.output)));
        }
    }
    if c.status == ChangeStatus::Pending {
        out.push_str("\nStill waiting for the engineer; call get_change_status with this id.");
    }
    out
}

fn indent(text: &str) -> String {
    text.lines().take(60).map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")
}

/// One device's outcome in words, for the assistant and the Markdown record.
pub fn describe_target(s: TargetStatus) -> &'static str {
    match s {
        TargetStatus::Pending => "waiting for the engineer's decision",
        TargetStatus::Denied => "denied; nothing was sent",
        TargetStatus::NotRun => "approved, not reached: the batch stopped before it",
        TargetStatus::Running => "running",
        TargetStatus::PushFailed => "a line was refused; what was entered was rolled back",
        TargetStatus::Verified => "pushed and verified",
        TargetStatus::VerifyFailed => "pushed, and a check failed; rollback is offered",
        TargetStatus::DeviceRollingBack => "waiting for the device to roll itself back",
        TargetStatus::RolledBack => "rolled back",
        TargetStatus::RollbackFailed => "the rollback failed",
        TargetStatus::Saved => "saved",
        TargetStatus::SaveFailed => "the save failed",
        TargetStatus::Interrupted => "interrupted when Corelens closed",
    }
}

fn describe_status(s: ChangeStatus) -> &'static str {
    match s {
        ChangeStatus::Pending => "waiting for the engineer's decision",
        ChangeStatus::Denied => "denied by the engineer; nothing was sent",
        ChangeStatus::TimedOut => "not decided in time, so denied; nothing was sent",
        ChangeStatus::Withdrawn => "withdrawn before a decision; nothing was sent",
        ChangeStatus::Running => "running",
        ChangeStatus::Done => "approved and done",
        ChangeStatus::Failed => "approved, and it failed",
        ChangeStatus::Interrupted => "interrupted when Corelens closed",
    }
}

/// `propose_save_config`, with the id the caller chose.
pub async fn propose_save_as(host: &Host<'_>, project_id: &str, arguments: &Value, id: String) -> Raw {
    let mut raw = Raw { result: Err(String::new()), device: String::new(), address: String::new(), command: String::new() };
    raw.result = propose_save_inner(host, project_id, arguments, &mut raw, Some(id)).await;
    raw
}

/// `propose_save_config`.
pub async fn propose_save(host: &Host<'_>, project_id: &str, arguments: &Value) -> Raw {
    let mut raw = Raw { result: Err(String::new()), device: String::new(), address: String::new(), command: String::new() };
    raw.result = propose_save_inner(host, project_id, arguments, &mut raw, None).await;
    raw
}

async fn propose_save_inner(host: &Host<'_>, project_id: &str, arguments: &Value, raw: &mut Raw, id: Option<String>) -> Result<String, String> {
    let hub = host.changes.ok_or("Saving configurations is not available from here.")?;
    let args: SaveArgs = tools::args("propose_save_config", arguments)?;
    if host.state.vault_key.lock().map(|k| k.is_none()).unwrap_or(true) {
        return Err("The vault in Corelens is locked, so no login can be used. The engineer unlocks it in the Corelens window, then ask again.".into());
    }
    let secrets = super::vault_secrets(host.state);
    let mut s = open_session(host, project_id, &args.target).await?;
    raw.device = s.label.clone();
    raw.address = s.address.clone();
    let Some(save_cmd) = s.platform.save_command() else {
        s.device.close().await;
        return Ok(format!("{} ({}) runs {}: a committed change is already its saved configuration, so there is nothing to save.", s.label, s.address, s.platform.name()));
    };
    raw.command = save_cmd.into();
    // What saving would write over: the startup configuration against the
    // running one.
    let running = read_running(&mut s).await.unwrap_or_default();
    let startup = s.device.run("show startup-config").await.unwrap_or_default().replace("\r\n", "\n");
    s.device.close().await;
    let predicted = diff::line_diff(&gate(&startup, &secrets), &gate(&running, &secrets));
    let created = now_ms();
    let platform = s.platform;
    let mut change = Change {
        id: id.unwrap_or_else(new_id),
        project_id: Some(project_id.to_string()),
        kind: Kind::Save,
        source: host.source.kind().into(),
        proposer: host.source.describe(),
        reason: args.reason.trim().to_string(),
        created_ms: created,
        deadline_ms: created + timeout_ms(host.state),
        status: ChangeStatus::Pending,
        decided_ms: None,
        decided_by: String::new(),
        note: String::new(),
        targets: vec![Target {
            seq: 0,
            device: s.label.clone(),
            address: s.address.clone(),
            platform,
            platform_name: platform.name().into(),
            commands: vec![save_cmd.into()],
            verify_commands: Vec::new(),
            assessment: Assessment { class: risk::Class::Medium, hard_blocks: Vec::new(), summary: "Makes the running configuration the one the device starts with; the saved configuration it replaces is gone.".into() },
            predicted,
            predicted_is_model: false,
            rollback_plan: "None: the startup configuration is overwritten.".into(),
            status: TargetStatus::Pending,
            message: String::new(),
            checkpoint: None,
            snapshot: String::new(),
            actual: Vec::new(),
            push: Vec::new(),
            verification: Vec::new(),
            rollback: Vec::new(),
            confirm_by_ms: None,
            updated_ms: created,
        }],
    };
    let Some(decision) = await_decision(host, hub, &mut change).await else {
        return Ok(summary(&change));
    };
    if let Decision::Deny(note) = decision {
        change.status = ChangeStatus::Denied;
        change.note = if note.trim().is_empty() { "Denied by the engineer.".into() } else { note };
        change.targets[0].status = TargetStatus::Denied;
        record(host, &change);
        return Ok(summary(&change));
    }
    change.status = ChangeStatus::Running;
    change.targets[0].status = TargetStatus::Running;
    change.targets[0].message = "Saving…".into();
    record(host, &change);
    let ok = save_on(host, hub, project_id, &mut change, &secrets).await;
    change.status = if ok { ChangeStatus::Done } else { ChangeStatus::Failed };
    record(host, &change);
    Ok(summary(&change))
}

async fn save_on(host: &Host<'_>, hub: &ChangeHub, project_id: &str, change: &mut Change, secrets: &redact::Secrets) -> bool {
    let lock = hub.device_lock(&change.targets[0].address).await;
    let _held = lock.lock().await;
    let target = change.targets[0].device.clone();
    let mut s = match open_session(host, project_id, &target).await {
        Ok(s) => s,
        Err(e) => {
            change.targets[0].status = TargetStatus::SaveFailed;
            change.targets[0].message = gate(&e, secrets);
            return false;
        }
    };
    let platform = s.platform;
    let mut cli = Cli::new(&mut s.device);
    let r = platform::save(&mut cli, platform).await;
    let t = &mut change.targets[0];
    let ok = match r {
        Ok(ex) => {
            t.push = gate_exchanges(ex, secrets);
            t.status = TargetStatus::Saved;
            t.message = "Saved: the running configuration is now the startup configuration.".into();
            true
        }
        Err(f) => {
            t.push = gate_exchanges(f.transcript, secrets);
            t.status = TargetStatus::SaveFailed;
            t.message = format!("Saving failed: {}", gate(&f.why, secrets));
            false
        }
    };
    s.device.close().await;
    ok
}

/// `get_change_status`.
pub async fn status(host: &Host<'_>, arguments: &Value) -> Raw {
    let mut raw = Raw { result: Err(String::new()), device: String::new(), address: String::new(), command: String::new() };
    let args: StatusArgs = match tools::args("get_change_status", arguments) {
        Ok(a) => a,
        Err(e) => {
            raw.result = Err(e);
            return raw;
        }
    };
    let load = || host.state.db.lock().ok().and_then(|c| store::get(&c, &args.change_id).ok().flatten());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(50);
    loop {
        let Some(c) = load() else {
            raw.result = Err(format!("There is no change {} in this project.", args.change_id));
            return raw;
        };
        let open = c.status.open();
        if !open || tokio::time::Instant::now() >= deadline {
            raw.device = c.targets.iter().map(|t| t.device.clone()).collect::<Vec<_>>().join(", ");
            raw.result = Ok(summary(&c));
            return raw;
        }
        match host.changes {
            Some(hub) => {
                let _ = tokio::time::timeout_at(deadline, hub.moved.notified()).await;
            }
            None => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
}

/// Rolls one device of a change back, from the window.
async fn rollback_target(app: &AppHandle, id: &str, seq: u32, by_timer: bool) -> CmdResult<Change> {
    let state = app.state::<AppState>();
    let hub = app.state::<ChangeHub>();
    let emit = |c: &Change| {
        let _ = app.emit(EVENT, c);
    };
    let noop = |_: &super::Activity| {};
    let host = Host { state: state.inner(), templates: None, ssh_port: 22, emit: &noop, changes: Some(hub.inner()), emit_change: &emit, source: super::Source::Operator, app: Some(app.clone()) };
    rollback_with(&host, id, seq, by_timer).await
}

/// Rolls one device of a change back. `by_timer` is the check after an
/// unconfirmed commit's timer, which only reads unless the device is
/// still changed.
pub(crate) async fn rollback_with(host: &Host<'_>, id: &str, seq: u32, by_timer: bool) -> CmdResult<Change> {
    let state = host.state;
    let hub = host.changes.ok_or("Rollback is not available from here.")?;
    let mut change = {
        let conn = state.db.lock().map_err(db_err)?;
        store::get(&conn, id).map_err(db_err)?.ok_or("There is no such change.")?
    };
    let project_id = change.project_id.clone().ok_or("That change belongs to no project.")?;
    let i = change.targets.iter().position(|t| t.seq == seq).ok_or("That change has no such device.")?;
    if by_timer && change.targets[i].status != TargetStatus::DeviceRollingBack {
        return Ok(change);
    }
    if !matches!(change.targets[i].status, TargetStatus::VerifyFailed | TargetStatus::Verified | TargetStatus::DeviceRollingBack | TargetStatus::RollbackFailed) {
        return Err("That device has nothing to roll back.".into());
    }
    let secrets = super::vault_secrets(state);
    let lock = hub.device_lock(&change.targets[i].address).await;
    let _held = lock.lock().await;
    let target = change.targets[i].device.clone();
    let mut s = open_session(host, &project_id, &target).await?;
    let platform = s.platform;
    let mut cli = Cli::new(&mut s.device);
    let now = read_running_cli(&mut cli, platform).await.unwrap_or_default();
    let snapshot = change.targets[i].snapshot.clone();
    if diff::line_diff(&snapshot, &gate(&now, &secrets)).is_empty() {
        let t = &mut change.targets[i];
        t.status = TargetStatus::RolledBack;
        t.message = if by_timer { "Rolled back by the device: its confirm timer ran out.".into() } else { "Already as it was before the change.".into() };
        t.confirm_by_ms = None;
        s.device.close().await;
        record(host, &change);
        return Ok(change);
    }
    let reverse = if platform.cisco_like() { diff::reverse(&diff::parse(&snapshot), &diff::parse(&gate(&now, &secrets))) } else { Vec::new() };
    if reverse.iter().any(|l| l.contains(redact::MARK)) {
        let t = &mut change.targets[i];
        t.status = TargetStatus::RollbackFailed;
        t.message = "The lines to put back hold a secret Corelens removed from its record; restore those by hand.".into();
        s.device.close().await;
        record(host, &change);
        return Ok(change);
    }
    let checkpoint = change.targets[i].checkpoint.clone().unwrap_or(Checkpoint::None);
    let r = platform::rollback(&mut cli, platform, &checkpoint, &reverse).await;
    let after = read_running_cli(&mut cli, platform).await.unwrap_or_default();
    s.device.close().await;
    let back = diff::line_diff(&snapshot, &gate(&after, &secrets)).is_empty();
    let t = &mut change.targets[i];
    match r {
        Ok(ex) => {
            t.rollback.extend(gate_exchanges(ex, &secrets));
            t.status = if back { TargetStatus::RolledBack } else { TargetStatus::RollbackFailed };
            t.message = if back { "Rolled back: the running configuration is as it was before the change.".into() } else { "Rolled back, but the configuration still differs from the snapshot; check the device.".into() };
        }
        Err(f) => {
            t.rollback.extend(gate_exchanges(f.transcript, &secrets));
            t.status = TargetStatus::RollbackFailed;
            t.message = format!("Rollback failed: {}", gate(&f.why, &secrets));
        }
    }
    t.confirm_by_ms = None;
    crate::applog::info("change", &format!("change {id} device {}: rollback {:?}", seq + 1, t.status));
    record(host, &change);
    Ok(change)
}

// --- The commands the page sends. None of them is a tool. ---

/// The open project's changes, newest first.
#[tauri::command(async)]
pub fn change_list(state: State<'_, AppState>, limit: Option<i64>) -> CmdResult<Vec<Change>> {
    let Some(project) = open_project(&state) else { return Ok(Vec::new()) };
    let conn = state.db.lock().map_err(db_err)?;
    store::list(&conn, Some(&project), limit.unwrap_or(200).clamp(1, 2000)).map_err(db_err)
}

/// The engineer's decision on a proposal: the devices approved (by
/// `seq`), and for each one with a hard block, its name typed again in
/// `typed`, in the same order as `approve`. Nothing approved is a denial.
#[tauri::command(async)]
pub fn change_decide(state: State<'_, AppState>, hub: State<'_, ChangeHub>, id: String, approve: Vec<u32>, typed: Vec<String>, note: Option<String>) -> CmdResult<()> {
    decide_as_operator(state.inner(), hub.inner(), &id, approve, &typed, note)
}

/// The engineer's decision, checked: a device with a hard block is
/// approved only with its name typed (in `typed`, in the order of
/// `approve`).
pub(crate) fn decide_as_operator(state: &AppState, hub: &ChangeHub, id: &str, approve: Vec<u32>, typed: &[String], note: Option<String>) -> CmdResult<()> {
    if approve.is_empty() {
        return hub.decide(id, Decision::Deny(note.unwrap_or_default()));
    }
    let change = {
        let conn = state.db.lock().map_err(db_err)?;
        store::get(&conn, id).map_err(db_err)?.ok_or("There is no such proposal.")?
    };
    // Approved only from the project it was proposed in.
    if change.project_id.is_none() || change.project_id != open_project(state) {
        return Err("That proposal belongs to a project other than the one that is open.".into());
    }
    if change.status != ChangeStatus::Pending || !hub.is_waiting(id) {
        return Err("That proposal is no longer waiting for a decision.".into());
    }
    check_confirmations(&change, &approve, typed)?;
    hub.decide(id, Decision::Approve(approve))
}

/// Every approved device with a hard block has its name typed again.
pub fn check_confirmations(change: &Change, approve: &[u32], typed: &[String]) -> Result<(), String> {
    for (n, seq) in approve.iter().enumerate() {
        let t = change.targets.iter().find(|t| t.seq == *seq).ok_or("That proposal has no such device.")?;
        if !t.assessment.hard_blocks.is_empty() && typed.get(n).map(|s| s.trim()) != Some(t.device.as_str()) {
            return Err(format!("{} has lines that can cut it off: type its name, {}, to approve them.", t.device, t.device));
        }
    }
    Ok(())
}

/// Rolls one device of a change back, at the engineer's click.
#[tauri::command]
pub async fn change_rollback(app: AppHandle, id: String, seq: u32) -> CmdResult<Change> {
    rollback_target(&app, &id, seq, false).await
}

/// The open project's change record, as Markdown or CSV.
#[tauri::command(async)]
pub fn change_export(state: State<'_, AppState>, format: String, id: Option<String>) -> CmdResult<String> {
    let project = open_project(&state).ok_or("Open the project whose changes you want first.")?;
    let conn = state.db.lock().map_err(db_err)?;
    let name = db::load_project(&conn, &project).map_err(db_err)?.map(|p| p.meta.name).unwrap_or_default();
    let mut all = store::list(&conn, Some(&project), 100_000).map_err(db_err)?;
    if let Some(id) = id {
        all.retain(|c| c.id == id);
    }
    all.reverse();
    match format.as_str() {
        "md" => Ok(export_md(&name, &all)),
        "csv" => Ok(export_csv(&all)),
        _ => Err("The format is md or csv.".into()),
    }
}

fn when(ms: i64) -> String {
    super::audit_time(ms)
}

pub fn export_md(project: &str, changes: &[Change]) -> String {
    let mut out = format!("# Configuration changes — {project}\n\nEvery change an AI assistant proposed through Corelens Desktop AI, what the engineer decided, and what happened. Passwords, keys and community strings are removed. Oldest first; {} change(s).\n\n", changes.len());
    for c in changes {
        out.push_str(&format!("## {} — change {} ({})\n\n- Proposed by: {} ({})\n- Reason: {}\n- Status: {}\n", when(c.created_ms), c.id, if c.kind == Kind::Save { "save" } else { "configuration" }, c.proposer, c.source, c.reason.replace('\n', " "), describe_status(c.status)));
        if let Some(d) = c.decided_ms {
            out.push_str(&format!("- Decided: {} by {}\n", when(d), c.decided_by));
        }
        if !c.note.is_empty() {
            out.push_str(&format!("- Note: {}\n", c.note));
        }
        out.push('\n');
        for t in &c.targets {
            out.push_str(&format!("### {} ({}, {})\n\n- Risk: {:?} — {}\n- Outcome: {} — {}\n", t.device, t.address, t.platform_name, t.assessment.class, t.assessment.summary, describe_target(t.status), t.message));
            for h in &t.assessment.hard_blocks {
                out.push_str(&format!("- Hard block: `{}` — {}\n", h.command.replace('`', "'"), h.risk));
            }
            out.push_str("\nCommands:\n\n```text\n");
            for l in &t.commands {
                out.push_str(&format!("{l}\n"));
            }
            out.push_str("```\n\n");
            let diffs = if t.actual.is_empty() { &t.predicted } else { &t.actual };
            if !diffs.is_empty() {
                out.push_str(&format!("{}:\n\n```diff\n{}```\n\n", if t.actual.is_empty() { "Predicted diff" } else { "Diff after the push" }, diff::render(diffs).replace("```", "'''")));
            }
            for (title, ex) in [("Push", &t.push), ("Rollback", &t.rollback)] {
                if !ex.is_empty() {
                    out.push_str(&format!("{title}:\n\n```text\n"));
                    for e in ex {
                        out.push_str(&format!("> {}\n{}\n", e.command, e.output.trim_end().replace("```", "'''")));
                    }
                    out.push_str("```\n\n");
                }
            }
            for v in &t.verification {
                out.push_str(&format!("Verify `{}`: {}\n\n```text\n{}\n```\n\n", v.command.replace('`', "'"), if v.ok { "ok" } else { "FAILED" }, v.output.trim_end().replace("```", "'''")));
            }
        }
    }
    out
}

fn csv_field(s: &str) -> String {
    let s = s.replace('\n', " ");
    // A cell that a spreadsheet would read as a formula is quoted as text.
    let s = if s.starts_with(['=', '+', '-', '@']) { format!("'{s}") } else { s };
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub fn export_csv(changes: &[Change]) -> String {
    let mut out = String::from("time,change,kind,proposer,source,reason,decision,decided_by,device,address,platform,risk,hard_blocks,commands,outcome,message\n");
    for c in changes {
        for t in &c.targets {
            let row = [
                when(c.created_ms),
                c.id.clone(),
                format!("{:?}", c.kind),
                c.proposer.clone(),
                c.source.clone(),
                c.reason.clone(),
                c.status.as_str().into(),
                c.decided_by.clone(),
                t.device.clone(),
                t.address.clone(),
                t.platform_name.clone(),
                format!("{:?}", t.assessment.class),
                t.assessment.hard_blocks.len().to_string(),
                t.commands.join(" | "),
                format!("{:?}", t.status),
                t.message.clone(),
            ];
            out.push_str(&row.iter().map(String::as_str).map(csv_field).collect::<Vec<_>>().join(","));
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_inside_a_change_is_refused_with_where_to_ask() {
        for line in ["write memory", "wr", "copy running-config startup-config", "copy run start"] {
            let e = normalise(Platform::CiscoIos, &[line.to_string()]).unwrap_err();
            assert!(e.contains("propose_save_config"), "{line}: {e}");
        }
        assert!(normalise(Platform::CiscoIos, &["do show run".into()]).is_err());
    }

    #[test]
    fn the_framing_is_left_out_and_a_multi_line_entry_is_split() {
        let out = normalise(Platform::CiscoIos, &["configure terminal".into(), "interface Gi1/0/24\n description X".into(), "end".into()]).unwrap();
        assert_eq!(out, ["interface Gi1/0/24", " description X"]);
        assert!(normalise(Platform::CiscoIos, &["conf t".into(), "end".into()]).is_err());
    }

    #[test]
    fn a_csv_cell_never_reads_as_a_formula() {
        assert_eq!(csv_field("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
    }
}
