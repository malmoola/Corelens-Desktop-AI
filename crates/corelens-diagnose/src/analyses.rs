//! What no single device can show: the two ends of a link compared, the
//! same VLAN's root and gateway across switches, an address claimed twice,
//! the diagram against what the devices say, and this run against the
//! last. They read the tables the playbooks fill (`interfaces`,
//! `switchport`, `neighbors`, `stp_root`, `fhrp`, `arp`, `ip_interfaces`,
//! `ospf`, `vlans`, `helpers`), so they do not care which vendor said it.
//! What each finding is called, how bad it is and which fix it takes is
//! data, in `resources/diagnose/analyses.yaml`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::{Finding, TopoLink};
use crate::playbook::{Layer, Severity};
use crate::rules::{fill, finding_id};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisSpec {
    pub id: String,
    pub title: String,
    pub layer: Layer,
    pub severity: Severity,
    pub says: String,
    #[serde(default)]
    pub key: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub fix: Option<String>,
    pub passed: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Analyses {
    pub format: u32,
    pub version: u32,
    pub analyses: Vec<AnalysisSpec>,
}

impl Analyses {
    pub fn get(&self, id: &str) -> Option<&AnalysisSpec> {
        self.analyses.iter().find(|a| a.id == id)
    }
}

/// One device's tables, in the tables' own field names.
pub type DeviceTables = BTreeMap<String, Vec<BTreeMap<String, String>>>;

/// What the analyses are given.
pub struct Input<'a> {
    /// Device name → its tables, for the devices reached this run.
    pub now: &'a BTreeMap<String, DeviceTables>,
    /// The same, from the previous run of this scope, when there was one.
    pub before: Option<&'a BTreeMap<String, DeviceTables>>,
    pub links: &'a [TopoLink],
    /// Device name → its role in the project ("core", "access", …).
    pub roles: &'a BTreeMap<String, String>,
}

/// A port's name in one spelling: `GigabitEthernet1/0/1`, `Gi1/0/1` and
/// `gi1/0/1` are the same port.
pub fn canon_port(p: &str) -> String {
    let p = p.trim().to_ascii_lowercase().replace(' ', "");
    let split = p.find(|c: char| c.is_ascii_digit()).unwrap_or(p.len());
    let (letters, rest) = p.split_at(split);
    let short = match letters {
        l if l.starts_with("gi") => "gi",
        l if l.starts_with("te") => "te",
        l if l.starts_with("fa") => "fa",
        l if l.starts_with("twe") => "twe",
        l if l.starts_with("two") || l == "tw" => "tw",
        l if l.starts_with("fo") => "fo",
        l if l.starts_with("hu") => "hu",
        l if l.starts_with("et") => "eth",
        l if l.starts_with("po") => "po",
        l if l.starts_with("vl") => "vlan",
        l if l.starts_with("mg") || l.starts_with("ma") => "mgmt",
        l => l,
    };
    format!("{short}{rest}")
}

/// A VLAN list as devices write one: `1-10,20`, `ALL`, `none`.
pub fn vlan_set(list: &str) -> BTreeSet<u16> {
    let l = list.trim().to_ascii_lowercase();
    if l.is_empty() || l == "none" {
        return BTreeSet::new();
    }
    if l == "all" || l == "1-4094" || l == "1-4095" {
        return (1..=4094).collect();
    }
    let mut out = BTreeSet::new();
    for part in l.split([',', ' ']).filter(|p| !p.is_empty()) {
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.trim().parse::<u16>(), b.trim().parse::<u16>()) {
                if a <= b && b <= 4094 {
                    out.extend(a..=b);
                }
            }
        } else if let Ok(v) = part.parse::<u16>() {
            out.insert(v);
        }
    }
    out
}

fn rows<'a>(t: &'a DeviceTables, table: &str) -> &'a [BTreeMap<String, String>] {
    t.get(table).map(Vec::as_slice).unwrap_or(&[])
}

fn get<'a>(r: &'a BTreeMap<String, String>, f: &str) -> &'a str {
    r.get(f).map(String::as_str).unwrap_or("")
}

fn port_row<'a>(t: &'a DeviceTables, table: &str, field: &str, port: &str) -> Option<&'a BTreeMap<String, String>> {
    let want = canon_port(port);
    rows(t, table).iter().find(|r| canon_port(get(r, field)) == want)
}

/// The two ends of every link both of whose devices were reached: the
/// diagram's links, and those the devices' own neighbour tables show.
fn ends(input: &Input<'_>) -> Vec<TopoLink> {
    let mut out: Vec<TopoLink> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut add = |l: TopoLink, out: &mut Vec<TopoLink>| {
        if l.a_port.is_empty() || l.b_port.is_empty() {
            return;
        }
        let mut k = [(l.a.to_lowercase(), canon_port(&l.a_port)), (l.b.to_lowercase(), canon_port(&l.b_port))];
        k.sort();
        if seen.insert(k) {
            out.push(l);
        }
    };
    for l in input.links {
        if input.now.contains_key(&l.a) && input.now.contains_key(&l.b) {
            add(l.clone(), &mut out);
        }
    }
    for (dev, t) in input.now {
        for n in rows(t, "neighbors") {
            let remote = get(n, "remote");
            if let Some(peer) = peer_named(input, remote) {
                if &peer != dev {
                    add(TopoLink { a: dev.clone(), a_port: get(n, "local_port").into(), b: peer, b_port: get(n, "remote_port").into() }, &mut out);
                }
            }
        }
    }
    out
}

/// The reached device a neighbour table's name means: `SW2`,
/// `SW2.example.test`, `SW2(FDO123)`.
fn peer_named(input: &Input<'_>, remote: &str) -> Option<String> {
    let r = remote.trim().to_ascii_lowercase();
    let base = r.split(['.', '(']).next().unwrap_or("");
    input.now.keys().find(|d| {
        let d = d.to_ascii_lowercase();
        !d.is_empty() && (d == r || d == base)
    }).cloned()
}

struct Out<'a> {
    specs: &'a Analyses,
    findings: Vec<Finding>,
    /// Analyses that ran (had something to compare) and found nothing.
    ran: BTreeSet<String>,
}

impl Out<'_> {
    fn ran(&mut self, id: &str) {
        self.ran.insert(id.to_string());
    }
    fn add(&mut self, id: &str, device: &str, fields: BTreeMap<String, String>, evidence: Vec<String>) {
        self.ran(id);
        let Some(f) = make(self.specs, id, device, fields, evidence) else { return };
        if !self.findings.iter().any(|x| x.id == f.id) {
            self.findings.push(f);
        }
    }
}

/// A finding of analysis `id`, worded as its spec says.
pub fn make(specs: &Analyses, id: &str, device: &str, fields: BTreeMap<String, String>, evidence: Vec<String>) -> Option<Finding> {
    let spec = specs.get(id)?;
    let mut ev: Vec<String> = spec.evidence.iter().filter_map(|f| fields.get(f).filter(|v| !v.is_empty()).map(|v| format!("{f}: {v}"))).collect();
    ev.extend(evidence);
    Some(Finding {
        id: finding_id(id, device, &spec.key, &fields),
        check: id.to_string(),
        title: spec.title.clone(),
        layer: spec.layer,
        severity: spec.severity,
        device: device.to_string(),
        says: fill(&spec.says, device, &fields),
        fields,
        evidence: ev,
        fix: spec.fix.clone(),
    })
}

/// The lines of a configuration that are configuration: blank lines,
/// comments, the banner the device prints around it, and what `ignore`
/// matches are not.
fn config_lines(text: &str, ignore: &[regex::Regex]) -> Vec<String> {
    text.replace("\r\n", "\n")
        .lines()
        .map(str::trim_end)
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && t != "!" && !t.starts_with("Building configuration") && !t.starts_with("Current configuration") && !t.starts_with("!Time:") && !t.starts_with("!Command:") && t != "end" && !ignore.iter().any(|r| r.is_match(l))
        })
        .map(str::to_string)
        .collect()
}

/// What one configuration has that the other lacks: `('-', line)` for a
/// line only `from` has, `('+', line)` for one only `to` has. The lines are
/// returned bare, so whoever shows them can redact each as the
/// configuration line it is before marking it.
pub fn config_diff(from: &str, to: &str, ignore: &[regex::Regex]) -> Vec<(char, String)> {
    let a = config_lines(from, ignore);
    let b = config_lines(to, ignore);
    let sa: BTreeSet<&String> = a.iter().collect();
    let sb: BTreeSet<&String> = b.iter().collect();
    let mut out: Vec<(char, String)> = a.iter().filter(|l| !sb.contains(l)).map(|l| ('-', l.clone())).collect();
    out.extend(b.iter().filter(|l| !sa.contains(l)).map(|l| ('+', l.clone())));
    out
}

fn f(pairs: &[(&str, String)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

fn is_trunk(r: &BTreeMap<String, String>) -> bool {
    get(r, "mode").to_ascii_lowercase().contains("trunk")
}

fn vlans_on(t: &DeviceTables) -> BTreeSet<u16> {
    rows(t, "vlans").iter().filter(|r| !get(r, "status").to_ascii_lowercase().contains("suspend")).filter_map(|r| get(r, "vlan").parse().ok()).collect()
}

/// Duplex as one word: `a-full` and `full` are both `full`.
fn duplex(s: &str) -> String {
    s.to_ascii_lowercase().trim_start_matches("a-").to_string()
}

fn hard_set(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    !l.starts_with("a-") && !l.starts_with("auto") && !l.is_empty()
}

fn up(status: &str) -> bool {
    matches!(status.to_ascii_lowercase().as_str(), "up" | "connected")
}

/// Every analysis, in a fixed order.
pub fn run(specs: &Analyses, input: &Input<'_>) -> (Vec<Finding>, BTreeSet<String>) {
    let mut out = Out { specs, findings: Vec::new(), ran: BTreeSet::new() };
    let links = ends(input);
    link_checks(&mut out, input, &links);
    expected_up(&mut out, input, &links);
    spanning_tree(&mut out, input);
    gateways(&mut out, input);
    addresses(&mut out, input);
    drift(&mut out, input);
    ospf(&mut out, input);
    helpers(&mut out, input);
    let ran = out.ran.iter().filter(|id| !out.findings.iter().any(|f| &&f.check == id)).cloned().collect();
    (out.findings, ran)
}

fn link_checks(out: &mut Out<'_>, input: &Input<'_>, links: &[TopoLink]) {
    for l in links {
        let (ta, tb) = (&input.now[&l.a], &input.now[&l.b]);
        if let (Some(sa), Some(sb)) = (port_row(ta, "switchport", "port", &l.a_port), port_row(tb, "switchport", "port", &l.b_port)) {
            if is_trunk(sa) && is_trunk(sb) {
                out.ran("x.trunk_vlan_mismatch");
                out.ran("x.native_vlan_mismatch");
                let (aa, ab) = (vlan_set(get(sa, "allowed")), vlan_set(get(sb, "allowed")));
                let both: BTreeSet<u16> = vlans_on(ta).intersection(&vlans_on(tb)).copied().collect();
                for (dev, port, peer, peer_port, have, lack) in [(&l.a, &l.a_port, &l.b, &l.b_port, &ab, &aa), (&l.b, &l.b_port, &l.a, &l.a_port, &aa, &ab)] {
                    for v in have.difference(lack).filter(|v| both.contains(v)) {
                        let allowed = if dev == &l.a { get(sa, "allowed") } else { get(sb, "allowed") };
                        out.add(
                            "x.trunk_vlan_mismatch",
                            dev,
                            f(&[("port", port.clone()), ("vlan", v.to_string()), ("peer", peer.clone()), ("peer_port", peer_port.clone()), ("allowed", allowed.to_string())]),
                            vec![format!("{dev} {port} allows: {allowed}"), format!("{peer} {peer_port} allows: {}", if dev == &l.a { get(sb, "allowed") } else { get(sa, "allowed") })],
                        );
                    }
                }
                let (na, nb) = (get(sa, "native"), get(sb, "native"));
                if !na.is_empty() && !nb.is_empty() && na != nb {
                    out.add(
                        "x.native_vlan_mismatch",
                        &l.a,
                        f(&[("port", l.a_port.clone()), ("native", na.into()), ("peer", l.b.clone()), ("peer_port", l.b_port.clone()), ("peer_native", nb.into())]),
                        Vec::new(),
                    );
                }
            }
        }
        if let (Some(ia), Some(ib)) = (port_row(ta, "interfaces", "port", &l.a_port), port_row(tb, "interfaces", "port", &l.b_port)) {
            if up(get(ia, "status")) && up(get(ib, "status")) {
                out.ran("x.duplex_mismatch");
                let (da, db) = (duplex(get(ia, "duplex")), duplex(get(ib, "duplex")));
                let (pa, pb) = (get(ia, "speed").to_ascii_lowercase().trim_start_matches("a-").to_string(), get(ib, "speed").to_ascii_lowercase().trim_start_matches("a-").to_string());
                let duplex_differs = !da.is_empty() && !db.is_empty() && da != db;
                let speed_differs = !pa.is_empty() && !pb.is_empty() && pa != pb && !pa.starts_with("auto") && !pb.starts_with("auto");
                if duplex_differs || speed_differs {
                    // The fix goes on the end that was set by hand (or the
                    // half-duplex one when both were).
                    let a_first = hard_set(get(ia, "duplex")) && (!hard_set(get(ib, "duplex")) || da == "half");
                    let (dev, port, peer, peer_port, mine, theirs) = if a_first { (&l.a, &l.a_port, &l.b, &l.b_port, ia, ib) } else { (&l.b, &l.b_port, &l.a, &l.a_port, ib, ia) };
                    out.add(
                        "x.duplex_mismatch",
                        dev,
                        f(&[
                            ("port", port.clone()),
                            ("duplex", get(mine, "duplex").into()),
                            ("speed", get(mine, "speed").into()),
                            ("peer", peer.clone()),
                            ("peer_port", peer_port.clone()),
                            ("peer_duplex", get(theirs, "duplex").into()),
                            ("peer_speed", get(theirs, "speed").into()),
                        ]),
                        Vec::new(),
                    );
                }
            }
        }
    }
}

fn expected_up(out: &mut Out<'_>, input: &Input<'_>, links: &[TopoLink]) {
    let mut expected: BTreeMap<(String, String), String> = BTreeMap::new();
    for l in input.links.iter().chain(links) {
        for (d, p, peer) in [(&l.a, &l.a_port, &l.b), (&l.b, &l.b_port, &l.a)] {
            if !p.is_empty() {
                expected.entry((d.clone(), canon_port(p))).or_insert_with(|| format!("it carries the link to {peer}"));
            }
        }
    }
    if let Some(before) = input.before {
        for (d, t) in before {
            for r in rows(t, "interfaces") {
                if up(get(r, "status")) {
                    expected.entry((d.clone(), canon_port(get(r, "port")))).or_insert_with(|| "it was up in the previous run".into());
                }
            }
        }
    }
    for ((dev, port), why) in &expected {
        let Some(t) = input.now.get(dev) else { continue };
        let Some(r) = rows(t, "interfaces").iter().find(|r| &canon_port(get(r, "port")) == port) else { continue };
        let status = get(r, "status").to_ascii_lowercase();
        out.ran("x.port_shut");
        out.ran("x.expected_up_down");
        let fields = f(&[("port", get(r, "port").into()), ("status", status.clone()), ("why", why.clone()), ("description", get(r, "description").into())]);
        match status.as_str() {
            "admin-down" | "disabled" => out.add("x.port_shut", dev, fields, Vec::new()),
            "down" | "notconnect" => out.add("x.expected_up_down", dev, fields, Vec::new()),
            _ => {}
        }
    }
}

fn root_of(t: &DeviceTables, vlan: &str) -> Option<BTreeMap<String, String>> {
    rows(t, "stp_root").iter().find(|r| get(r, "vlan").trim_start_matches("VLAN").trim_start_matches('0') == vlan.trim_start_matches('0')).cloned()
}

fn vlan_key(v: &str) -> String {
    v.to_ascii_uppercase().trim_start_matches("VLAN").trim_start_matches('0').to_string()
}

/// No root port (IOS), or "This bridge is root" (NX-OS).
fn this_is_root(r: &BTreeMap<String, String>) -> bool {
    let p = get(r, "root_port").trim().to_ascii_lowercase();
    p.is_empty() || p.contains("this bridge") || p == "root"
}

fn spanning_tree(out: &mut Out<'_>, input: &Input<'_>) {
    // VLAN → (root bridge → the devices that say so).
    let mut roots: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let mut is_root: BTreeMap<String, String> = BTreeMap::new();
    let mut priority: BTreeMap<String, String> = BTreeMap::new();
    for (dev, t) in input.now {
        for r in rows(t, "stp_root") {
            let v = vlan_key(get(r, "vlan"));
            roots.entry(v.clone()).or_default().entry(get(r, "root_id").to_ascii_lowercase()).or_default().push(dev.clone());
            if this_is_root(r) {
                is_root.insert(v.clone(), dev.clone());
            }
            priority.entry(v).or_insert_with(|| get(r, "priority").to_string());
        }
    }
    for (vlan, by_root) in &roots {
        out.ran("x.stp_root_disagree");
        if by_root.len() > 1 && input.links.iter().any(|l| by_root.values().flatten().any(|d| d == &l.a) && by_root.values().flatten().any(|d| d == &l.b)) {
            let first = by_root.values().next().and_then(|d| d.first()).cloned().unwrap_or_default();
            let said: Vec<String> = by_root.iter().map(|(root, devs)| format!("{} see root {root}", devs.join(", "))).collect();
            out.add("x.stp_root_disagree", &first, f(&[("vlan", vlan.clone()), ("roots", said.join("; "))]), said);
        }
        if let Some(root_dev) = is_root.get(vlan) {
            out.ran("x.stp_root_on_access");
            let role = input.roles.get(root_dev).map(|r| r.to_ascii_lowercase()).unwrap_or_default();
            if role.contains("access") || role.contains("edge") {
                out.add("x.stp_root_on_access", root_dev, f(&[("vlan", vlan.clone()), ("role", role)]), Vec::new());
            }
        }
    }
    let Some(before) = input.before else { return };
    for (dev, t) in before {
        for r in rows(t, "stp_root") {
            if !this_is_root(r) {
                continue;
            }
            // `dev` was the root of this VLAN last time.
            let vlan = vlan_key(get(r, "vlan"));
            let was = get(r, "root_id").to_ascii_lowercase();
            out.ran("x.stp_root_moved");
            let now_root = roots.get(&vlan).and_then(|m| m.keys().next().cloned());
            if let Some(now_root) = now_root.filter(|n| n != &was) {
                let new_dev = is_root.get(&vlan).cloned().unwrap_or_else(|| "a device outside this scope".into());
                let current_priority = input.now.get(dev).and_then(|t| root_of(t, &vlan)).map(|r| get(&r, "priority").to_string()).or_else(|| priority.get(&vlan).cloned()).unwrap_or_default();
                let target = crate::parse::number(&current_priority).map(|p| (((p as i64 - 4096).max(0)) / 4096 * 4096).to_string()).unwrap_or_default();
                out.add(
                    "x.stp_root_moved",
                    dev,
                    f(&[("vlan", vlan.clone()), ("was", was.clone()), ("now", now_root.clone()), ("new_root", new_dev), ("priority", target)]),
                    vec![format!("previous run: {dev} was the root ({was})"), format!("now: the root is {now_root}")],
                );
            }
        }
    }
}

fn gateways(out: &mut Out<'_>, input: &Input<'_>) {
    // Group → (device, state, the row).
    type Member = (String, String, BTreeMap<String, String>);
    let mut groups: BTreeMap<String, Vec<Member>> = BTreeMap::new();
    for (dev, t) in input.now {
        for r in rows(t, "fhrp") {
            let key = if get(r, "vip").is_empty() { format!("{}/{}", canon_port(get(r, "interface")), get(r, "group")) } else { get(r, "vip").to_string() };
            groups.entry(key).or_default().push((dev.clone(), get(r, "state").to_ascii_lowercase(), r.clone()));
        }
    }
    for (vip, members) in &groups {
        out.ran("x.fhrp_split");
        out.ran("x.fhrp_no_active");
        let active: Vec<&Member> = members.iter().filter(|(_, s, _)| s == "active" || s == "master").collect();
        let first = &members[0].2;
        let vlan = canon_port(get(first, "interface")).trim_start_matches("vlan").to_string();
        let fields = |devs: String| f(&[("vip", vip.clone()), ("group", get(first, "group").into()), ("interface", get(first, "interface").into()), ("vlan", vlan.clone()), ("devices", devs)]);
        if active.len() > 1 {
            out.add("x.fhrp_split", &active[0].0, fields(active.iter().map(|(d, _, _)| d.clone()).collect::<Vec<_>>().join(", ")), active.iter().map(|(d, s, _)| format!("{d}: {s}")).collect());
        } else if active.is_empty() && members.len() > 1 {
            out.add("x.fhrp_no_active", &members[0].0, fields(members.iter().map(|(d, _, _)| d.clone()).collect::<Vec<_>>().join(", ")), members.iter().map(|(d, s, _)| format!("{d}: {s}")).collect());
        }
    }
}

/// Virtual router MACs (HSRP v1/v2, VRRP): an address answering from one
/// of these on two boxes is the gateway, not a duplicate.
fn virtual_mac(m: &str) -> bool {
    let m: String = m.to_ascii_lowercase().chars().filter(char::is_ascii_hexdigit).collect();
    m.starts_with("00000c07ac") || m.starts_with("00000c9ff") || m.starts_with("00005e0001") || m.starts_with("00005e0002")
}

fn addresses(out: &mut Out<'_>, input: &Input<'_>) {
    let vips: BTreeSet<String> = input.now.values().flat_map(|t| rows(t, "fhrp").iter().map(|r| get(r, "vip").to_string())).collect();
    let mut by_ip: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut seen_on: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (dev, t) in input.now {
        for r in rows(t, "arp") {
            let (ip, mac) = (get(r, "ip"), get(r, "mac").to_ascii_lowercase());
            if ip.is_empty() || mac.is_empty() || mac.contains("incomplete") || virtual_mac(&mac) || vips.contains(ip) {
                continue;
            }
            let mac: String = mac.chars().filter(char::is_ascii_hexdigit).collect();
            if by_ip.entry(ip.to_string()).or_default().insert(mac.clone()) {
                seen_on.entry(ip.to_string()).or_default().push(format!("{dev}: {ip} at {mac}"));
            }
        }
    }
    if !by_ip.is_empty() {
        out.ran("x.duplicate_ip");
    }
    for (ip, macs) in &by_ip {
        if macs.len() > 1 {
            let first = seen_on[ip][0].split(':').next().unwrap_or("").to_string();
            out.add("x.duplicate_ip", &first, f(&[("ip", ip.clone()), ("macs", macs.iter().cloned().collect::<Vec<_>>().join(", "))]), seen_on[ip].clone());
        }
    }
    let mut configured: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (dev, t) in input.now {
        for r in rows(t, "ip_interfaces") {
            let ip = get(r, "ip");
            if ip.is_empty() || ip.eq_ignore_ascii_case("unassigned") || vips.contains(ip) {
                continue;
            }
            configured.entry(ip.to_string()).or_default().push((dev.clone(), get(r, "interface").to_string()));
        }
    }
    if !configured.is_empty() {
        out.ran("x.ip_on_two_devices");
    }
    for (ip, on) in configured {
        let devs: BTreeSet<&String> = on.iter().map(|(d, _)| d).collect();
        if devs.len() > 1 {
            out.add("x.ip_on_two_devices", &on[0].0, f(&[("ip", ip.clone()), ("where", on.iter().map(|(d, i)| format!("{d} {i}")).collect::<Vec<_>>().join(", "))]), on.iter().map(|(d, i)| format!("{d} {i}: {ip}")).collect());
        }
    }
}

fn drift(out: &mut Out<'_>, input: &Input<'_>) {
    for l in input.links {
        let (Some(ta), Some(tb)) = (input.now.get(&l.a), input.now.get(&l.b)) else { continue };
        if rows(ta, "neighbors").is_empty() && rows(tb, "neighbors").is_empty() {
            continue;
        }
        out.ran("x.link_not_seen");
        let sees = |t: &DeviceTables, other: &str| rows(t, "neighbors").iter().any(|n| peer_named(input, get(n, "remote")).as_deref() == Some(other));
        if !sees(ta, &l.b) && !sees(tb, &l.a) {
            out.add("x.link_not_seen", &l.a, f(&[("port", l.a_port.clone()), ("peer", l.b.clone()), ("peer_port", l.b_port.clone())]), Vec::new());
        }
    }
    for (dev, t) in input.now {
        for n in rows(t, "neighbors") {
            let Some(peer) = peer_named(input, get(n, "remote")) else { continue };
            out.ran("x.neighbor_not_drawn");
            let drawn = input.links.iter().any(|l| (l.a == *dev && l.b == peer) || (l.b == *dev && l.a == peer));
            if !drawn && &peer != dev {
                out.add("x.neighbor_not_drawn", dev, f(&[("port", get(n, "local_port").into()), ("peer", peer.clone()), ("peer_port", get(n, "remote_port").into())]), Vec::new());
            }
        }
    }
}

fn ospf(out: &mut Out<'_>, input: &Input<'_>) {
    let Some(before) = input.before else { return };
    for (dev, t) in before {
        let Some(now) = input.now.get(dev) else { continue };
        for r in rows(t, "ospf") {
            if !get(r, "state").to_ascii_lowercase().starts_with("full") {
                continue;
            }
            out.ran("x.ospf_neighbor_lost");
            let id = get(r, "neighbor");
            let still = rows(now, "ospf").iter().any(|n| get(n, "neighbor") == id);
            if !still {
                out.add("x.ospf_neighbor_lost", dev, f(&[("neighbor", id.into()), ("interface", get(r, "interface").into()), ("address", get(r, "address").into())]), vec![format!("previous run: {id} FULL on {}", get(r, "interface"))]);
            }
        }
    }
}

fn helpers(out: &mut Out<'_>, input: &Input<'_>) {
    for (dev, t) in input.now {
        let helped: BTreeMap<String, String> = rows(t, "helpers").iter().filter(|r| !get(r, "helper").is_empty()).map(|r| (canon_port(get(r, "interface")), get(r, "helper").to_string())).collect();
        if helped.is_empty() {
            continue;
        }
        out.ran("x.dhcp_helper_missing");
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for h in helped.values() {
            *counts.entry(h.split(',').next().unwrap_or(h).trim()).or_default() += 1;
        }
        let common = counts.iter().max_by_key(|(_, n)| **n).map(|(h, _)| h.to_string()).unwrap_or_default();
        for r in rows(t, "ip_interfaces") {
            let name = canon_port(get(r, "interface"));
            let Some(vlan) = name.strip_prefix("vlan") else { continue };
            if get(r, "ip").is_empty() || get(r, "ip").eq_ignore_ascii_case("unassigned") || helped.contains_key(&name) || !up(get(r, "status")) {
                continue;
            }
            // Only a VLAN with access ports up in it has clients asking.
            let clients = rows(t, "interfaces").iter().filter(|i| get(i, "vlan") == vlan && up(get(i, "status"))).count();
            if clients == 0 {
                continue;
            }
            out.add("x.dhcp_helper_missing", dev, f(&[("interface", get(r, "interface").into()), ("vlan", vlan.into()), ("helper", common.clone()), ("clients", clients.to_string())]), vec![format!("other SVIs on {dev} relay to {common}")]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_in_every_spelling_are_one() {
        assert_eq!(canon_port("GigabitEthernet1/0/1"), canon_port("Gi1/0/1"));
        assert_eq!(canon_port("Ethernet1/49"), canon_port("Eth1/49"));
        assert_eq!(canon_port("TenGigabitEthernet1/1/1"), "te1/1/1");
        assert_eq!(canon_port("Port-channel10"), canon_port("Po10"));
        assert_eq!(canon_port("Vlan10"), "vlan10");
        assert_ne!(canon_port("Gi1/0/1"), canon_port("Gi1/0/10"));
    }

    #[test]
    fn vlan_lists() {
        assert_eq!(vlan_set("1-3,10"), [1, 2, 3, 10].into());
        assert_eq!(vlan_set("ALL").len(), 4094);
        assert!(vlan_set("none").is_empty());
        assert_eq!(vlan_set("10,20 30"), [10, 20, 30].into());
    }
}
