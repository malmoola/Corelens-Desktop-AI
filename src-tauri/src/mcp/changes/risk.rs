//! How dangerous a proposed change is, said in plain words.
//!
//! Every proposal gets a class — low, medium or high — and a list of
//! **hard blocks**: lines that can take the device or the way to it away
//! (a reload, an erase, the management interface, the default route, the
//! logins), which no approval passes until the engineer has also typed the
//! device's name under a sentence saying what could happen. The rules are
//! deliberately broad: a line is held back for a second look when it
//! *could* be one of these, and the cost of that is one more typed word.
//!
//! The management path is worked out from the device's own running
//! configuration: the interface that carries the address Corelens reached
//! it on, that interface's VLAN, the access lists the VTY lines are
//! guarded by.

use serde::{Deserialize, Serialize};

use super::diff;
use super::platform::Platform;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Class {
    Low,
    Medium,
    High,
}

/// A line that needs the device's name typed before it can be approved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HardBlock {
    pub command: String,
    /// What could happen, in a sentence.
    pub risk: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assessment {
    pub class: Class,
    pub hard_blocks: Vec<HardBlock>,
    /// One sentence a person reads first.
    pub summary: String,
}

/// What is known about the way in to the device.
#[derive(Debug, Clone, Default)]
pub struct ManagementPath {
    /// The address Corelens reached it on.
    pub address: String,
    /// The interfaces (canonical names) whose configuration holds it.
    pub interfaces: Vec<String>,
    /// That interface's VLAN when it is an SVI (`Vlan10` → `10`).
    pub vlans: Vec<String>,
    /// Access lists named by `access-class` under the VTY lines.
    pub vty_acls: Vec<String>,
}

/// The management path, read from a Cisco-style running configuration.
pub fn management_path(running: &str, address: &str) -> ManagementPath {
    let tree = diff::parse(running);
    let mut path = ManagementPath { address: address.to_string(), ..Default::default() };
    for node in &tree.lines {
        let lower = node.text.to_ascii_lowercase();
        if lower.starts_with("interface ") && !address.is_empty() && node.children.iter().any(|c| c.text.split_whitespace().any(|w| w == address)) {
            let name = node.text["interface ".len()..].trim().to_string();
            if let Some(v) = name.strip_prefix("Vlan").or_else(|| name.strip_prefix("vlan")) {
                path.vlans.push(v.trim().to_string());
            }
            path.interfaces.push(name);
        }
        if lower.starts_with("line vty") {
            for c in &node.children {
                let w: Vec<&str> = c.text.split_whitespace().collect();
                if w.first() == Some(&"access-class") {
                    if let Some(acl) = w.get(1) {
                        path.vty_acls.push(acl.to_string());
                    }
                }
            }
        }
    }
    path
}

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(|w| w.to_ascii_lowercase()).collect()
}

/// A hard block's sentence for one line at the top level or in `block`,
/// or `None`.
fn hard_reason(platform: Platform, block: Option<&str>, line: &str, path: &ManagementPath) -> Option<String> {
    let l = line.trim().to_ascii_lowercase();
    let bare = l.strip_prefix("no ").unwrap_or(&l).to_string();
    let starts = |p: &str| bare.starts_with(p) || l.starts_with(p);
    let w = words(&bare);

    // Restarting, erasing, booting — any platform's spelling.
    if starts("reload") || starts("reboot") || l.contains("request system reboot") || l.contains("request system halt") || l.contains("request restart") || starts("execute reboot") || starts("execute shutdown") || l.contains("request system power-off") {
        return Some("This restarts or stops the device: everything through it is down until it comes back, and it may not come back with this configuration.".into());
    }
    if starts("write erase") || starts("erase ") || starts("format ") || starts("delete ") || starts("execute factoryreset") || starts("execute erase") || l.contains("request system zeroize") || l.contains("load factory-default") || starts("clear config") {
        return Some("This erases files or the configuration itself; what is lost cannot be put back from Corelens.".into());
    }
    if bare.starts_with("debug") || starts("diagnose debug") {
        return Some("Debugging can load the CPU until the device stops answering, and stays on until it is turned off.".into());
    }
    if starts("boot ") || starts("boot-start") || l.contains("system boot") || starts("set system boot") || starts("execute set-next-reboot") {
        return Some("This changes what the device starts from next time; a mistake shows only at the next restart, and may leave it unable to start.".into());
    }
    // Default route.
    if starts("ip default-gateway") || bare.starts_with("ip route 0.0.0.0 0.0.0.0") || bare.starts_with("ip route vrf") && bare.contains(" 0.0.0.0 0.0.0.0") || bare.starts_with("ipv6 route ::/0") || l.contains("route 0.0.0.0/0") || (l.contains("dst 0.0.0.0 0.0.0.0") || l.contains("dst 0.0.0.0/0")) {
        return Some("This changes the default route; the device, and the way Corelens reaches it, can lose every network it does not know directly.".into());
    }
    // Logins and how they are checked.
    if starts("aaa") || starts("tacacs") || starts("radius") || starts("tacacs-server") || starts("radius-server") || l.contains("system authentication-order") || l.contains("system radius-server") || l.contains("system tacplus-server") || l.contains("config user radius") || l.contains("config user tacacs+") || bare.starts_with("ip tacacs") || bare.starts_with("ip radius") {
        return Some("This changes how logins are checked (AAA, TACACS+, RADIUS); a mistake can lock everyone out, Corelens included.".into());
    }
    if starts("username ") || starts("user ") && platform != Platform::Junos || l.contains("system login user") || starts("enable secret") || starts("enable password") || l.contains("config system admin") || starts("mgt-config users") || l.contains("mgt-config users") {
        return Some("This changes a user account or its password; a mistake can lock the engineer out.".into());
    }
    if starts("crypto key") || starts("crypto pki") || starts("ip ssh") || l.contains("security pki") || l.contains("system services ssh") || starts("ssh server") {
        return Some("This changes the device's keys or its SSH service; a mistake ends SSH access, Corelens's included.".into());
    }
    if starts("line vty") || starts("line con") || starts("line console") || starts("line aux") || block.is_some_and(|b| b.to_ascii_lowercase().starts_with("line ")) || l.contains("system services") || starts("transport input") {
        return Some("This changes how the device is reached over its terminal lines (VTY or console); a mistake can stop every login.".into());
    }
    // The management path, from the running configuration.
    let in_block = |b: &str| block.is_some_and(|x| x.eq_ignore_ascii_case(&format!("interface {b}")));
    for iface in &path.interfaces {
        if in_block(iface) || bare == format!("interface {}", iface.to_ascii_lowercase()) {
            return Some(format!("This changes {iface}, which carries {} — the address Corelens and the engineer reach the device on.", path.address));
        }
    }
    if starts("interface mgmt") || starts("interface management") || block.is_some_and(|b| { let b = b.to_ascii_lowercase(); b.starts_with("interface mgmt") || b.starts_with("interface management") }) || l.contains("interfaces fxp0") || l.contains("interfaces em0") || l.contains("interfaces me0") || l.contains("interfaces irb") && !path.vlans.is_empty() {
        return Some("This changes the management interface; a mistake ends every management session.".into());
    }
    for v in &path.vlans {
        let vlan_cmd = w.first().map(String::as_str) == Some("vlan") && w.get(1).is_some_and(|x| x == v);
        let trunk_drop = bare.contains("switchport trunk allowed vlan") && (bare.contains(&format!("remove {v}")) || (!bare.contains(" add ") && !vlan_list_has(&bare, v)));
        if vlan_cmd || trunk_drop {
            return Some(format!("This touches VLAN {v}, the management VLAN; the device can drop off the network."));
        }
    }
    if !path.address.is_empty() && l.split_whitespace().any(|x| x == path.address) {
        return Some(format!("This names {}, the management address; a mistake moves or removes it.", path.address));
    }
    for acl in &path.vty_acls {
        let a = acl.to_ascii_lowercase();
        let names_it = w.windows(2).any(|p| p[1] == a && (p[0] == "access-list" || p[0] == "standard" || p[0] == "extended")) || w.get(1).is_some_and(|x| *x == a) && w.first().is_some_and(|x| x == "access-list") || block.is_some_and(|b| b.to_ascii_lowercase().ends_with(&format!(" {a}")) && b.to_ascii_lowercase().contains("access-list"));
        if names_it {
            return Some(format!("This changes access list {acl}, which guards the VTY lines; a mistake can stop every SSH login."));
        }
    }
    if starts("access-class") || l.contains("set allowaccess") || l.contains("trusthost") || starts("ip http access-class") || starts("management-access") {
        return Some("This changes which addresses may manage the device; a mistake can lock the engineer and Corelens out.".into());
    }
    None
}

fn vlan_list_has(line: &str, vlan: &str) -> bool {
    let Ok(want) = vlan.parse::<u32>() else { return false };
    let list = line.rsplit(' ').next().unwrap_or("");
    list.split(',').any(|part| match part.split_once('-') {
        Some((a, b)) => matches!((a.parse::<u32>(), b.parse::<u32>()), (Ok(a), Ok(b)) if a <= want && want <= b),
        None => part.parse::<u32>().ok() == Some(want),
    })
}

/// Lines that only describe or label.
fn is_low(line: &str) -> bool {
    let l = line.trim().to_ascii_lowercase();
    let bare = l.strip_prefix("no ").unwrap_or(&l);
    ["description", "name ", "remark", "alias", "snmp-server location", "snmp-server contact", "set description", "set comment", "set alias", "banner"]
        .iter()
        .any(|p| bare.starts_with(p) || bare.contains(&format!(" {p}")))
}

/// Lines that can stop traffic.
fn is_high(line: &str) -> bool {
    let l = line.trim().to_ascii_lowercase();
    let bare = l.strip_prefix("no ").unwrap_or(&l);
    bare == "shutdown"
        || l == "shutdown"
        || ["router ", "ip route", "ipv6 route", "access-list", "ip access-list", "spanning-tree", "channel-group", "switchport mode", "switchport trunk", "ip address", "vrf ", "ip nat", "policy-map", "service-policy", "route-map", "set protocols", "set routing-options", "set security policies", "set firewall", "config firewall policy", "config router", "set rulebase", "set network"]
            .iter()
            .any(|p| bare.starts_with(p))
}

/// The assessment of `commands` for one device.
pub fn assess(platform: Platform, commands: &[String], path: &ManagementPath) -> Assessment {
    let mut hard_blocks = Vec::new();
    let mut class = Class::Low;
    let mut open: Option<String> = None;
    for raw in commands {
        let indented = raw.starts_with(' ') || raw.starts_with('\t');
        let line = raw.trim();
        if line.is_empty() || line == "!" {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower == "exit" || lower == "end" || lower == "next" {
            open = None;
            continue;
        }
        if platform.cisco_like() && !indented && diff::opens_block(line) {
            open = Some(diff::canonical(line));
        }
        if let Some(risk) = hard_reason(platform, open.as_deref().filter(|b| !b.eq_ignore_ascii_case(&diff::canonical(line))), line, path) {
            hard_blocks.push(HardBlock { command: line.to_string(), risk });
        }
        let this = if is_high(line) {
            Class::High
        } else if is_low(line) || (platform.cisco_like() && !indented && diff::opens_block(line)) {
            Class::Low
        } else {
            Class::Medium
        };
        class = class.max(this);
    }
    if !hard_blocks.is_empty() {
        class = Class::High;
    }
    let summary = match (class, hard_blocks.len()) {
        (_, n) if n > 0 => format!("{n} line(s) can cut the device off or restart it; type the device's name to approve."),
        (Class::Low, _) => "Labels and descriptions only; traffic is not affected.".into(),
        (Class::Medium, _) => "Changes how ports or features behave; traffic through them may be affected.".into(),
        (Class::High, _) => "Can stop traffic: interfaces, routing, access lists or spanning tree.".into(),
    };
    Assessment { class, hard_blocks, summary }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNING: &str = "hostname SW1\ninterface GigabitEthernet1/0/1\n switchport mode trunk\n switchport trunk allowed vlan 10,20,30-40\ninterface GigabitEthernet1/0/24\n description spare\ninterface Vlan10\n ip address 192.0.2.10 255.255.255.0\nip access-list standard MGMT\n permit 192.0.2.0 0.0.0.255\nline vty 0 4\n access-class MGMT in\n transport input ssh\n";

    fn path() -> ManagementPath {
        management_path(RUNNING, "192.0.2.10")
    }

    fn cmds(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn blocked(v: &[&str]) -> Vec<String> {
        assess(Platform::CiscoIos, &cmds(v), &path()).hard_blocks.into_iter().map(|b| b.command).collect()
    }

    #[test]
    fn the_management_path_comes_from_the_running_config() {
        let p = path();
        assert_eq!(p.interfaces, ["Vlan10"]);
        assert_eq!(p.vlans, ["10"]);
        assert_eq!(p.vty_acls, ["MGMT"]);
    }

    #[test]
    fn a_description_on_a_spare_port_is_low_and_blocks_nothing() {
        let a = assess(Platform::CiscoIos, &cmds(&["interface Gi1/0/24", " description LAB-PORT"]), &path());
        assert_eq!(a.class, Class::Low);
        assert!(a.hard_blocks.is_empty());
    }

    #[test]
    fn every_listed_kind_of_line_is_a_hard_block() {
        for line in [
            "reload",
            "reload in 5",
            "write erase",
            "erase startup-config",
            "format flash:",
            "delete flash:config.text",
            "debug all",
            "debug ip packet",
            "boot system flash:c2960x.bin",
            "ip default-gateway 192.0.2.254",
            "ip route 0.0.0.0 0.0.0.0 192.0.2.254",
            "no ip route 0.0.0.0 0.0.0.0 192.0.2.1",
            "aaa new-model",
            "tacacs server LAB",
            "radius server LAB",
            "username admin privilege 15 secret fixture",
            "enable secret fixture",
            "crypto key generate rsa modulus 2048",
            "ip ssh version 2",
            "line vty 0 4",
            "line con 0",
        ] {
            assert_eq!(blocked(&[line]), [line], "{line} was not held back");
        }
    }

    #[test]
    fn the_management_interface_vlan_and_acl_are_hard_blocks() {
        assert_eq!(blocked(&["interface Vlan10", " shutdown"]), ["interface Vlan10", "shutdown"]);
        assert_eq!(blocked(&["no vlan 10"]), ["no vlan 10"]);
        assert_eq!(blocked(&["interface Gi1/0/1", " switchport trunk allowed vlan remove 10"]), ["switchport trunk allowed vlan remove 10"]);
        assert_eq!(blocked(&["interface Gi1/0/1", " switchport trunk allowed vlan 20,30"]), ["switchport trunk allowed vlan 20,30"]);
        assert!(blocked(&["interface Gi1/0/1", " switchport trunk allowed vlan 10,20"]).is_empty());
        assert_eq!(blocked(&["ip access-list standard MGMT", " permit any"]), ["ip access-list standard MGMT", "permit any"]);
        assert_eq!(blocked(&["line vty 0 4", " access-class OTHER in"]), ["line vty 0 4", "access-class OTHER in"]);
    }

    #[test]
    fn other_platforms_spellings_are_held_back_too() {
        let p = ManagementPath::default();
        for (platform, line) in [
            (Platform::Junos, "request system reboot"),
            (Platform::Junos, "set system login user eve class super-user"),
            (Platform::Junos, "set routing-options static route 0.0.0.0/0 next-hop 192.0.2.1"),
            (Platform::Junos, "delete system services ssh"),
            (Platform::FortiOs, "execute reboot"),
            (Platform::FortiOs, "set allowaccess ping https ssh"),
            (Platform::PanOs, "set mgt-config users eve permissions role-based superuser yes"),
            (Platform::CiscoNxOs, "interface mgmt0"),
        ] {
            assert!(!assess(platform, &cmds(&[line]), &p).hard_blocks.is_empty(), "{platform:?}: {line}");
        }
    }

    #[test]
    fn shutting_a_port_or_changing_routing_is_high_without_being_blocked() {
        let a = assess(Platform::CiscoIos, &cmds(&["interface Gi1/0/24", " shutdown"]), &path());
        assert_eq!((a.class, a.hard_blocks.len()), (Class::High, 0));
        let m = assess(Platform::CiscoIos, &cmds(&["interface Gi1/0/24", " switchport access vlan 20"]), &path());
        assert_eq!(m.class, Class::Medium);
    }
}
