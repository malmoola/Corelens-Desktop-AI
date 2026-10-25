//! Path mode: the devices between two endpoints, as the project's links
//! join them, and at each one whether the packet would get through — the
//! ports up, the VLAN carried on a trunk, a route onward, an ACL in the
//! way, a firewall's policy — and the first hop where it would not.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::Ipv4Addr;

use crate::analyses::{canon_port, vlan_set, DeviceTables};
use crate::model::{Finding, Hop, HopCheck, PathResult, TopoLink};
use crate::playbook::{Layer, Severity};

/// The devices from `src` to `dst` over the links, with the ports each
/// one is entered and left by; `None` when the links do not join them.
pub fn route(links: &[TopoLink], src: &str, dst: &str) -> Option<Vec<(String, String, String)>> {
    if src == dst {
        return Some(vec![(src.to_string(), String::new(), String::new())]);
    }
    let mut prev: BTreeMap<String, (String, String, String)> = BTreeMap::new();
    let mut seen: BTreeSet<String> = [src.to_string()].into();
    let mut q: VecDeque<String> = [src.to_string()].into();
    while let Some(at) = q.pop_front() {
        if at == dst {
            break;
        }
        for l in links {
            let next = if l.a == at { Some((l.b.clone(), l.a_port.clone(), l.b_port.clone())) } else if l.b == at { Some((l.a.clone(), l.b_port.clone(), l.a_port.clone())) } else { None };
            if let Some((n, out_port, in_port)) = next {
                if seen.insert(n.clone()) {
                    prev.insert(n.clone(), (at.clone(), out_port, in_port));
                    q.push_back(n);
                }
            }
        }
    }
    if !seen.contains(dst) {
        return None;
    }
    // Walk back: each device with the port it was entered and left by.
    let mut chain: Vec<(String, String, String)> = Vec::new();
    let mut at = dst.to_string();
    let mut left_by = String::new();
    while let Some((p, p_out, at_in)) = prev.get(&at).cloned() {
        chain.push((at, at_in, std::mem::take(&mut left_by)));
        left_by = p_out;
        at = p;
    }
    chain.push((at, String::new(), left_by));
    chain.reverse();
    Some(chain)
}

fn get<'a>(r: &'a BTreeMap<String, String>, f: &str) -> &'a str {
    r.get(f).map(String::as_str).unwrap_or("")
}

fn port_row<'a>(t: &'a DeviceTables, table: &str, port: &str) -> Option<&'a BTreeMap<String, String>> {
    let want = canon_port(port);
    t.get(table)?.iter().find(|r| canon_port(get(r, "port")) == want)
}

/// The VLAN whose SVI holds `ip`, on any device's `ip_interfaces`.
pub fn vlan_of(tables: &BTreeMap<String, DeviceTables>, ip: Ipv4Addr) -> Option<u16> {
    for t in tables.values() {
        for r in t.get("ip_interfaces").into_iter().flatten() {
            let (Ok(a), Ok(len)) = (get(r, "ip").parse::<Ipv4Addr>(), get(r, "prefix_length").parse::<u32>()) else { continue };
            if len == 0 || len > 32 {
                continue;
            }
            let mask = u32::MAX << (32 - len);
            if u32::from(a) & mask == u32::from(ip) & mask {
                if let Some(v) = canon_port(get(r, "interface")).strip_prefix("vlan").and_then(|v| v.parse().ok()) {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// The hops' verdicts. `findings` are the run's, of which the `path`
/// layer's (no route, a policy that denies) count at their hop.
pub fn walk(chain: &[(String, String, String)], tables: &BTreeMap<String, DeviceTables>, findings: &[Finding], passed: &BTreeMap<String, BTreeSet<String>>, vlan: Option<u16>, src: &str, dst: &str) -> PathResult {
    let mut hops = Vec::new();
    for (dev, in_port, out_port) in chain {
        let mut checks = Vec::new();
        let Some(t) = tables.get(dev) else {
            checks.push(HopCheck { what: "reached".into(), ok: false, detail: format!("{dev} could not be read, so nothing past here is known") });
            hops.push(Hop { device: dev.clone(), in_port: in_port.clone(), out_port: out_port.clone(), checks });
            continue;
        };
        for (which, port) in [("in", in_port), ("out", out_port)] {
            if port.is_empty() {
                continue;
            }
            if let Some(r) = port_row(t, "interfaces", port) {
                let s = get(r, "status").to_ascii_lowercase();
                let ok = s == "up" || s == "connected";
                checks.push(HopCheck { what: format!("{port} ({which}) up"), ok, detail: if ok { format!("{port} is up") } else { format!("{port} is {s}") } });
            }
            if let Some(r) = port_row(t, "counters", port) {
                let crc = crate::parse::number(get(r, "crc")).unwrap_or(0.0);
                let ie = crate::parse::number(get(r, "input_errors")).unwrap_or(0.0);
                if crc > 0.0 || ie > 0.0 {
                    checks.push(HopCheck { what: format!("{port} errors"), ok: true, detail: format!("{port} has {ie} input errors, {crc} CRC: it passes traffic, but loses some") });
                }
            }
            if let (Some(v), Some(r)) = (vlan, port_row(t, "switchport", port)) {
                if get(r, "mode").to_ascii_lowercase().contains("trunk") {
                    let ok = vlan_set(get(r, "allowed")).contains(&v);
                    checks.push(HopCheck { what: format!("VLAN {v} on {port}"), ok, detail: if ok { format!("{port} carries VLAN {v}") } else { format!("{port} does not carry VLAN {v} (allowed: {})", get(r, "allowed")) } });
                }
            }
        }
        if let Some(v) = vlan {
            for r in t.get("ip_interfaces").into_iter().flatten() {
                if canon_port(get(r, "interface")) == format!("vlan{v}") {
                    for (dir, f) in [("inbound", "acl_in"), ("outbound", "acl_out")] {
                        let acl = get(r, f);
                        if !acl.is_empty() && !acl.eq_ignore_ascii_case("not set") {
                            checks.push(HopCheck { what: format!("ACL on Vlan{v}"), ok: true, detail: format!("{acl} is applied {dir} on Vlan{v}; Corelens does not evaluate ACL entries, so read it with show ip access-lists {acl}") });
                        }
                    }
                }
            }
        }
        for f in findings.iter().filter(|f| f.device == *dev && f.layer == Layer::Path) {
            checks.push(HopCheck { what: f.title.clone(), ok: f.severity < Severity::Critical, detail: f.says.clone() });
        }
        for c in passed.get(dev).into_iter().flatten().filter(|c| c.starts_with("path.")) {
            checks.push(HopCheck { what: c.trim_start_matches("path.").replace('_', " "), ok: true, detail: "passed".into() });
        }
        hops.push(Hop { device: dev.clone(), in_port: in_port.clone(), out_port: out_port.clone(), checks });
    }
    let first_break = hops.iter().position(|h| h.checks.iter().any(|c| !c.ok));
    PathResult { src: src.into(), dst: dst.into(), hops, first_break, probe: Vec::new(), note: String::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(a: &str, ap: &str, b: &str, bp: &str) -> TopoLink {
        TopoLink { a: a.into(), a_port: ap.into(), b: b.into(), b_port: bp.into() }
    }

    #[test]
    fn the_route_names_each_hops_ports() {
        let links = [l("ACC1", "Gi1/0/48", "DIST1", "Gi1/0/1"), l("DIST1", "Gi1/0/2", "CORE1", "Te1/1/1"), l("DIST1", "Gi1/0/3", "ACC2", "Gi1/0/48")];
        let r = route(&links, "ACC1", "CORE1").unwrap();
        assert_eq!(
            r,
            vec![
                ("ACC1".into(), "".into(), "Gi1/0/48".into()),
                ("DIST1".into(), "Gi1/0/1".into(), "Gi1/0/2".into()),
                ("CORE1".into(), "Te1/1/1".into(), "".into())
            ]
        );
        assert!(route(&links, "ACC1", "NOWHERE").is_none());
        assert_eq!(route(&links, "ACC1", "ACC1").unwrap().len(), 1);
    }
}
