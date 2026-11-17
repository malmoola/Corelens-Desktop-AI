//! The open project's devices and links, as the tools see them.
//!
//! Read from the saved document — the same JSON the window saves — so a
//! device is exactly what the diagram shows. A device is named the way a
//! person would name it (its label, hostname, any of its addresses, or its
//! id), and only a device of this project resolves: anything else is
//! refused, which is what keeps every tool inside the customer in hand.

use serde_json::Value;

/// One device on any page of the project.
#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub page: String,
    pub data: Value,
    /// Hostnames a crawl or a collection run found at one of its
    /// addresses, which the drawing may not carry (see
    /// [`with_discovered_names`]).
    pub aliases: Vec<String>,
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

impl Device {
    pub fn label(&self) -> &str {
        s(&self.data, "label")
    }
    pub fn field(&self, key: &str) -> &str {
        s(&self.data, key)
    }
    /// The addresses, primary first, as `(label, address)`.
    pub fn addresses(&self) -> Vec<(String, String)> {
        let mut out: Vec<(bool, String, String)> = self
            .data
            .get("addresses")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|x| !s(x, "address").trim().is_empty())
                    .map(|x| (x.get("isPrimary").and_then(Value::as_bool).unwrap_or(false), s(x, "label").to_string(), s(x, "address").trim().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|(primary, _, _)| !*primary);
        out.into_iter().map(|(_, l, a)| (l, a)).collect()
    }
    /// Where a session to it goes: the primary address, else the DNS name,
    /// else the hostname.
    pub fn target(&self) -> Option<String> {
        self.addresses()
            .into_iter()
            .map(|(_, a)| a)
            .next()
            .or_else(|| Some(self.field("dnsName").to_string()).filter(|d| !d.is_empty()))
            .or_else(|| Some(self.field("hostname").to_string()).filter(|d| !d.is_empty()))
    }
    pub fn has_login(&self) -> bool {
        !self.field("sshCredentialId").is_empty()
    }
    /// The one-line summary `list_devices` gives.
    pub fn summary(&self) -> String {
        let mut parts = vec![self.label().to_string()];
        for key in ["deviceType", "vendor", "model", "osVersion"] {
            let v = self.field(key);
            if !v.is_empty() {
                parts.push(v.to_string());
            }
        }
        let addrs: Vec<String> = self.addresses().into_iter().map(|(l, a)| if l.is_empty() { a } else { format!("{a} ({l})") }).collect();
        if !addrs.is_empty() {
            parts.push(addrs.join(", "));
        }
        for (key, name) in [("hostname", "hostname"), ("site", "site"), ("role", "role")] {
            let v = self.field(key);
            if !v.is_empty() && v != self.label() {
                parts.push(format!("{name} {v}"));
            }
        }
        parts.push(if self.has_login() { "login saved".into() } else { "no login saved".into() });
        parts.join(" · ")
    }
    /// Everything a filter or a search looks at.
    pub fn haystack(&self) -> String {
        let mut h: Vec<String> = ["label", "hostname", "dnsName", "vendor", "model", "osVersion", "serial", "site", "role", "deviceType", "mac"]
            .iter()
            .map(|k| self.field(k).to_string())
            .collect();
        h.extend(self.addresses().into_iter().map(|(_, a)| a));
        h.join(" ").to_lowercase()
    }
}

/// One link between two devices.
#[derive(Debug, Clone)]
pub struct Link {
    pub source: String,
    pub target: String,
    pub data: Value,
}

/// The project's devices, every page, in page then drawing order.
pub fn devices(document: &Value) -> Vec<Device> {
    let mut out = Vec::new();
    for page in document.get("pages").and_then(Value::as_array).into_iter().flatten() {
        let page_name = s(page, "name").to_string();
        for node in page.get("nodes").and_then(Value::as_array).into_iter().flatten() {
            if node.get("type").and_then(Value::as_str).unwrap_or("device") != "device" {
                continue;
            }
            let Some(data) = node.get("data") else { continue };
            out.push(Device { id: s(node, "id").to_string(), page: page_name.clone(), data: data.clone(), aliases: Vec::new() });
        }
    }
    out
}

/// The project's links between devices; a note's leader line is not one.
pub fn links(document: &Value) -> Vec<Link> {
    let mut out = Vec::new();
    for page in document.get("pages").and_then(Value::as_array).into_iter().flatten() {
        for edge in page.get("edges").and_then(Value::as_array).into_iter().flatten() {
            let data = edge.get("data").cloned().unwrap_or(Value::Null);
            if s(&data, "kind") == "leader" {
                continue;
            }
            out.push(Link { source: s(edge, "source").to_string(), target: s(edge, "target").to_string(), data });
        }
    }
    out
}

/// The hostname a CLI prompt shows: `access-switch#`, `sw(config)#`,
/// `admin@fw-01:~$`, `FGT-01 (root) #`. Empty when it shows none.
pub fn hostname_from_prompt(prompt: &str) -> String {
    let mut t = prompt.trim().trim_end_matches(['#', '>', '$', '%']).trim();
    if let Some((_, host)) = t.split_once('@') {
        t = host;
    }
    let t = t.split([':', '(']).next().unwrap_or("").trim();
    t.to_string()
}

/// Gives each device the hostnames discovery found at its addresses:
/// `(address, hostname)` pairs, from crawls and collection runs. A name
/// found at an address no device has names nothing.
pub fn with_discovered_names(devices: &mut [Device], names: &[(String, String)]) {
    for (address, name) in names {
        let (address, name) = (address.trim(), name.trim());
        if address.is_empty() || name.is_empty() {
            continue;
        }
        for d in devices.iter_mut() {
            let here = d.addresses().iter().any(|(_, a)| a.eq_ignore_ascii_case(address)) || d.target().is_some_and(|t| t.eq_ignore_ascii_case(address));
            if here && !d.aliases.iter().any(|a| a.eq_ignore_ascii_case(name)) {
                d.aliases.push(name.to_string());
            }
        }
    }
}

impl Device {
    /// Every name the device goes by: its label, hostname, DNS name, the
    /// hostname its prompt showed, and what discovery found at its address.
    fn names(&self) -> Vec<&str> {
        let mut n = vec![self.label(), self.field("hostname"), self.field("dnsName")];
        if let Some(h) = self.data.pointer("/evidence/hostname/detail").and_then(Value::as_str) {
            n.push(h);
        }
        n.extend(self.aliases.iter().map(String::as_str));
        n.into_iter().map(str::trim).filter(|x| !x.is_empty()).collect()
    }
}

/// A name without its domain: `access-switch.corp.example` → `access-switch`.
fn short_name(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}

/// The device a person meant, or why there is not exactly one.
pub fn resolve<'a>(devices: &'a [Device], name: &str) -> Result<&'a Device, String> {
    let wanted = name.trim();
    if wanted.is_empty() {
        return Err("Name a device: its name, hostname, management address or id, as list_devices shows it.".into());
    }
    let lower = wanted.to_lowercase();
    let mut exact: Vec<&Device> = devices
        .iter()
        .filter(|d| {
            d.id == wanted
                || d.names().iter().any(|n| n.to_lowercase() == lower)
                || d.addresses().iter().any(|(_, a)| a.eq_ignore_ascii_case(wanted))
        })
        .collect();
    // Then a hostname with its domain or without, when no name was exact.
    // An address is never shortened.
    if exact.is_empty() && wanted.parse::<std::net::IpAddr>().is_err() {
        let short = short_name(&lower).to_string();
        exact = devices
            .iter()
            .filter(|d| d.names().iter().any(|n| (n.contains('.') || wanted.contains('.')) && n.parse::<std::net::IpAddr>().is_err() && short_name(&n.to_lowercase()) == short))
            .collect();
    }
    match exact.as_slice() {
        [one] => Ok(one),
        [] => Err(format!(
            "\"{wanted}\" is not a device of the project open in Corelens, so nothing was done. Only that project's devices can be used; list_devices shows them."
        )),
        many => Err(format!(
            "\"{wanted}\" names {} devices in this project ({}). Use the id instead.",
            many.len(),
            many.iter().map(|d| format!("{} [{}]", d.label(), d.id)).collect::<Vec<_>>().join(", ")
        )),
    }
}

#[cfg(test)]
pub mod fixture {
    use serde_json::{json, Value};

    /// A small project: invented names, documentation-range addresses.
    pub fn document() -> Value {
        json!({
            "pages": [
                { "id": "p1", "name": "Core", "nodes": [
                    { "id": "n1", "type": "device", "data": {
                        "label": "SW1", "deviceType": "switch", "vendor": "Cisco", "model": "WS-C2960X-24TS-L", "osVersion": "15.2(4)E7",
                        "hostname": "sw1.example.test", "site": "HQ", "role": "access", "sshCredentialId": "cred-1", "tags": [],
                        "addresses": [{ "id": "a1", "label": "Management", "address": "192.0.2.10", "isPrimary": true }],
                        "inventory": { "collectedAt": 1, "ports": [
                            { "port": "Gi1/0/1", "description": "uplink to RTR1", "status": "up", "speed": "1G", "mode": "trunk", "trunkVlans": "10,20" },
                            { "port": "Gi1/0/2", "description": "printer", "status": "down", "vlan": 20, "errors": { "input": 0, "crc": 12, "output": 0, "collisions": 0, "resets": 0, "drops": 3 } }
                        ], "vlans": [{ "id": 10, "name": "users" }, { "id": 20, "name": "printers" }],
                        "routes": [{ "family": 4, "prefix": "0.0.0.0/0", "protocol": "static", "nextHops": ["192.0.2.1"] }],
                        "spanningTree": [{ "instance": "VLAN0010", "vlan": 10, "rootBridge": "RTR1", "isRoot": false, "rootPort": "Gi1/0/1", "blocked": [] }] }
                    } },
                    { "id": "n2", "type": "device", "data": {
                        "label": "RTR1", "deviceType": "router", "vendor": "Cisco", "tags": [],
                        "addresses": [{ "id": "a2", "label": "", "address": "192.0.2.1", "isPrimary": true }]
                    } },
                    { "id": "n3", "type": "note", "data": { "text": "a note" } }
                ], "edges": [
                    { "id": "e1", "source": "n1", "target": "n2", "data": { "sourcePortLabel": "Gi1/0/1", "targetPortLabel": "Gi0/0", "label": "" } },
                    { "id": "e2", "source": "n3", "target": "n1", "data": { "kind": "leader", "sourcePortLabel": "", "targetPortLabel": "", "label": "" } }
                ] },
                { "id": "p2", "name": "Branch", "nodes": [
                    { "id": "n4", "type": "device", "data": { "label": "FW1", "deviceType": "firewall", "vendor": "Fortinet", "tags": [],
                        "addresses": [{ "id": "a4", "label": "", "address": "198.51.100.4", "isPrimary": true }] } }
                ], "edges": [] }
            ],
            "activePageId": "p1",
            "probes": []
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_come_from_every_page_and_notes_are_not_devices() {
        let doc = fixture::document();
        let d = devices(&doc);
        assert_eq!(d.iter().map(|x| x.label()).collect::<Vec<_>>(), ["SW1", "RTR1", "FW1"]);
        assert_eq!(links(&doc).len(), 1, "a leader line is not a link");
    }

    #[test]
    fn a_device_is_found_by_any_of_its_names_and_nothing_else_is() {
        let doc = fixture::document();
        let d = devices(&doc);
        for name in ["SW1", "sw1", "sw1.example.test", "192.0.2.10", "n1", " SW1 "] {
            assert_eq!(resolve(&d, name).unwrap().id, "n1", "{name}");
        }
        let err = resolve(&d, "203.0.113.9").unwrap_err();
        assert!(err.contains("not a device of the project open in Corelens"), "{err}");
        assert!(resolve(&d, "").is_err());
    }

    #[test]
    fn a_device_is_found_by_the_hostname_discovery_found_at_its_address() {
        let mut d = devices(&fixture::document());
        // A crawl or collection run met "access-switch" at SW1's address,
        // and "elsewhere-sw" at an address no device of the project has.
        with_discovered_names(&mut d, &[("192.0.2.10".into(), "access-switch".into()), ("203.0.113.9".into(), "elsewhere-sw".into())]);
        for name in ["access-switch", "ACCESS-SWITCH", "access-switch.corp.example", " access-switch "] {
            assert_eq!(resolve(&d, name).map(|x| x.id.as_str()), Ok("n1"), "{name}");
        }
        assert!(resolve(&d, "elsewhere-sw").is_err(), "a name found at no address of this project stays refused");
        // The stored hostname with its domain or without.
        assert_eq!(resolve(&d, "sw1.example.test").map(|x| x.id.as_str()), Ok("n1"));
        assert_eq!(resolve(&d, "sw1.other.example").map(|x| x.id.as_str()), Ok("n1"));
        // What the prompt said, kept as evidence.
        let mut doc = fixture::document();
        doc["pages"][0]["nodes"][0]["data"]["evidence"] = serde_json::json!({ "hostname": { "source": "prompt", "seenAtMs": 1, "detail": "core-sw-01" } });
        assert_eq!(resolve(&devices(&doc), "core-sw-01").map(|x| x.id.as_str()), Ok("n1"));
    }

    #[test]
    fn a_hostname_is_read_off_a_prompt() {
        for (prompt, name) in [("access-switch#", "access-switch"), ("access-switch>", "access-switch"), ("access-switch(config)#", "access-switch"), ("admin@fw-01:~$", "fw-01"), ("FGT-01 (root) #", "FGT-01"), ("#", "")] {
            assert_eq!(hostname_from_prompt(prompt), name, "{prompt}");
        }
    }

    #[test]
    fn the_summary_says_what_a_person_needs() {
        let doc = fixture::document();
        let d = devices(&doc);
        let line = d[0].summary();
        assert!(line.starts_with("SW1 · switch · Cisco · WS-C2960X-24TS-L · 15.2(4)E7 · 192.0.2.10 (Management)"), "{line}");
        assert!(line.ends_with("login saved"));
        assert!(d[1].summary().ends_with("no login saved"));
        assert_eq!(d[0].target().as_deref(), Some("192.0.2.10"));
    }
}
