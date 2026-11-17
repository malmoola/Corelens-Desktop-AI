//! The read-only tools over the open project.
//!
//! Plain functions over a [`Ctx`] — a database connection, the loaded
//! project and its backup folder — so they run the same from the bridge,
//! from a test, or from a future in-app chat. Each returns the text for the
//! client or a sentence saying why not; the caller redacts both.

use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::Value;

use super::project::{self, Device};
use crate::db::ProjectPackage;

pub struct Ctx<'a> {
    pub conn: &'a Connection,
    pub project: &'a ProjectPackage,
    pub backup_root: Option<PathBuf>,
    pub vault_unlocked: bool,
}

/// A tool's arguments, or an error naming the tool and the keys it was
/// given. Never a value: serde quotes the value it could not read in a
/// type error, and a value can be anything the client sent, so only a
/// missing or unknown field is passed on in serde's words.
pub(crate) fn args<T: for<'de> Deserialize<'de>>(tool: &str, v: &Value) -> Result<T, String> {
    serde_json::from_value(v.clone()).map_err(|e| {
        let said = e.to_string();
        let keys = corelens_mcp::tools::keys_of(v);
        if said.starts_with("missing field `target`") {
            return format!("{tool}: missing 'target' (the device name or IP). Keys received: {keys}.");
        }
        let why = if said.starts_with("missing field") || said.starts_with("unknown field") { said } else { "a value has the wrong type or is out of range".to_string() };
        format!("{tool}: the arguments do not match its schema ({why}). Keys received: {keys}.")
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    filter: Option<String>,
    page: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceArgs {
    target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TopologyArgs {
    target: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArgs {
    run: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigArgs {
    target: String,
    capture: Option<String>,
    from_line: Option<usize>,
    max_lines: Option<usize>,
}

const PAGE: usize = 200;

/// A crawl run as listed: id, when, seed, devices reached, status.
type CrawlRow = (String, i64, String, i64, String);
/// A collection run as listed: id, when, seed, status, devices.
type CollectionRow = (String, i64, String, String, i64);
/// A crawl run in full: id, when, seed, status, its stored result.
type CrawlRun = (String, i64, String, String, Value);

fn when(ms: i64) -> String {
    crate::mcp::audit_time(ms)
}

/// The read tools, by name. `run_show_command` is not here: it is async
/// and reaches a device (`show.rs`).
pub fn run(ctx: &Ctx, tool: &str, arguments: &Value) -> Result<String, String> {
    match tool {
        "corelens_status" => {
            args::<NoArgs>(tool, arguments)?;
            status(ctx)
        }
        "list_devices" => {
            let a: ListArgs = args(tool, arguments)?;
            Ok(list_devices(ctx, a.filter.as_deref(), a.page.unwrap_or(1)))
        }
        "search" => {
            let a: SearchArgs = args(tool, arguments)?;
            search(ctx, &a.query)
        }
        "get_device" => {
            let a: DeviceArgs = args(tool, arguments)?;
            get_device(ctx, &a.target)
        }
        "get_topology" => {
            let a: TopologyArgs = args(tool, arguments)?;
            get_topology(ctx, a.target.as_deref())
        }
        "get_discovery_results" => {
            let a: RunArgs = args(tool, arguments)?;
            discovery_results(ctx, a.run.as_deref())
        }
        "get_config" => {
            let a: ConfigArgs = args(tool, arguments)?;
            get_config(ctx, &a.target, a.capture.as_deref(), a.from_line.unwrap_or(1), a.max_lines.unwrap_or(400).min(400))
        }
        "get_findings" => {
            args::<NoArgs>(tool, arguments)?;
            findings(ctx)
        }
        other => Err(format!("{other} is not a tool this version of Corelens has.")),
    }
}

fn devices(ctx: &Ctx) -> Vec<Device> {
    let mut all = project::devices(&ctx.project.document);
    project::with_discovered_names(&mut all, &super::discovered_names(ctx.conn, &ctx.project.meta.id));
    all
}

fn status(ctx: &Ctx) -> Result<String, String> {
    let m = &ctx.project.meta;
    let devs = devices(ctx);
    let links = project::links(&ctx.project.document);
    let mut out = format!("Project open in Corelens: {}", m.name);
    for (label, v) in [("customer", &m.customer), ("site", &m.site), ("ticket", &m.ticket)] {
        if !v.is_empty() {
            out.push_str(&format!(" · {label} {v}"));
        }
    }
    out.push_str(&format!(
        "\n{} device(s), {} link(s), {} with a saved login.\nVault: {}.\n",
        devs.len(),
        links.len(),
        devs.iter().filter(|d| d.has_login()).count(),
        if ctx.vault_unlocked { "unlocked — run_show_command can log in" } else { "locked — run_show_command needs the engineer to unlock it in Corelens first" }
    ));
    let crawls = crawl_runs(ctx.conn, &m.id, 3).map_err(|e| e.to_string())?;
    if crawls.is_empty() {
        out.push_str("No discovery runs yet.\n");
    } else {
        out.push_str("Discovery runs (newest first):\n");
        for (id, at, seed, n, status) in crawls {
            out.push_str(&format!("- {id}: {} from {seed}, {n} device(s), {status}\n", when(at)));
        }
    }
    let collections = collection_runs(ctx.conn, &m.id, 3).map_err(|e| e.to_string())?;
    if !collections.is_empty() {
        out.push_str("Collection runs (newest first):\n");
        for (id, at, seed, status, n) in collections {
            out.push_str(&format!("- {id}: {} from {seed}, {n} device(s), {status}\n", when(at)));
        }
    }
    out.push_str(match &ctx.backup_root {
        Some(_) => "Backups: a backup folder is set; get_config reads it.\n",
        None => "Backups: no backup folder is set for this project, so get_config has nothing to read.\n",
    });
    out.push_str(&format!("Corelens Desktop AI {}, tool list {}.\n", env!("CARGO_PKG_VERSION"), corelens_mcp::tools::fingerprint()));
    Ok(out)
}

fn list_devices(ctx: &Ctx, filter: Option<&str>, page: usize) -> String {
    let wanted = filter.map(|f| f.trim().to_lowercase()).filter(|f| !f.is_empty());
    let all: Vec<Device> = devices(ctx).into_iter().filter(|d| wanted.as_ref().map(|w| d.haystack().contains(w.as_str())).unwrap_or(true)).collect();
    if all.is_empty() {
        return match wanted {
            Some(w) => format!("No device of the open project matches \"{w}\"."),
            None => "The open project has no devices yet.".into(),
        };
    }
    let page = page.max(1);
    let pages = all.len().div_ceil(PAGE);
    let start = (page - 1) * PAGE;
    if start >= all.len() {
        return format!("There are {} device(s), {} page(s) of {PAGE}; page {page} is past the end.", all.len(), pages);
    }
    let mut out = format!("{} device(s){}; page {page} of {pages}:\n", all.len(), wanted.map(|w| format!(" matching \"{w}\"")).unwrap_or_default());
    for d in all.iter().skip(start).take(PAGE) {
        out.push_str(&format!("- {}\n", d.summary()));
    }
    out
}

fn inventory_lines(d: &Device) -> Vec<String> {
    let mut out = Vec::new();
    let Some(inv) = d.data.get("inventory") else { return out };
    let s = |v: &Value, k: &str| v.get(k).map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).unwrap_or_default();
    if let Some(ports) = inv.get("ports").and_then(Value::as_array) {
        out.push(format!("Ports ({}):", ports.len()));
        for p in ports {
            let mut line = format!("  {} {}", s(p, "port"), s(p, "status"));
            for k in ["speed", "duplex", "mode"] {
                let v = s(p, k);
                if !v.is_empty() {
                    line.push_str(&format!(" {v}"));
                }
            }
            if let Some(v) = p.get("vlan").filter(|v| !v.is_null()) {
                line.push_str(&format!(" vlan {v}"));
            }
            let t = s(p, "trunkVlans");
            if !t.is_empty() {
                line.push_str(&format!(" vlans {t}"));
            }
            let desc = s(p, "description");
            if !desc.is_empty() {
                line.push_str(&format!(" \"{desc}\""));
            }
            if let Some(e) = p.get("errors").and_then(Value::as_object) {
                let nonzero: Vec<String> = e.iter().filter(|(_, v)| v.as_i64().unwrap_or(0) > 0).map(|(k, v)| format!("{k} {v}")).collect();
                if !nonzero.is_empty() {
                    line.push_str(&format!(" errors: {}", nonzero.join(", ")));
                }
            }
            out.push(line);
        }
    }
    if let Some(vlans) = inv.get("vlans").and_then(Value::as_array).filter(|v| !v.is_empty()) {
        out.push(format!("VLANs: {}", vlans.iter().map(|v| format!("{} {}", s(v, "id"), s(v, "name"))).collect::<Vec<_>>().join(", ")));
    }
    if let Some(routes) = inv.get("routes").and_then(Value::as_array).filter(|v| !v.is_empty()) {
        out.push(format!("Routes ({}):", routes.len()));
        for r in routes {
            let hops = r.get("nextHops").and_then(Value::as_array).map(|h| h.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            out.push(format!("  {} {} via {}{}", s(r, "prefix"), s(r, "protocol"), if hops.is_empty() { "-".into() } else { hops }, Some(s(r, "interface")).filter(|i| !i.is_empty()).map(|i| format!(" ({i})")).unwrap_or_default()));
        }
    }
    if let Some(stp) = inv.get("spanningTree").and_then(Value::as_array).filter(|v| !v.is_empty()) {
        out.push("Spanning tree:".into());
        for i in stp {
            let blocked = i.get("blocked").and_then(Value::as_array).map(|b| b.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            out.push(format!(
                "  {}: root {}{}{}",
                s(i, "instance"),
                s(i, "rootBridge"),
                if i.get("isRoot").and_then(Value::as_bool) == Some(true) { " (this device)".to_string() } else { format!(", root port {}", s(i, "rootPort")) },
                if blocked.is_empty() { String::new() } else { format!(", blocking {blocked}") }
            ));
        }
    }
    out
}

fn get_device(ctx: &Ctx, name: &str) -> Result<String, String> {
    let all = devices(ctx);
    let d = project::resolve(&all, name)?;
    let mut out = format!("{} [{}] on page {}\n", d.label(), d.id, d.page);
    for (key, label) in [
        ("deviceType", "Type"),
        ("hostname", "Hostname"),
        ("dnsName", "DNS name"),
        ("vendor", "Vendor"),
        ("model", "Model"),
        ("osVersion", "OS"),
        ("serial", "Serial"),
        ("mac", "MAC"),
        ("site", "Site"),
        ("rack", "Rack"),
        ("role", "Role"),
        ("stackKind", "Stack"),
        ("stackMembers", "Stack members"),
        ("discoveredVia", "Discovered via"),
    ] {
        let v = d.field(key);
        if !v.is_empty() {
            out.push_str(&format!("{label}: {v}\n"));
        }
    }
    for (l, a) in d.addresses() {
        out.push_str(&format!("Address: {a}{}\n", if l.is_empty() { String::new() } else { format!(" ({l})") }));
    }
    out.push_str(&format!("Login: {}\n", if d.has_login() { "saved" } else { "none saved for this device (the project's logins are tried)" }));
    let notes = d.field("notes");
    if !notes.is_empty() {
        out.push_str(&format!("Notes: {notes}\n"));
    }
    let inv = inventory_lines(d);
    if inv.is_empty() {
        out.push_str("No discovery has read this device's ports yet.\n");
    } else {
        out.push_str(&inv.join("\n"));
        out.push('\n');
    }
    Ok(out)
}

fn get_topology(ctx: &Ctx, scope: Option<&str>) -> Result<String, String> {
    let all = devices(ctx);
    let only = match scope.map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => Some(project::resolve(&all, name)?.id.clone()),
        None => None,
    };
    let name_of = |id: &str| all.iter().find(|d| d.id == id).map(|d| d.label().to_string()).unwrap_or_else(|| id.to_string());
    let links: Vec<_> = project::links(&ctx.project.document)
        .into_iter()
        .filter(|l| only.as_ref().map(|o| &l.source == o || &l.target == o).unwrap_or(true))
        .collect();
    let mut out = format!("{} link(s){}:\n", links.len(), only.as_ref().map(|o| format!(" of {}", name_of(o))).unwrap_or_default());
    for l in &links {
        let port = |k: &str| l.data.get(k).and_then(Value::as_str).filter(|p| !p.is_empty()).map(|p| format!(" {p}")).unwrap_or_default();
        let kind = if l.data.get("overlay").and_then(Value::as_bool) == Some(true) {
            " (tunnel)"
        } else if l.data.get("layer3").and_then(Value::as_bool) == Some(true) {
            " (layer 3)"
        } else {
            ""
        };
        let label = l.data.get("label").and_then(Value::as_str).filter(|s| !s.is_empty()).map(|s| format!(" \"{s}\"")).unwrap_or_default();
        out.push_str(&format!("- {}{} — {}{}{kind}{label}\n", name_of(&l.source), port("sourcePortLabel"), name_of(&l.target), port("targetPortLabel")));
    }
    let neighbours = latest_neighbours(ctx, only.as_ref().and_then(|o| all.iter().find(|d| &d.id == o))).map_err(|e| e.to_string())?;
    if !neighbours.is_empty() {
        out.push_str(&format!("Neighbours seen by the latest collection run ({}):\n", neighbours.len()));
        for n in neighbours {
            out.push_str(&format!("- {n}\n"));
        }
    }
    Ok(out)
}

/// CDP/LLDP neighbours from the newest collection run, as lines.
fn latest_neighbours(ctx: &Ctx, of: Option<&Device>) -> rusqlite::Result<Vec<String>> {
    let Some(run) = ctx
        .conn
        .query_row("SELECT id FROM collection_runs WHERE project_id = ?1 AND plan_only = 0 ORDER BY started_ms DESC LIMIT 1", params![ctx.project.meta.id], |r| r.get::<_, String>(0))
        .optional()?
    else {
        return Ok(Vec::new());
    };
    let hosts: Vec<String> = of.map(|d| d.addresses().into_iter().map(|(_, a)| a).chain([d.label().to_string(), d.field("hostname").to_string()]).filter(|h| !h.is_empty()).collect()).unwrap_or_default();
    let mut stmt = ctx.conn.prepare(
        "SELECT cd.host, n.local_if, n.proto, n.rem_sysname, n.rem_port_id, n.rem_mgmt_ip, n.rem_platform
         FROM d_neighbor n JOIN collection_devices cd ON cd.run_id = n.run_id AND cd.device_id = n.device_id
         WHERE n.run_id = ?1 ORDER BY cd.host, n.local_if LIMIT 2000",
    )?;
    let rows = stmt.query_map(params![run], |r| {
        let g = |i: usize| r.get::<_, Option<String>>(i).map(|v| v.unwrap_or_default());
        Ok((g(0)?, g(1)?, g(2)?, g(3)?, g(4)?, g(5)?, g(6)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (host, local_if, proto, sys, port, ip, platform) = row?;
        if !hosts.is_empty() && !hosts.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
            continue;
        }
        let mut line = format!("{host} {local_if} —{}→ {sys} {port}", if proto.is_empty() { String::new() } else { format!(" {proto} ") });
        if !ip.is_empty() {
            line.push_str(&format!(" ({ip})"));
        }
        if !platform.is_empty() {
            line.push_str(&format!(" [{platform}]"));
        }
        out.push(line);
    }
    Ok(out)
}

fn crawl_runs(conn: &Connection, project: &str, limit: i64) -> rusqlite::Result<Vec<CrawlRow>> {
    let mut stmt = conn.prepare("SELECT id, taken_at, seed, devices, status FROM crawl_runs WHERE project_id = ?1 ORDER BY taken_at DESC LIMIT ?2")?;
    let rows = stmt.query_map(params![project, limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
    rows.collect()
}

fn collection_runs(conn: &Connection, project: &str, limit: i64) -> rusqlite::Result<Vec<CollectionRow>> {
    let mut stmt = conn.prepare(
        "SELECT r.id, r.started_ms, r.seed, r.status, (SELECT COUNT(*) FROM collection_devices d WHERE d.run_id = r.id)
         FROM collection_runs r WHERE r.project_id = ?1 AND r.plan_only = 0 ORDER BY r.started_ms DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![project, limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
    rows.collect()
}

/// A crawl run's stored summary: (id, when, seed, status, result JSON).
fn crawl_run(conn: &Connection, project: &str, run: Option<&str>) -> rusqlite::Result<Option<CrawlRun>> {
    conn.query_row(
        "SELECT id, taken_at, seed, status, result FROM crawl_runs WHERE project_id = ?1 AND (?2 IS NULL OR id = ?2) ORDER BY taken_at DESC LIMIT 1",
        params![project, run],
        |r| {
            let result: String = r.get(4)?;
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, serde_json::from_str(&result).unwrap_or(Value::Null)))
        },
    )
    .optional()
}

fn run_devices(conn: &Connection, run: &str) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare("SELECT device FROM crawl_run_devices WHERE run_id = ?1 ORDER BY seq")?;
    let rows = stmt.query_map(params![run], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).filter_map(|s| serde_json::from_str(&s).ok()).collect())
}

fn text(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn discovery_results(ctx: &Ctx, run: Option<&str>) -> Result<String, String> {
    let project = &ctx.project.meta.id;
    if let Some((id, at, seed, status, result)) = crawl_run(ctx.conn, project, run).map_err(|e| e.to_string())? {
        let mut devices = run_devices(ctx.conn, &id).map_err(|e| e.to_string())?;
        if devices.is_empty() {
            devices = result.get("devices").and_then(Value::as_array).cloned().unwrap_or_default();
        }
        let mut out = format!("Discovery run {id}: {} from {seed}, {status}.\n", when(at));
        out.push_str(&format!("Reached {} device(s):\n", devices.len()));
        for d in &devices {
            let name = [text(d, "hostname"), text(d, "name"), text(d, "address")].into_iter().find(|s| !s.is_empty()).unwrap_or_default();
            let mut line = format!("- {name}");
            for k in ["address", "platform", "model", "version"] {
                let v = text(d, k);
                if !v.is_empty() && v != name {
                    line.push_str(&format!(" · {v}"));
                }
            }
            out.push_str(&line);
            out.push('\n');
        }
        let failures = result.get("failures").and_then(Value::as_array).cloned().unwrap_or_default();
        if !failures.is_empty() {
            out.push_str(&format!("Could not reach or read {}:\n", failures.len()));
            for f in &failures {
                out.push_str(&format!("- {}: {}\n", text(f, "address"), text(f, "reason")));
            }
        }
        let not_visited = result.get("notVisited").and_then(Value::as_array).cloned().unwrap_or_default();
        if !not_visited.is_empty() {
            out.push_str(&format!("Seen as neighbours but not visited ({}):\n", not_visited.len()));
            for n in not_visited.iter().take(200) {
                let name = [text(n, "deviceId"), text(n, "systemName"), text(n, "name")].into_iter().find(|s| !s.is_empty()).unwrap_or_default();
                out.push_str(&format!("- {name}{}\n", Some(text(n, "address")).filter(|a| !a.is_empty()).map(|a| format!(" ({a})")).unwrap_or_default()));
            }
        }
        return Ok(out);
    }
    // No crawl: the catalog-driven collector's runs.
    let run_row = ctx
        .conn
        .query_row(
            "SELECT id, started_ms, seed, status FROM collection_runs WHERE project_id = ?1 AND plan_only = 0 AND (?2 IS NULL OR id = ?2) ORDER BY started_ms DESC LIMIT 1",
            params![project, run],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((id, at, seed, status)) = run_row else {
        return Err(match run {
            Some(r) => format!("There is no discovery run {r} in the open project; corelens_status lists them."),
            None => "The open project has no discovery runs yet. The engineer starts one from Discover in Corelens.".into(),
        });
    };
    let mut stmt = ctx.conn.prepare("SELECT host, COALESCE(os, ''), COALESCE(role, ''), COALESCE(failure, '') FROM collection_devices WHERE run_id = ?1 ORDER BY host").map_err(|e| e.to_string())?;
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    let mut out = format!("Collection run {id}: {} from {seed}, {status}, {} device(s):\n", when(at), rows.len());
    for (host, os, role, failure) in rows {
        out.push_str(&format!("- {host}{}{}{}\n", if os.is_empty() { String::new() } else { format!(" · {os}") }, if role.is_empty() { String::new() } else { format!(" · {role}") }, if failure.is_empty() { String::new() } else { format!(" · FAILED: {failure}") }));
    }
    Ok(out)
}

fn findings(ctx: &Ctx) -> Result<String, String> {
    let mut out = String::new();
    let devs = devices(ctx);
    if let Some((id, at, _, _, result)) = crawl_run(ctx.conn, &ctx.project.meta.id, None).map_err(|e| e.to_string())? {
        let failures = result.get("failures").and_then(Value::as_array).cloned().unwrap_or_default();
        let (keys, other): (Vec<_>, Vec<_>) = failures.iter().partition(|f| {
            let reason = text(f, "reason").to_lowercase();
            text(f, "kind").to_lowercase().contains("hostkey") || reason.contains("host key")
        });
        if !keys.is_empty() {
            out.push_str(&format!("Host keys that changed and were refused (latest run {id}, {}):\n", when(at)));
            for f in keys {
                out.push_str(&format!("- {}: {}\n", text(f, "address"), text(f, "reason")));
            }
        }
        if !other.is_empty() {
            out.push_str(&format!("Not reached or not read in the latest run ({id}, {}):\n", when(at)));
            for f in other {
                out.push_str(&format!("- {}: {}\n", text(f, "address"), text(f, "reason")));
            }
        }
    }
    let mut stmt = ctx
        .conn
        .prepare(
            "SELECT d.host, d.failure FROM collection_devices d JOIN collection_runs r ON r.id = d.run_id
             WHERE r.project_id = ?1 AND d.failure IS NOT NULL AND d.failure != '' AND r.id = (SELECT id FROM collection_runs WHERE project_id = ?1 AND plan_only = 0 ORDER BY started_ms DESC LIMIT 1)",
        )
        .map_err(|e| e.to_string())?;
    let failed: Vec<(String, String)> = stmt.query_map(params![ctx.project.meta.id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
    if !failed.is_empty() {
        out.push_str("Failed in the latest collection run:\n");
        for (host, failure) in failed {
            out.push_str(&format!("- {host}: {failure}\n"));
        }
    }
    let no_login: Vec<&Device> = devs.iter().filter(|d| !d.has_login()).collect();
    if !no_login.is_empty() {
        out.push_str(&format!("Devices with no login of their own ({}; the project's logins are tried in turn):\n", no_login.len()));
        for d in no_login {
            out.push_str(&format!("- {}\n", d.label()));
        }
    }
    let no_address: Vec<&Device> = devs.iter().filter(|d| d.target().is_none()).collect();
    if !no_address.is_empty() {
        out.push_str("Devices with no address, which nothing can reach:\n");
        for d in no_address {
            out.push_str(&format!("- {}\n", d.label()));
        }
    }
    if out.is_empty() {
        out.push_str("Nothing to report: no failures in the latest runs, and every device has an address and a login.\n");
    }
    Ok(out)
}

/// A device's captures, newest first, under any of the names a backup
/// may have filed it under.
pub(crate) fn captures(root: &std::path::Path, d: &Device) -> Vec<PathBuf> {
    let address = d.target().unwrap_or_default();
    for name in [d.label(), d.field("hostname"), address.as_str()] {
        if name.is_empty() {
            continue;
        }
        let found = corelens_discover::capture::list_captures(root, name, &address);
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

pub(crate) fn file_name(p: &std::path::Path) -> String {
    p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
}

fn get_config(ctx: &Ctx, name: &str, capture: Option<&str>, from: usize, max: usize) -> Result<String, String> {
    let all = devices(ctx);
    let d = project::resolve(&all, name)?;
    let root = ctx.backup_root.as_ref().ok_or("No backup folder is set for the open project, so there are no configurations to read. The engineer chooses one in Corelens under the project's settings.")?;
    let found = captures(root, d);
    if found.is_empty() {
        return Err(format!("Corelens has no backed-up configuration of {} yet. The engineer can take one with Backup in Corelens; run_show_command can read it live.", d.label()));
    }
    let chosen = match capture.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => found.iter().find(|p| file_name(p) == c).ok_or_else(|| format!("{} has no capture named {c}. Its captures: {}", d.label(), found.iter().take(20).map(|p| file_name(p)).collect::<Vec<_>>().join(", ")))?,
        None => found.iter().find(|p| file_name(p).to_lowercase().contains("run")).unwrap_or(&found[0]),
    };
    let body = std::fs::read_to_string(chosen).map_err(|e| format!("Could not read {}: {e}", file_name(chosen)))?;
    let lines: Vec<&str> = body.lines().collect();
    let from = from.max(1);
    if from > lines.len() && !lines.is_empty() {
        return Err(format!("{} has {} lines; from_line {from} is past the end.", file_name(chosen), lines.len()));
    }
    let end = (from - 1 + max).min(lines.len());
    let mut out = format!("{} — {} (lines {from}–{end} of {}):\n", d.label(), file_name(chosen), lines.len());
    for line in &lines[from - 1..end] {
        out.push_str(line);
        out.push('\n');
    }
    if end < lines.len() {
        out.push_str(&format!("[... {} more lines: ask again with from_line {}]\n", lines.len() - end, end + 1));
    }
    let others: Vec<String> = found.iter().filter(|p| *p != chosen).take(20).map(|p| file_name(p)).collect();
    if !others.is_empty() {
        out.push_str(&format!("Other captures of {} (newest first): {}\n", d.label(), others.join(", ")));
    }
    Ok(out)
}

fn search(ctx: &Ctx, query: &str) -> Result<String, String> {
    let q = query.trim().to_lowercase();
    if q.len() < 2 {
        return Err("Search for at least two characters.".into());
    }
    const LIMIT: usize = 50;
    let mut hits: Vec<String> = Vec::new();
    let devs = devices(ctx);
    for d in &devs {
        if d.haystack().contains(&q) {
            hits.push(format!("device: {}", d.summary()));
        }
        if let Some(ports) = d.data.get("inventory").and_then(|i| i.get("ports")).and_then(Value::as_array) {
            for p in ports {
                let desc = text(p, "description");
                let port = text(p, "port");
                if desc.to_lowercase().contains(&q) || port.to_lowercase() == q {
                    hits.push(format!("port: {} {port} \"{desc}\"", d.label()));
                }
            }
        }
        if let Some(vlans) = d.data.get("inventory").and_then(|i| i.get("vlans")).and_then(Value::as_array) {
            for v in vlans {
                let line = format!("{} {}", v.get("id").map(Value::to_string).unwrap_or_default(), text(v, "name"));
                if line.to_lowercase().split_whitespace().any(|w| w == q) || text(v, "name").to_lowercase().contains(&q) {
                    hits.push(format!("vlan: {} {line}", d.label()));
                }
            }
        }
    }
    for l in project::links(&ctx.project.document) {
        let label = text(&l.data, "label");
        if !label.is_empty() && label.to_lowercase().contains(&q) {
            hits.push(format!("link: \"{label}\""));
        }
    }
    if let Some(root) = &ctx.backup_root {
        for d in &devs {
            if hits.len() >= LIMIT {
                break;
            }
            let Some(newest) = captures(root, d).into_iter().next() else { continue };
            let Ok(body) = std::fs::read_to_string(&newest) else { continue };
            for (i, line) in body.lines().enumerate() {
                if line.to_lowercase().contains(&q) {
                    hits.push(format!("config: {} {} line {}: {}", d.label(), file_name(&newest), i + 1, line.trim()));
                    if hits.len() >= LIMIT {
                        break;
                    }
                }
            }
        }
    }
    if let Some((id, _, _, _, result)) = crawl_run(ctx.conn, &ctx.project.meta.id, None).map_err(|e| e.to_string())? {
        for f in result.get("failures").and_then(Value::as_array).into_iter().flatten() {
            let line = format!("{}: {}", text(f, "address"), text(f, "reason"));
            if line.to_lowercase().contains(&q) {
                hits.push(format!("discovery failure ({id}): {line}"));
            }
        }
    }
    if hits.is_empty() {
        return Ok(format!("Nothing in the open project matches \"{query}\"."));
    }
    let more = hits.len().saturating_sub(LIMIT);
    let mut out = format!("{} match(es) for \"{query}\":\n", hits.len().min(LIMIT));
    for h in hits.into_iter().take(LIMIT) {
        out.push_str(&format!("- {h}\n"));
    }
    if more > 0 {
        out.push_str(&format!("[... {more} more: search for something narrower]\n"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::project::fixture;
    use serde_json::json;

    fn package() -> ProjectPackage {
        serde_json::from_value(json!({
            "meta": { "id": "p1", "name": "Example project", "customer": "Example Co", "site": "HQ", "created_at": 1, "updated_at": 1 },
            "document_version": 1,
            "document": fixture::document()
        }))
        .unwrap()
    }

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_tests(&c);
        c
    }

    fn ctx<'a>(c: &'a Connection, p: &'a ProjectPackage, root: Option<PathBuf>) -> Ctx<'a> {
        Ctx { conn: c, project: p, backup_root: root, vault_unlocked: false }
    }

    #[test]
    fn get_device_takes_the_hostname_a_crawl_or_a_collection_found() {
        let c = conn();
        let p = package();
        assert!(run(&ctx(&c, &p, None), "get_device", &json!({ "target": "access-switch" })).is_err(), "nothing found it yet");
        crate::db::upsert_project(&c, &p).unwrap();
        // A collection run met SW1's address with the prompt `access-switch#`;
        // a crawl named it `core-crawled`; neither changed the drawing.
        c.execute_batch(
            "INSERT INTO collection_runs (id, project_id, seed, started_ms, status) VALUES ('r1', 'p1', '192.0.2.10', 1, 'complete');
             INSERT INTO collection_devices (run_id, device_id, host, prompt, collected_at) VALUES ('r1', 'd1', '192.0.2.10', 'access-switch#', 1);
             INSERT INTO crawl_runs (id, project_id, taken_at, result) VALUES ('c1', 'p1', 1, '{}');
             INSERT INTO crawl_run_devices (run_id, seq, device) VALUES ('c1', 0, '{\"hostname\":\"core-crawled\",\"address\":\"192.0.2.10\"}');",
        )
        .unwrap();
        for name in ["access-switch", "Access-Switch", "core-crawled"] {
            let out = run(&ctx(&c, &p, None), "get_device", &json!({ "target": name })).unwrap();
            assert!(out.starts_with("SW1 [n1]"), "{name}: {out}");
        }
    }

    #[test]
    fn status_names_the_project_and_what_is_missing() {
        let c = conn();
        let p = package();
        let out = run(&ctx(&c, &p, None), "corelens_status", &json!({})).unwrap();
        assert!(out.starts_with("Project open in Corelens: Example project · customer Example Co · site HQ"), "{out}");
        assert!(out.contains("3 device(s), 1 link(s), 1 with a saved login"));
        assert!(out.contains("Vault: locked"));
        assert!(out.contains("No discovery runs yet"));
        assert!(out.contains("no backup folder is set"));
        assert!(out.contains(&format!("tool list {}", corelens_mcp::tools::fingerprint())), "the build is named: {out}");
    }

    #[test]
    fn devices_list_filter_and_page() {
        let c = conn();
        let p = package();
        let cx = ctx(&c, &p, None);
        let all = run(&cx, "list_devices", &json!({})).unwrap();
        assert!(all.starts_with("3 device(s); page 1 of 1:"), "{all}");
        let fw = run(&cx, "list_devices", &json!({ "filter": "fortinet" })).unwrap();
        assert!(fw.contains("FW1") && !fw.contains("SW1"), "{fw}");
        assert!(run(&cx, "list_devices", &json!({ "page": 2 })).unwrap().contains("past the end"));
        assert!(run(&cx, "list_devices", &json!({ "filter": 1 })).unwrap_err().contains("do not match its schema"));
        assert!(run(&cx, "list_devices", &json!({ "everything": true })).unwrap_err().contains("do not match its schema"));
    }

    #[test]
    fn a_device_in_full_and_one_outside_the_project_refused() {
        let c = conn();
        let p = package();
        let cx = ctx(&c, &p, None);
        let out = run(&cx, "get_device", &json!({ "target": "192.0.2.10" })).unwrap();
        for want in ["SW1 [n1] on page Core", "Model: WS-C2960X-24TS-L", "Address: 192.0.2.10 (Management)", "Gi1/0/2 down vlan 20 \"printer\" errors: crc 12, drops 3", "VLANs: 10 users, 20 printers", "0.0.0.0/0 static via 192.0.2.1", "VLAN0010: root RTR1, root port Gi1/0/1"] {
            assert!(out.contains(want), "missing {want}:\n{out}");
        }
        let err = run(&cx, "get_device", &json!({ "target": "203.0.113.50" })).unwrap_err();
        assert!(err.contains("not a device of the project open in Corelens"));
    }

    #[test]
    fn topology_names_both_ends_and_their_ports() {
        let c = conn();
        let p = package();
        let cx = ctx(&c, &p, None);
        let out = run(&cx, "get_topology", &json!({})).unwrap();
        assert!(out.contains("- SW1 Gi1/0/1 — RTR1 Gi0/0"), "{out}");
        assert!(run(&cx, "get_topology", &json!({ "target": "FW1" })).unwrap().starts_with("0 link(s) of FW1"));
    }

    #[test]
    fn discovery_results_and_findings_from_a_run() {
        let c = conn();
        let p = package();
        c.execute("INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p1', 'Example project', 1, 1)", []).unwrap();
        let result = json!({ "failures": [
            { "address": "192.0.2.50", "reason": "did not answer within 10s", "kind": "unreachable" },
            { "address": "192.0.2.51", "reason": "host key changed and the connection was refused", "kind": "hostKey" }
        ], "notVisited": [{ "deviceId": "AP-7", "address": "192.0.2.77" }], "cancelled": false });
        c.execute("INSERT INTO crawl_runs (id, project_id, taken_at, seed, devices, result, status) VALUES ('r1', 'p1', 1761264000000, '192.0.2.10', 1, ?1, 'complete')", params![result.to_string()]).unwrap();
        c.execute("INSERT INTO crawl_run_devices (run_id, seq, device) VALUES ('r1', 0, ?1)", params![json!({ "hostname": "SW1", "address": "192.0.2.10", "platform": "cisco_ios" }).to_string()]).unwrap();
        let cx = ctx(&c, &p, None);
        let out = run(&cx, "get_discovery_results", &json!({})).unwrap();
        assert!(out.contains("Discovery run r1:") && out.contains("- SW1 · 192.0.2.10 · cisco_ios"), "{out}");
        assert!(out.contains("- 192.0.2.50: did not answer within 10s"));
        assert!(out.contains("- AP-7 (192.0.2.77)"));
        assert!(run(&cx, "get_discovery_results", &json!({ "run": "nope" })).unwrap_err().contains("no discovery run nope"));
        let f = run(&cx, "get_findings", &json!({})).unwrap();
        assert!(f.contains("Host keys that changed") && f.contains("192.0.2.51"), "{f}");
        assert!(f.contains("Devices with no login of their own (2"));
        let s = run(&cx, "search", &json!({ "query": "did not answer" })).unwrap();
        assert!(s.contains("discovery failure (r1): 192.0.2.50"), "{s}");
    }

    #[test]
    fn configs_by_line_range_and_search_reads_them() {
        let c = conn();
        let p = package();
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("SW1");
        std::fs::create_dir_all(&dir).unwrap();
        let body: String = (1..=450).map(|i| if i == 3 { "interface Vlan10\n".to_string() } else { format!("! line {i}\n") }).collect();
        std::fs::write(dir.join("SW1_show-running-config_20261024-0900.txt"), body).unwrap();
        let cx = ctx(&c, &p, Some(root.path().to_path_buf()));
        let first = run(&cx, "get_config", &json!({ "target": "SW1" })).unwrap();
        assert!(first.contains("(lines 1–400 of 450)") && first.contains("ask again with from_line 401"), "{}", &first[..200]);
        let tail = run(&cx, "get_config", &json!({ "target": "SW1", "from_line": 441 })).unwrap();
        assert!(tail.contains("lines 441–450 of 450") && tail.contains("! line 450"));
        assert!(run(&cx, "get_config", &json!({ "target": "RTR1" })).unwrap_err().contains("no backed-up configuration of RTR1"));
        assert!(run(&cx, "get_config", &json!({ "target": "SW1", "capture": "../../etc/passwd" })).unwrap_err().contains("has no capture named"));
        let s = run(&cx, "search", &json!({ "query": "vlan10" })).unwrap();
        assert!(s.contains("config: SW1 SW1_show-running-config_20261024-0900.txt line 3: interface Vlan10"), "{s}");
        assert!(s.contains("vlan: SW1 10 users") || s.contains("device:") || s.contains("config:"));
    }
}
