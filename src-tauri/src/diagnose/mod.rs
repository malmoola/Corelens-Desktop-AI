//! Diagnose network, from the app's side: a read-only sweep of the devices
//! a scope names, judged by the playbooks (`corelens_diagnose`), stored,
//! and a fix plan whose steps wait in Corelens as change proposals.
//!
//! One engine for every way in: the `run_diagnostic` tool (an MCP client
//! or the in-app chat) and the Diagnose buttons in the window run the same
//! [`run`]. Devices are asked in parallel, a few at a time, each with its
//! own time limit; a device that cannot be reached, or drops partway, is
//! reported and the run goes on. Every command is a read the allowlist
//! passes and the playbook names. Nothing the run finds changes a device:
//! each fix becomes a pending proposal, which only the engineer's click in
//! the approval dialog runs, with every rule a proposal has — hard blocks
//! typed, saving its own approval.

pub mod pdf;
pub mod session;
pub mod store;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use corelens_diagnose::model::{DeviceInput, Report, Scope, StepStatus, Stored, TopoLink};
use corelens_diagnose::playbook::{Depth, FixKind, PathValues};
use corelens_diagnose::{Library, PathPlan};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{watch, Semaphore};

use crate::commands::{open_project, AppState};
use crate::db;
use crate::mcp::changes::{self, platform::Platform, risk, ChangeHub};
use crate::mcp::{project, redact, tools, Activity, Host, Raw, Source};
use crate::vault_commands::db_err;

type CmdResult<T> = Result<T, String>;

pub const EVENT: &str = "corelens://diagnose";
/// How many devices are asked at once.
pub const CONCURRENCY: usize = 6;
/// The most devices one run asks; a project bigger than that is diagnosed
/// a site or a selection at a time.
pub const MAX_DEVICES: usize = 300;

fn per_device(depth: Depth) -> Duration {
    match depth {
        Depth::Quick => Duration::from_secs(150),
        Depth::Full => Duration::from_secs(420),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// The runs going on, so the window can stop one.
#[derive(Default)]
pub struct DiagnoseHub {
    running: Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl DiagnoseHub {
    pub fn register(&self, id: &str) -> watch::Receiver<bool> {
        let (tx, rx) = watch::channel(false);
        if let Ok(mut m) = self.running.lock() {
            m.insert(id.to_string(), tx);
        }
        rx
    }

    pub fn cancel(&self, id: &str) -> bool {
        self.running.lock().ok().and_then(|m| m.get(id).map(|tx| tx.send(true).is_ok())).unwrap_or(false)
    }

    pub fn done(&self, id: &str) {
        if let Ok(mut m) = self.running.lock() {
            m.remove(id);
        }
    }
}

/// What the window is told while a run goes on.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub run_id: String,
    pub project_id: String,
    /// `started`, `device`, `finished`, `failed`.
    pub kind: String,
    pub scope: String,
    pub devices: Vec<String>,
    pub device: String,
    /// `waiting`, `running`, `done`, `unreachable`, `stopped`.
    pub state: String,
    pub done: usize,
    pub total: usize,
    pub command: String,
    pub summary: Vec<String>,
    pub error: String,
}

/// `run_diagnostic`'s arguments.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunArgs {
    /// `project`, `site`, `devices` or `path`.
    pub scope: String,
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub targets: Option<Vec<String>>,
    #[serde(default)]
    pub src: Option<String>,
    #[serde(default)]
    pub dst: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub depth: Option<String>,
    #[serde(default)]
    pub symptom: Option<String>,
}

impl RunArgs {
    fn depth(&self) -> Result<Depth, String> {
        match self.depth.as_deref().unwrap_or("quick").to_ascii_lowercase().as_str() {
            "quick" => Ok(Depth::Quick),
            "full" => Ok(Depth::Full),
            other => Err(format!("run_diagnostic: depth is quick or full, not {other}.")),
        }
    }

    fn scope(&self) -> Result<Scope, String> {
        let need = |v: &Option<String>, what: &str| v.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).ok_or_else(|| format!("run_diagnostic: scope {} needs {what}.", self.scope));
        Ok(match self.scope.trim().to_ascii_lowercase().as_str() {
            "project" => Scope::Project,
            "site" => Scope::Site { site: need(&self.site, "site")? },
            "devices" | "device" => Scope::Devices { devices: self.targets.clone().filter(|t| !t.is_empty()).ok_or("run_diagnostic: scope devices needs targets (one or more device names).")? },
            "path" => Scope::Path { src: need(&self.src, "src")?, dst: need(&self.dst, "dst")? },
            other => return Err(format!("run_diagnostic: scope is project, site, devices or path, not {other}.")),
        })
    }
}

/// A device the run will ask.
struct Target {
    device: project::Device,
    address: String,
}

/// What a scope resolves to: the devices, the project's links between
/// devices (by name), and for a path the hops.
struct Resolved {
    targets: Vec<Target>,
    links: Vec<TopoLink>,
    path: Option<(PathPlan, PathValues)>,
    note: String,
}

fn topo_links(document: &Value, all: &[project::Device]) -> Vec<TopoLink> {
    let name: BTreeMap<&str, &str> = all.iter().map(|d| (d.id.as_str(), d.label())).collect();
    project::links(document)
        .into_iter()
        .filter_map(|l| {
            let s = |k: &str| l.data.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            Some(TopoLink { a: name.get(l.source.as_str())?.to_string(), a_port: s("sourcePortLabel"), b: name.get(l.target.as_str())?.to_string(), b_port: s("targetPortLabel") })
        })
        .collect()
}

/// The device an endpoint means: a device of the project by name or
/// address, or the device the last project-wide run saw the address on.
fn endpoint<'a>(all: &'a [project::Device], previous: Option<&Report>, what: &str) -> Result<(&'a project::Device, Option<std::net::Ipv4Addr>), String> {
    if let Ok(d) = project::resolve(all, what) {
        let ip = d.target().and_then(|a| a.parse().ok());
        return Ok((d, what.parse().ok().or(ip)));
    }
    let Ok(ip) = what.trim().parse::<std::net::Ipv4Addr>() else {
        return Err(format!("\"{what}\" is not a device of the project open in Corelens, nor an IPv4 address."));
    };
    if let Some(r) = previous {
        let mut owner = None;
        for (dev, tables) in &r.tables {
            let has = |t: &str| tables.get(t).into_iter().flatten().any(|row| row.get("ip").map(|v| v.split(',').any(|x| x.trim() == what.trim())).unwrap_or(false));
            if has("ip_interfaces") || has("arp") {
                owner = Some(dev.clone());
                break;
            }
            if corelens_diagnose::path::vlan_of(&BTreeMap::from([(dev.clone(), tables.clone())]), ip).is_some() {
                owner = owner.or(Some(dev.clone()));
            }
        }
        if let Some(o) = owner {
            if let Ok(d) = project::resolve(all, &o) {
                return Ok((d, Some(ip)));
            }
        }
    }
    Err(format!("{what} is not the address of a device in the project, and no earlier diagnosis of the whole project saw it. Name the switch or gateway it sits behind, or diagnose the project first so Corelens knows where it is."))
}

fn resolve(state: &AppState, project_id: &str, scope: &Scope, port: Option<u16>) -> Result<Resolved, String> {
    let (package, previous) = {
        let conn = state.db.lock().map_err(db_err)?;
        let p = db::load_project(&conn, project_id).map_err(db_err)?.ok_or("The project open in Corelens could not be read.")?;
        let prev = store::latest_project(&conn, project_id).ok().flatten();
        (p, prev)
    };
    let all = project::devices(&package.document);
    let links = topo_links(&package.document, &all);
    let pick = |d: &project::Device| d.target().map(|address| Target { device: d.clone(), address });
    let mut note = String::new();
    let (chosen, path): (Vec<&project::Device>, Option<(PathPlan, PathValues)>) = match scope {
        Scope::Project => (all.iter().collect(), None),
        Scope::Site { site } => {
            let s: Vec<&project::Device> = all.iter().filter(|d| d.field("site").eq_ignore_ascii_case(site.trim())).collect();
            if s.is_empty() {
                return Err(format!("No device of the open project is in site \"{site}\"."));
            }
            (s, None)
        }
        Scope::Devices { devices } => {
            let mut out = Vec::new();
            for name in devices {
                let d = project::resolve(&all, name)?;
                if !out.iter().any(|x: &&project::Device| x.id == d.id) {
                    out.push(d);
                }
            }
            (out, None)
        }
        Scope::Path { src, dst } => {
            let (a, a_ip) = endpoint(&all, previous.as_ref(), src)?;
            let (b, b_ip) = endpoint(&all, previous.as_ref(), dst)?;
            let chain = corelens_diagnose::path::route(&links, a.label(), b.label()).ok_or_else(|| format!("The project's links do not join {} and {}, so Corelens cannot tell which devices are between them. Draw the links, or name the devices to diagnose instead.", a.label(), b.label()))?;
            let mut devs = Vec::new();
            for (name, _, _) in &chain {
                if let Ok(d) = project::resolve(&all, name) {
                    devs.push(d);
                }
            }
            let vlan = a_ip.and_then(|ip| previous.as_ref().and_then(|r| corelens_diagnose::path::vlan_of(&r.tables, ip)));
            let values = PathValues {
                src: a_ip.map(|i| i.to_string()).unwrap_or_default(),
                dst: b_ip.map(|i| i.to_string()).unwrap_or_default(),
                port: port.unwrap_or(443).to_string(),
                proto: "6".into(),
            };
            if values.src.is_empty() || values.dst.is_empty() {
                note = "An endpoint has no IPv4 address Corelens knows, so the route and policy lookups toward it were not asked.".into();
            } else if port.is_none() {
                note = "No port was given: the firewall policy lookups used TCP 443.".into();
            }
            (devs, Some((PathPlan { src: src.clone(), dst: dst.clone(), chain, vlan }, values)))
        }
    };
    let mut targets = Vec::new();
    let mut skipped = Vec::new();
    for d in chosen {
        match pick(d) {
            Some(t) => targets.push(t),
            None => skipped.push(d.label().to_string()),
        }
    }
    if !skipped.is_empty() {
        note = format!("{note} Without an address, not asked: {}.", skipped.join(", ")).trim().to_string();
    }
    if targets.is_empty() {
        return Err("None of those devices has an address in the project, so there is nothing to ask.".into());
    }
    if targets.len() > MAX_DEVICES {
        return Err(format!("That is {} devices; one run asks at most {MAX_DEVICES}. Diagnose a site or a selection at a time.", targets.len()));
    }
    Ok(Resolved { targets, links, path, note })
}

/// Waits until the run is stopped (never, when nobody can stop it).
async fn stopped(mut rx: watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Every future to completion, side by side, results in order.
async fn join_all<T>(futures: Vec<Pin<Box<dyn Future<Output = T> + Send + '_>>>) -> Vec<T> {
    let mut futures: Vec<Option<Pin<Box<dyn Future<Output = T> + Send + '_>>>> = futures.into_iter().map(Some).collect();
    let mut out: Vec<Option<T>> = (0..futures.len()).map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (i, slot) in futures.iter_mut().enumerate() {
            if let Some(f) = slot {
                match f.as_mut().poll(cx) {
                    std::task::Poll::Ready(v) => {
                        out[i] = Some(v);
                        *slot = None;
                    }
                    std::task::Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    out.into_iter().map(|v| v.expect("every future finished")).collect()
}

fn stored_running(state: &AppState, project_id: &str, d: &project::Device) -> Option<Stored> {
    let root = state.db.lock().ok().and_then(|c| db::project_settings(&c, project_id).ok()).and_then(|s| s.get("backupFolder").cloned()).map(std::path::PathBuf::from)?;
    let found = tools::captures(&root, d);
    let chosen = found.iter().find(|p| tools::file_name(p).to_lowercase().contains("run")).or_else(|| found.first())?;
    let text = std::fs::read_to_string(chosen).ok()?;
    Some(Stored { name: tools::file_name(chosen), text })
}

fn platform_of(ntc: &str) -> Option<Platform> {
    Some(match ntc {
        "cisco_ios" => Platform::CiscoIos,
        "cisco_nxos" => Platform::CiscoNxOs,
        "aruba_aoscx" => Platform::ArubaOsCx,
        "juniper_junos" => Platform::Junos,
        "fortinet" => Platform::FortiOs,
        "paloalto_panos" => Platform::PanOs,
        _ => return None,
    })
}

/// The change layer's own risk for each step, against what the run read.
fn assess_plan(report: &mut Report, devices: &[DeviceInput]) {
    for step in &mut report.plan {
        if step.kind == FixKind::Save {
            step.risk = "Medium: saving replaces the startup configuration".into();
            continue;
        }
        if step.commands.is_empty() {
            continue;
        }
        let Some(dev) = devices.iter().find(|d| d.name == step.device) else { continue };
        let Some(platform) = platform_of(&dev.platform) else { continue };
        let running = dev.outputs.iter().find(|o| o.command_id == "running").and_then(|o| o.text.clone());
        let path = match running {
            Some(r) if platform.cisco_like() => risk::management_path(&r, &dev.address),
            _ => risk::ManagementPath { address: dev.address.clone(), ..Default::default() },
        };
        let a = risk::assess(platform, &step.commands, &path);
        step.risk = format!("{:?}: {}", a.class, a.summary);
    }
}

/// A run, start to finish. `progress` hears about every device as it goes;
/// `cancel` stops it, keeping what was read.
pub async fn run(host: &Host<'_>, project_id: &str, args: &RunArgs, run_id: &str, progress: &(dyn Fn(&Progress) + Send + Sync), cancel: watch::Receiver<bool>) -> Result<Report, String> {
    let started = now_ms();
    let scope = args.scope()?;
    let depth = args.depth()?;
    let state = host.state;
    if state.vault_key.lock().map(|k| k.is_none()).unwrap_or(true) {
        return Err("The vault in Corelens is locked, so no login can be used and no device can be asked. The engineer unlocks it in the Corelens window, then ask again.".into());
    }
    let resolved = resolve(state, project_id, &scope, args.port)?;
    let names: Vec<String> = resolved.targets.iter().map(|t| t.device.label().to_string()).collect();
    let base = Progress { run_id: run_id.into(), project_id: project_id.into(), scope: scope.describe(), ..Default::default() };
    progress(&Progress { kind: "started".into(), devices: names.clone(), total: names.len(), ..base.clone() });
    crate::applog::info("diagnose", &format!("run {run_id}: {} device(s), {:?} run, by {}", names.len(), depth, host.source.kind()));
    let store_keys = crate::discovery::load_host_keys(state)?;
    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    let path_values = resolved.path.as_ref().map(|(_, v)| v.clone());
    let futures: Vec<Pin<Box<dyn Future<Output = DeviceInput> + Send + '_>>> = resolved
        .targets
        .iter()
        .map(|t| {
            let sem = Arc::clone(&sem);
            let store_keys = Arc::clone(&store_keys);
            let cancel = cancel.clone();
            let base = base.clone();
            let path_values = path_values.clone();
            let fut = async move {
                let label = t.device.label().to_string();
                let mut input = DeviceInput { name: label.clone(), address: t.address.clone(), role: t.device.field("role").into(), site: t.device.field("site").into(), ..Default::default() };
                let say = |state: &str, done: usize, total: usize, command: &str, error: &str| {
                    progress(&Progress { kind: "device".into(), device: label.clone(), state: state.into(), done, total, command: command.into(), error: error.into(), ..base.clone() });
                };
                say("waiting", 0, 0, "", "");
                let work = async {
                    let _slot = sem.acquire().await;
                    let (_, address, chain) = match crate::mcp::login_for(state, project_id, &label) {
                        Ok(p) => p,
                        Err(e) => return session::Collected { unreachable: Some(e), ..Default::default() },
                    };
                    for (id, _) in chain.iter().take(1) {
                        crate::vault_commands::note_use(state, id, "Diagnose: read-only commands", &address);
                    }
                    let logins: Vec<_> = chain.iter().map(|(_, c)| c.clone()).collect();
                    let on = |done: usize, total: usize, command: &str| say("running", done, total, command, "");
                    tokio::time::timeout(per_device(depth), session::collect(&label, &address, host.ssh_port, &logins, Arc::clone(&store_keys), depth, path_values.as_ref(), &on))
                        .await
                        .unwrap_or_else(|_| session::Collected { unreachable: Some(format!("{label} ({address}) did not finish within {}s; what it had answered is not kept. Is the VPN still connected?", per_device(depth).as_secs())), ..Default::default() })
                };
                let got = tokio::select! {
                    c = work => c,
                    _ = stopped(cancel) => session::Collected { unreachable: Some("The run was stopped before this device was finished.".into()), ..Default::default() },
                };
                input.platform = got.platform;
                input.outputs = got.outputs;
                input.unreachable = got.unreachable;
                if input.unreachable.is_none() {
                    input.stored = stored_running(state, project_id, &t.device);
                }
                let secrets = crate::mcp::vault_secrets(state);
                let err = input.unreachable.as_deref().map(|e| redact::gate(e, &secrets).unwrap_or_else(|_| redact::WITHHELD.into())).unwrap_or_default();
                say(if input.unreachable.is_some() { "unreachable" } else { "done" }, 0, 0, "", &err);
                input
            };
            Box::pin(fut) as Pin<Box<dyn Future<Output = DeviceInput> + Send + '_>>
        })
        .collect();
    let inputs = join_all(futures).await;
    crate::discovery::persist_host_keys_to(state, &store_keys);
    let cancelled = *cancel.borrow();
    let previous = {
        let conn = state.db.lock().map_err(db_err)?;
        store::previous(&conn, project_id, &scope.key(), started).map_err(db_err)?
    };
    let secrets = crate::mcp::vault_secrets(state);
    let redact_fn = move |s: &str| redact::gate(s, &secrets).unwrap_or_else(|_| redact::WITHHELD.to_string());
    let templates = host.templates.as_deref().and_then(corelens_diagnose::parse::templates);
    let lib = Library::embedded();
    let (path, path_note) = match resolved.path {
        Some((p, _)) => (Some(p), resolved.note.clone()),
        None => (None, resolved.note.clone()),
    };
    // Parsing is CPU work; the run already waited on the network.
    let mut report = corelens_diagnose::diagnose(
        lib,
        &corelens_diagnose::Run {
            id: run_id.into(),
            scope: scope.clone(),
            depth,
            symptom: args.symptom.clone().filter(|s| !s.trim().is_empty()),
            started_ms: started,
            finished_ms: now_ms(),
            cancelled,
            devices: &inputs,
            links: &resolved.links,
            previous: previous.as_ref(),
            templates,
            redact: &redact_fn,
            path,
            path_note,
            probe: Vec::new(),
        },
    );
    assess_plan(&mut report, &inputs);
    {
        let conn = state.db.lock().map_err(db_err)?;
        store::put(&conn, project_id, host.source.kind(), &report).map_err(db_err)?;
        let _ = store::prune(&conn, now_ms(), crate::mcp::audit::retention_days(&conn));
    }
    progress(&Progress { kind: "finished".into(), summary: report.summary.clone(), total: names.len(), devices: names, ..base });
    crate::applog::info("diagnose", &format!("run {run_id}: {} finding(s), {} cause(s), {} plan step(s){}", report.findings.len(), report.causes.len(), report.plan.len(), if cancelled { ", stopped" } else { "" }));
    Ok(report)
}

/// The arguments a plan step proposes with.
fn step_arguments(report: &Report, seq: u32) -> Option<(FixKind, Value)> {
    let step = report.plan.iter().find(|s| s.seq == seq)?;
    let cause = report.causes.iter().find(|c| c.id == step.cause).map(|c| c.title.clone()).unwrap_or_default();
    let reason = format!("Diagnosis {} — fix plan step {}: {}. Root cause: {cause}.", report.id, step.seq, step.title);
    Some(match step.kind {
        FixKind::Save => (FixKind::Save, json!({ "target": step.device, "reason": reason })),
        FixKind::Change => (FixKind::Change, json!({ "target": step.device, "commands": step.commands, "reason": reason, "verify_commands": step.verify })),
    })
}

/// Turns one plan step into a pending proposal and waits, as any proposal
/// does, for the engineer's decision. The report names the change before
/// the wait, so the window can open it.
pub async fn propose_step(host: &Host<'_>, project_id: &str, report_id: &str, seq: u32) -> Result<String, String> {
    let id = changes::fresh_id();
    let (kind, arguments) = {
        let conn = host.state.db.lock().map_err(db_err)?;
        let (_, mut report) = store::get(&conn, report_id).map_err(db_err)?.ok_or("That diagnosis is no longer kept.")?;
        let step = report.plan.iter_mut().find(|s| s.seq == seq).ok_or("That diagnosis has no such step.")?;
        if step.status == StepStatus::Manual || step.commands.is_empty() && step.kind == FixKind::Change {
            return Err("That step has no commands Corelens can propose; it is for the engineer to do by hand.".into());
        }
        step.status = StepStatus::Proposed;
        step.proposal_id = Some(id.clone());
        let out = step_arguments(&report, seq).ok_or("That diagnosis has no such step.")?;
        store::put(&conn, project_id, "diagnose", &report).map_err(db_err)?;
        out
    };
    let raw: Raw = match kind {
        FixKind::Save => changes::propose_save_as(host, project_id, &arguments, id.clone()).await,
        FixKind::Change => changes::propose_as(host, project_id, &arguments, id.clone()).await,
    };
    raw.result.map(|_| id)
}

/// Proposes every ready step of a report, each on its own task, from the
/// window's app handle.
fn spawn_plan(app: &AppHandle, project_id: &str, report: &Report) {
    for step in report.plan.iter().filter(|s| s.status == StepStatus::Ready) {
        let (app, project_id, report_id, seq) = (app.clone(), project_id.to_string(), report.id.clone(), step.seq);
        tauri::async_runtime::spawn(async move {
            let state = app.state::<AppState>();
            let hub = app.state::<ChangeHub>();
            let emit = |_: &Activity| {};
            let emit_change = |c: &changes::Change| {
                let _ = app.emit(changes::EVENT, c);
            };
            let host = Host {
                state: state.inner(),
                templates: crate::collection::templates_dir(&app).ok(),
                ssh_port: 22,
                emit: &emit,
                changes: Some(hub.inner()),
                emit_change: &emit_change,
                source: Source::Diagnose { run: report_id.clone() },
                app: Some(app.clone()),
            };
            if let Err(e) = propose_step(&host, &project_id, &report_id, seq).await {
                crate::applog::info("diagnose", &format!("run {report_id} step {seq} not proposed: {}", e.chars().take(200).collect::<String>()));
            }
            let _ = app.emit(EVENT, Progress { run_id: report_id, project_id, kind: "plan".into(), ..Default::default() });
        });
    }
}

/// `run_diagnostic`, for an MCP client or the chat.
pub async fn tool(host: &Host<'_>, project_id: &str, arguments: &Value) -> Raw {
    let mut raw = Raw { result: Err(String::new()), device: String::new(), address: String::new(), command: String::new() };
    let args: RunArgs = match tools::args("run_diagnostic", arguments) {
        Ok(a) => a,
        Err(e) => {
            raw.result = Err(e);
            return raw;
        }
    };
    let run_id = new_run_id();
    let hub = host.app.as_ref().map(|a| a.state::<DiagnoseHub>());
    let cancel = hub.as_ref().map(|h| h.register(&run_id)).unwrap_or_else(|| watch::channel(false).1);
    let app = host.app.clone();
    let emit = move |p: &Progress| {
        if let Some(a) = &app {
            let _ = a.emit(EVENT, p);
        }
    };
    let result = run(host, project_id, &args, &run_id, &emit, cancel).await;
    if let Some(h) = &hub {
        h.done(&run_id);
    }
    raw.command = args.scope.clone();
    raw.result = result.map(|report| {
        raw.device = report.devices.iter().map(|d| d.name.clone()).collect::<Vec<_>>().join(", ");
        if let Some(app) = &host.app {
            spawn_plan(app, project_id, &report);
        }
        let name = host.state.db.lock().ok().and_then(|c| db::load_project(&c, project_id).ok().flatten()).map(|p| p.meta.name).unwrap_or_default();
        let mut text = corelens_diagnose::report::for_model(&name, &report);
        if host.app.is_some() && report.plan.iter().any(|s| s.status == StepStatus::Ready) {
            text.push_str("The ready plan steps are being proposed in Corelens now, each as its own pending proposal for the engineer to approve or deny; do not propose them again.\n");
        }
        text
    });
    raw
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetArgs {
    #[serde(default)]
    diagnosis_id: Option<String>,
}

/// `get_diagnosis`: a stored report, with where each plan step's proposal
/// stands now.
pub fn get_tool(host: &Host<'_>, project_id: &str, arguments: &Value) -> Raw {
    let mut raw = Raw { result: Err(String::new()), device: String::new(), address: String::new(), command: String::new() };
    raw.result = (|| {
        let args: GetArgs = tools::args("get_diagnosis", arguments)?;
        let conn = host.state.db.lock().map_err(db_err)?;
        let report = match args.diagnosis_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => match store::get(&conn, id).map_err(db_err)? {
                Some((p, r)) if p == project_id => r,
                _ => return Err(format!("There is no diagnosis {id} in the open project.")),
            },
            None => {
                let newest = store::list(&conn, project_id, 1).map_err(db_err)?;
                let id = newest.first().map(|s| s.id.clone()).ok_or("No diagnosis has been run in the open project yet; run_diagnostic runs one.")?;
                store::get(&conn, &id).map_err(db_err)?.map(|(_, r)| r).ok_or("That diagnosis is no longer kept.")?
            }
        };
        let name = db::load_project(&conn, project_id).ok().flatten().map(|p| p.meta.name).unwrap_or_default();
        let mut text = corelens_diagnose::report::for_model(&name, &report);
        for step in report.plan.iter().filter(|s| s.proposal_id.is_some()) {
            let id = step.proposal_id.as_deref().unwrap_or("");
            let now = changes::store::get(&conn, id).ok().flatten().map(|c| c.status.as_str()).unwrap_or("not found");
            text.push_str(&format!("Plan step {} (change {id}) is {now} now.\n", step.seq));
        }
        Ok(text)
    })();
    raw
}

fn new_run_id() -> String {
    format!("d{}", &uuid::Uuid::new_v4().simple().to_string()[..7])
}

// ---- What the window calls ----

/// Starts a run from the window; its progress and report arrive as events.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn diagnose_start(
    app: AppHandle,
    scope: String,
    site: Option<String>,
    targets: Option<Vec<String>>,
    src: Option<String>,
    dst: Option<String>,
    port: Option<u16>,
    depth: Option<String>,
    symptom: Option<String>,
) -> CmdResult<String> {
    let args = RunArgs { scope, site, targets, src, dst, port, depth, symptom };
    let state = app.state::<AppState>();
    let project_id = open_project(&state).ok_or("Open a project first.")?;
    args.scope()?;
    args.depth()?;
    let run_id = new_run_id();
    let cancel = app.state::<DiagnoseHub>().register(&run_id);
    let id = run_id.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let hub = app.state::<ChangeHub>();
        let emit = |_: &Activity| {};
        let emit_change = |c: &changes::Change| {
            let _ = app.emit(changes::EVENT, c);
        };
        let host = Host {
            state: state.inner(),
            templates: crate::collection::templates_dir(&app).ok(),
            ssh_port: 22,
            emit: &emit,
            changes: Some(hub.inner()),
            emit_change: &emit_change,
            source: Source::Operator,
            app: Some(app.clone()),
        };
        let progress = |p: &Progress| {
            let _ = app.emit(EVENT, p);
        };
        match run(&host, &project_id, &args, &id, &progress, cancel).await {
            Ok(report) => spawn_plan(&app, &project_id, &report),
            Err(e) => {
                let secrets = crate::mcp::vault_secrets(&state);
                let _ = app.emit(EVENT, Progress { run_id: id.clone(), project_id, kind: "failed".into(), error: redact::gate(&e, &secrets).unwrap_or_else(|_| redact::WITHHELD.into()), ..Default::default() });
            }
        }
        app.state::<DiagnoseHub>().done(&id);
    });
    Ok(run_id)
}

#[tauri::command]
pub fn diagnose_cancel(hub: State<'_, DiagnoseHub>, run_id: String) -> CmdResult<bool> {
    Ok(hub.cancel(&run_id))
}

#[tauri::command(async)]
pub fn diagnose_list(state: State<'_, AppState>, limit: Option<i64>) -> CmdResult<Vec<store::Summary>> {
    let Some(project) = open_project(&state) else { return Ok(Vec::new()) };
    let conn = state.db.lock().map_err(db_err)?;
    store::list(&conn, &project, limit.unwrap_or(50).clamp(1, 500)).map_err(db_err)
}

#[tauri::command(async)]
pub fn diagnose_get(state: State<'_, AppState>, run_id: String) -> CmdResult<Report> {
    let project = open_project(&state).ok_or("Open a project first.")?;
    let conn = state.db.lock().map_err(db_err)?;
    match store::get(&conn, &run_id).map_err(db_err)? {
        Some((p, r)) if p == project => Ok(r),
        _ => Err("That diagnosis is not one of the open project's.".into()),
    }
}

/// A report as Markdown, for the page to save.
#[tauri::command(async)]
pub fn diagnose_export_md(state: State<'_, AppState>, run_id: String) -> CmdResult<String> {
    let report = diagnose_get(state.clone(), run_id)?;
    let name = open_project(&state).and_then(|p| state.db.lock().ok().and_then(|c| db::load_project(&c, &p).ok().flatten())).map(|p| p.meta.name).unwrap_or_default();
    Ok(corelens_diagnose::report::markdown(&name, &report))
}

/// A report as a PDF, for the page to save.
#[tauri::command(async)]
pub fn diagnose_export_pdf(state: State<'_, AppState>, run_id: String) -> CmdResult<Vec<u8>> {
    let md = diagnose_export_md(state, run_id)?;
    pdf::markdown_pdf(&md)
}

/// Proposes one step again (its proposal was denied or timed out).
#[tauri::command]
pub async fn diagnose_propose(app: AppHandle, run_id: String, seq: u32) -> CmdResult<()> {
    let state = app.state::<AppState>();
    let project_id = open_project(&state).ok_or("Open a project first.")?;
    let one = {
        let conn = state.db.lock().map_err(db_err)?;
        let (p, mut report) = store::get(&conn, &run_id).map_err(db_err)?.ok_or("That diagnosis is no longer kept.")?;
        if p != project_id {
            return Err("That diagnosis is not one of the open project's.".into());
        }
        let step = report.plan.iter_mut().find(|s| s.seq == seq).ok_or("That diagnosis has no such step.")?;
        if step.status == StepStatus::Manual {
            return Err("That step is for the engineer to do by hand.".into());
        }
        step.status = StepStatus::Ready;
        step.proposal_id = None;
        let step = step.clone();
        store::put(&conn, &project_id, "diagnose", &report).map_err(db_err)?;
        Report { plan: vec![step], ..report }
    };
    spawn_plan(&app, &project_id, &one);
    Ok(())
}

#[tauri::command(async)]
pub fn diagnose_clear(state: State<'_, AppState>) -> CmdResult<usize> {
    let Some(project) = open_project(&state) else { return Ok(0) };
    let conn = state.db.lock().map_err(db_err)?;
    store::clear(&conn, Some(&project)).map_err(db_err)
}
