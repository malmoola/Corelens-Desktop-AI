//! Configuration text as a tree, and what a change does to it.
//!
//! Two jobs, both for the platforms whose configuration is a Cisco-style
//! indented list (IOS, IOS-XE, NX-OS, AOS-CX):
//!
//! - **Before approval, a predicted diff.** The proposed lines applied to
//!   the running configuration the way such a device applies them —
//!   entering a block opens it, `no …` removes what it names, a setting
//!   that holds one value replaces the one there — and the blocks that
//!   change shown with their lines added and removed. It is a prediction,
//!   and the dialog says so; the device's own result is read back after
//!   the push and compared line by line ([`line_diff`]).
//! - **Reverse commands**, for a device whose own rollback did not work:
//!   what puts each changed block back as it was.
//!
//! Every function here is pure, so each rule has a test.

use std::collections::HashSet;

/// One line of configuration and the lines under it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Node {
    pub text: String,
    pub children: Vec<Node>,
}

/// A configuration: its top-level lines in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tree {
    pub lines: Vec<Node>,
}

/// One line of a diff.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    /// `" "` context, `"+"` added, `"-"` removed.
    pub mark: String,
    pub text: String,
}

impl DiffLine {
    fn new(mark: &str, text: &str) -> DiffLine {
        DiffLine { mark: mark.into(), text: text.into() }
    }
}

/// The lines a `show running-config` prints that are not configuration.
fn is_noise(line: &str) -> bool {
    let t = line.trim();
    t.is_empty()
        // `!` alone separates blocks; `!Time:`, `!Version …` and the like
        // are comments the device writes.
        || t.starts_with('!')
        || t == "end"
        || t.starts_with("Building configuration")
        || t.starts_with("Current configuration")
        || t.starts_with("Last configuration change")
        || t.starts_with("! Last configuration")
        || t.starts_with("!Time:")
        || t.starts_with("!Command:")
        || t.starts_with("version ") && line.starts_with("version")
}

fn indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

/// The configuration as a tree, by indentation.
pub fn parse(text: &str) -> Tree {
    let mut tree = Tree::default();
    // (indent, path of indices) for the open blocks.
    let mut stack: Vec<(usize, Vec<usize>)> = Vec::new();
    for raw in text.lines() {
        let raw = raw.trim_end_matches('\r');
        if is_noise(raw) {
            continue;
        }
        let depth = indent(raw);
        let node = Node { text: raw.trim().to_string(), children: Vec::new() };
        while stack.last().is_some_and(|(d, _)| *d >= depth) {
            stack.pop();
        }
        match stack.last() {
            None => {
                tree.lines.push(node);
                stack.push((depth, vec![tree.lines.len() - 1]));
            }
            Some((_, path)) => {
                let path = path.clone();
                let parent = node_at(&mut tree, &path);
                parent.children.push(node);
                let mut child = path;
                child.push(parent.children.len() - 1);
                stack.push((depth, child));
            }
        }
    }
    tree
}

fn node_at<'a>(tree: &'a mut Tree, path: &[usize]) -> &'a mut Node {
    let mut node = &mut tree.lines[path[0]];
    for i in &path[1..] {
        node = &mut node.children[*i];
    }
    node
}

/// An interface name as the device prints it in its configuration:
/// `Gi1/0/5` → `GigabitEthernet1/0/5`.
pub fn full_interface(name: &str) -> String {
    const LONG: &[(&str, &str)] = &[
        ("hundredgigabitethernet", "HundredGigabitEthernet"),
        ("fortygigabitethernet", "FortyGigabitEthernet"),
        ("twentyfivegige", "TwentyFiveGigE"),
        ("tengigabitethernet", "TenGigabitEthernet"),
        ("twogigabitethernet", "TwoGigabitEthernet"),
        ("fivegigabitethernet", "FiveGigabitEthernet"),
        ("gigabitethernet", "GigabitEthernet"),
        ("fastethernet", "FastEthernet"),
        ("ethernet", "Ethernet"),
        ("port-channel", "Port-channel"),
        ("loopback", "Loopback"),
        ("vlan", "Vlan"),
        ("tunnel", "Tunnel"),
        ("mgmt", "mgmt"),
    ];
    const SHORT: &[(&str, &str)] = &[
        ("hu", "HundredGigabitEthernet"),
        ("fo", "FortyGigabitEthernet"),
        ("twe", "TwentyFiveGigE"),
        ("te", "TenGigabitEthernet"),
        ("tw", "TwoGigabitEthernet"),
        ("fi", "FiveGigabitEthernet"),
        ("gi", "GigabitEthernet"),
        ("gig", "GigabitEthernet"),
        ("fa", "FastEthernet"),
        ("eth", "Ethernet"),
        ("et", "Ethernet"),
        ("po", "Port-channel"),
        ("lo", "Loopback"),
        ("vl", "Vlan"),
        ("tu", "Tunnel"),
    ];
    let name = name.trim();
    let split = name.find(|c: char| c.is_ascii_digit()).unwrap_or(name.len());
    let (word, rest) = name.split_at(split);
    let lower = word.trim().to_ascii_lowercase();
    for (long, proper) in LONG {
        if lower == *long {
            return format!("{proper}{rest}");
        }
    }
    for (short, proper) in SHORT {
        if lower == *short {
            return format!("{proper}{rest}");
        }
    }
    name.to_string()
}

/// A block-opening line in its canonical spelling (`int gi1/0/5` →
/// `interface GigabitEthernet1/0/5`), or the line itself.
pub fn canonical(line: &str) -> String {
    let t = line.trim();
    let mut words = t.splitn(2, char::is_whitespace);
    let first = words.next().unwrap_or("");
    let rest = words.next().unwrap_or("").trim();
    let f = first.to_ascii_lowercase();
    if !rest.is_empty() && ("interface".starts_with(&f) && f.len() >= 3) {
        // `interface Gi1/0/5`, `interface range …` left alone.
        if rest.to_ascii_lowercase().starts_with("range") {
            return format!("interface {rest}");
        }
        return format!("interface {}", full_interface(rest));
    }
    t.to_string()
}

/// Lines that open a block at the top level of a Cisco-style
/// configuration.
pub fn opens_block(line: &str) -> bool {
    let l = canonical(line).to_ascii_lowercase();
    const OPENERS: &[&str] = &[
        "interface ",
        "line ",
        "vlan ",
        "router ",
        "ip access-list ",
        "ipv6 access-list ",
        "mac access-list ",
        "class-map ",
        "policy-map ",
        "route-map ",
        "ip prefix-list ",
        "aaa group server ",
        "crypto ",
        "archive",
        "spanning-tree mst configuration",
        "vrf definition ",
        "vrf context ",
        "ip vrf ",
        "object-group ",
        "monitor session ",
        "track ",
        "key chain ",
        "snmp-server view ",
        "controller ",
        "redundancy",
        "ip dhcp pool ",
        "event manager applet ",
    ];
    !l.starts_with("no ") && OPENERS.iter().any(|o| l.starts_with(o) || l == o.trim_end())
        // `vlan 10` opens a block; `vlan internal allocation …` does not.
        && !(l.starts_with("vlan ") && !l[5..].trim().chars().next().is_some_and(|c| c.is_ascii_digit()))
}

/// Settings that hold more than one value, so a new one is added beside
/// the others instead of replacing them.
fn many_valued(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    ["ip helper-address", "permit", "deny", "remark", "neighbor ", "network ", "seq ", "ip address " /* secondary handled below */, "standby ", "vrrp ", "logging host", "logging server", "ntp server", "snmp-server host", "ip route ", "ipv6 route ", "ip name-server", "match ", "set ", "channel-group"]
        .iter()
        .any(|p| l.starts_with(p))
        && !(l.starts_with("ip address ") && !l.ends_with(" secondary"))
}

/// What identifies a setting, so a second value for it replaces the
/// first: `description`, `switchport access vlan`, `ip address`,
/// `hostname`…
fn key_of(line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    let take = match words.first().map(|w| w.to_ascii_lowercase()).as_deref() {
        Some("switchport") => match words.get(1).map(|w| w.to_ascii_lowercase()).as_deref() {
            Some("access") | Some("voice") => 3,
            Some("trunk") => 4.min(words.len()).max(3),
            _ => 2,
        },
        Some("ip") | Some("ipv6") | Some("spanning-tree") | Some("storm-control") | Some("power") | Some("lldp") | Some("cdp") => 2,
        Some("speed") | Some("duplex") | Some("mtu") | Some("description") | Some("hostname") | Some("shutdown") | Some("name") | Some("encapsulation") | Some("bandwidth") => 1,
        _ => 2,
    };
    words.iter().take(take.min(words.len())).map(|w| w.to_ascii_lowercase()).collect::<Vec<_>>().join(" ")
}

/// Whether `existing` is what `no <named>` removes: the words named,
/// in order, at its start (`no description` removes `description X`;
/// `no shutdown` removes `shutdown`).
fn removed_by(existing: &str, named: &str) -> bool {
    let e: Vec<String> = existing.split_whitespace().map(|w| w.to_ascii_lowercase()).collect();
    let n: Vec<String> = named.split_whitespace().map(|w| w.to_ascii_lowercase()).collect();
    !n.is_empty() && e.len() >= n.len() && e[..n.len()] == n[..]
}

/// Applies one line inside a block's children (or the top level).
fn apply_line(lines: &mut Vec<Node>, line: &str) {
    let t = line.trim();
    if let Some(named) = t.strip_prefix("no ") {
        // `no shutdown` on a block that has no `shutdown` is the device's
        // default; nothing to remove. A `no` that names a block removes it.
        let named = canonical(named);
        lines.retain(|n| !removed_by(&n.text, &named));
        return;
    }
    if t.eq_ignore_ascii_case("shutdown") {
        if !lines.iter().any(|n| n.text.eq_ignore_ascii_case("shutdown")) {
            lines.push(Node { text: "shutdown".into(), children: Vec::new() });
        }
        return;
    }
    if !many_valued(t) {
        let key = key_of(t);
        if let Some(n) = lines.iter_mut().find(|n| key_of(&n.text) == key && n.children.is_empty()) {
            n.text = t.to_string();
            return;
        }
    }
    if !lines.iter().any(|n| n.text == t) {
        lines.push(Node { text: t.to_string(), children: Vec::new() });
    }
}

/// The configuration after `commands` — the lines a person would type in
/// configuration mode, one per entry, sub-mode lines indented or not.
pub fn apply(before: &Tree, commands: &[String]) -> Tree {
    let mut after = before.clone();
    // The open block: its index among the top-level lines.
    let mut open: Option<usize> = None;
    for raw in commands {
        let indented = raw.starts_with(' ') || raw.starts_with('\t');
        let line = raw.trim();
        if line.is_empty() || line == "!" {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower == "exit" || lower == "end" {
            open = None;
            continue;
        }
        if !indented && opens_block(line) {
            let text = canonical(line);
            let at = match after.lines.iter().position(|n| n.text.eq_ignore_ascii_case(&text)) {
                Some(i) => i,
                None => {
                    after.lines.push(Node { text: text.clone(), children: Vec::new() });
                    after.lines.len() - 1
                }
            };
            open = Some(at);
            continue;
        }
        // A `no` naming a whole block at the top level removes it.
        if !indented && open.is_none() {
            if let Some(named) = line.strip_prefix("no ") {
                let named = canonical(named);
                if opens_block(&named) {
                    after.lines.retain(|n| !n.text.eq_ignore_ascii_case(&named));
                    continue;
                }
            }
        }
        match open {
            // A line typed flush left while a block is open goes into it
            // unless it is a setting only the top level has.
            Some(i) if indented || !global_only(line) => apply_line(&mut after.lines[i].children, line),
            _ => {
                open = None;
                apply_line(&mut after.lines, line);
            }
        }
    }
    after
}

/// Settings that only exist at the top level, so typing one leaves the
/// block that was open (as the device does).
fn global_only(line: &str) -> bool {
    let l = line.trim_start_matches("no ").to_ascii_lowercase();
    ["hostname", "ip route", "ipv6 route", "ip default-gateway", "ip domain", "ip name-server", "snmp-server", "logging ", "ntp ", "username ", "aaa ", "banner ", "service ", "clock ", "boot ", "enable ", "vtp ", "ip ssh", "ip http", "spanning-tree mode", "spanning-tree vlan", "errdisable", "cdp run", "lldp run"]
        .iter()
        .any(|p| l.starts_with(p))
}

/// The blocks and top-level lines that differ between two trees, each
/// changed block with its own line as context.
pub fn tree_diff(before: &Tree, after: &Tree) -> Vec<DiffLine> {
    let mut out = Vec::new();
    let find = |t: &Tree, text: &str| t.lines.iter().find(|n| n.text == text).cloned();
    let mut seen = HashSet::new();
    for node in before.lines.iter().chain(after.lines.iter()) {
        if !seen.insert(node.text.clone()) {
            continue;
        }
        match (find(before, &node.text), find(after, &node.text)) {
            (Some(b), Some(a)) if b == a => {}
            (Some(b), Some(a)) => {
                out.push(DiffLine::new(" ", &b.text));
                let bset: Vec<String> = b.children.iter().map(flat).collect();
                let aset: Vec<String> = a.children.iter().map(flat).collect();
                for l in &bset {
                    if !aset.contains(l) {
                        out.push(DiffLine::new("-", &format!(" {l}")));
                    }
                }
                for l in &aset {
                    if !bset.contains(l) {
                        out.push(DiffLine::new("+", &format!(" {l}")));
                    }
                }
            }
            (Some(b), None) => {
                out.push(DiffLine::new("-", &b.text));
                for c in &b.children {
                    out.push(DiffLine::new("-", &format!(" {}", flat(c))));
                }
            }
            (None, Some(a)) => {
                out.push(DiffLine::new("+", &a.text));
                for c in &a.children {
                    out.push(DiffLine::new("+", &format!(" {}", flat(c))));
                }
            }
            (None, None) => {}
        }
    }
    out
}

fn flat(n: &Node) -> String {
    if n.children.is_empty() {
        n.text.clone()
    } else {
        format!("{} {{{}}}", n.text, n.children.iter().map(flat).collect::<Vec<_>>().join("; "))
    }
}

/// The commands that turn `after` back into `before`, block by block:
/// lines added are removed with `no`, lines removed are typed again, a
/// block that was created is removed, a block that was removed is typed
/// again whole.
pub fn reverse(before: &Tree, after: &Tree) -> Vec<String> {
    let mut out = Vec::new();
    let find = |t: &Tree, text: &str| t.lines.iter().find(|n| n.text == text).cloned();
    let mut seen = HashSet::new();
    for node in after.lines.iter().chain(before.lines.iter()) {
        if !seen.insert(node.text.clone()) {
            continue;
        }
        match (find(before, &node.text), find(after, &node.text)) {
            (Some(b), Some(a)) if b == a => {}
            (Some(b), Some(a)) if b.children.is_empty() && a.children.is_empty() => {
                out.push(b.text.clone());
            }
            (Some(b), Some(a)) => {
                out.push(b.text.clone());
                for c in &a.children {
                    if !b.children.iter().any(|x| x.text == c.text) && !b.children.iter().any(|x| key_of(&x.text) == key_of(&c.text) && !many_valued(&x.text)) {
                        out.push(format!(" {}", negate(&c.text)));
                    }
                }
                for c in &b.children {
                    if !a.children.iter().any(|x| x.text == c.text) {
                        out.push(format!(" {}", c.text));
                    }
                }
                out.push("exit".into());
            }
            (None, Some(a)) => out.push(negate(&a.text)),
            (Some(b), None) => {
                out.push(b.text.clone());
                for c in &b.children {
                    out.push(format!(" {}", c.text));
                }
                if !b.children.is_empty() {
                    out.push("exit".into());
                }
            }
            (None, None) => {}
        }
    }
    out
}

/// `no <line>`, or the line without its `no`.
pub fn negate(line: &str) -> String {
    match line.strip_prefix("no ") {
        Some(rest) => rest.to_string(),
        None => format!("no {line}"),
    }
}

/// Whether every line of `commands` took effect in `after`: each setting
/// is there, in its block, and each `no …` left nothing it names.
pub fn took_effect(after: &Tree, commands: &[String]) -> Result<(), Vec<String>> {
    let mut missing = Vec::new();
    let mut open: Option<String> = None;
    for raw in commands {
        let indented = raw.starts_with(' ') || raw.starts_with('\t');
        let line = raw.trim();
        let lower = line.to_ascii_lowercase();
        if line.is_empty() || line == "!" {
            continue;
        }
        if lower == "exit" || lower == "end" {
            open = None;
            continue;
        }
        if !indented && opens_block(line) {
            let text = canonical(line);
            if !after.lines.iter().any(|n| n.text.eq_ignore_ascii_case(&text)) {
                missing.push(text.clone());
            }
            open = Some(text);
            continue;
        }
        let scope: Vec<String> = match &open {
            Some(b) if indented || !global_only(line) => after.lines.iter().find(|n| n.text.eq_ignore_ascii_case(b)).map(|n| n.children.iter().map(|c| c.text.clone()).collect()).unwrap_or_default(),
            _ => {
                open = None;
                after.lines.iter().map(|n| n.text.clone()).collect()
            }
        };
        let ok = match line.strip_prefix("no ") {
            // `no shutdown`: the device prints nothing for it.
            Some(named) => {
                let named = canonical(named);
                !scope.iter().any(|l| removed_by(l, &named))
            }
            None => scope.iter().any(|l| l.eq_ignore_ascii_case(line)),
        };
        if !ok {
            missing.push(match &open {
                Some(b) => format!("{b} / {line}"),
                None => line.to_string(),
            });
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// A line diff of two texts: the changed runs with three lines of
/// context around each, as a person reads `diff -u`.
pub fn line_diff(before: &str, after: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = before.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !is_volatile(l)).collect();
    let b: Vec<&str> = after.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !is_volatile(l)).collect();
    // The common head and tail first: a change touches a few lines of a
    // configuration of thousands.
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let ops = lcs_ops(am, bm);
    let mut full: Vec<DiffLine> = a[..head].iter().map(|l| DiffLine::new(" ", l)).collect();
    full.extend(ops);
    full.extend(a[a.len() - tail..].iter().map(|l| DiffLine::new(" ", l)));
    // Context of three lines around each change.
    let changed: Vec<usize> = full.iter().enumerate().filter(|(_, d)| d.mark != " ").map(|(i, _)| i).collect();
    if changed.is_empty() {
        return Vec::new();
    }
    let keep = |i: usize| changed.iter().any(|c| i + 3 >= *c && i <= c + 3);
    let mut out = Vec::new();
    let mut gap = false;
    for (i, d) in full.into_iter().enumerate() {
        if keep(i) {
            if gap && !out.is_empty() {
                out.push(DiffLine::new(" ", "…"));
            }
            gap = false;
            out.push(d);
        } else {
            gap = true;
        }
    }
    out
}

/// Lines that change on their own between two reads (a timestamp in a
/// header), which a diff would otherwise always show.
fn is_volatile(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("! Last configuration change")
        || t.starts_with("! NVRAM config last updated")
        || t.starts_with("!Time:")
        || t.starts_with("Current configuration")
        || t.starts_with("Building configuration")
        || t.starts_with("## Last commit:")
        || t.starts_with("#config-version=")
}

fn lcs_ops(a: &[&str], b: &[&str]) -> Vec<DiffLine> {
    // A middle too long to compare line by line is shown as removed and
    // added whole; a single change never comes near this.
    if a.len() * b.len() > 4_000_000 {
        let mut out: Vec<DiffLine> = a.iter().map(|l| DiffLine::new("-", l)).collect();
        out.extend(b.iter().map(|l| DiffLine::new("+", l)));
        return out;
    }
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] { t[i + 1][j + 1] + 1 } else { t[i + 1][j].max(t[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(DiffLine::new(" ", a[i]));
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            out.push(DiffLine::new("-", a[i]));
            i += 1;
        } else {
            out.push(DiffLine::new("+", b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| DiffLine::new("-", l)));
    out.extend(b[j..].iter().map(|l| DiffLine::new("+", l)));
    out
}

/// A diff as text, `+`/`-`/` ` in front of each line.
pub fn render(diff: &[DiffLine]) -> String {
    diff.iter().map(|d| format!("{}{}\n", d.mark, d.text)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNING: &str = "Building configuration...\n\nCurrent configuration : 512 bytes\n!\nversion 15.2\nhostname SW1\n!\ninterface GigabitEthernet1/0/1\n description uplink\n switchport mode trunk\n!\ninterface GigabitEthernet1/0/24\n description spare\n shutdown\n!\ninterface Vlan10\n ip address 192.0.2.10 255.255.255.0\n!\nip default-gateway 192.0.2.1\nline vty 0 4\n login local\n transport input ssh\nend\n";

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_running_config_reads_as_blocks_without_its_headers() {
        let t = parse(RUNNING);
        let tops: Vec<&str> = t.lines.iter().map(|n| n.text.as_str()).collect();
        assert_eq!(tops, ["hostname SW1", "interface GigabitEthernet1/0/1", "interface GigabitEthernet1/0/24", "interface Vlan10", "ip default-gateway 192.0.2.1", "line vty 0 4"]);
        assert_eq!(t.lines[2].children.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["description spare", "shutdown"]);
    }

    #[test]
    fn a_description_replaces_the_one_there_and_the_diff_shows_both() {
        let before = parse(RUNNING);
        let after = apply(&before, &lines(&["interface Gi1/0/24", " description LAB-PORT", " no shutdown"]));
        let diff = render(&tree_diff(&before, &after));
        assert_eq!(diff, " interface GigabitEthernet1/0/24\n- description spare\n- shutdown\n+ description LAB-PORT\n");
        assert!(took_effect(&after, &lines(&["interface Gi1/0/24", " description LAB-PORT", " no shutdown"])).is_ok());
    }

    #[test]
    fn lines_typed_flush_left_go_into_the_open_block_until_a_global_one() {
        let before = parse(RUNNING);
        let after = apply(&before, &lines(&["interface GigabitEthernet1/0/24", "description X", "ip route 198.51.100.0 255.255.255.0 192.0.2.1"]));
        assert!(after.lines.iter().any(|n| n.text == "ip route 198.51.100.0 255.255.255.0 192.0.2.1"));
        assert!(after.lines[2].children.iter().any(|c| c.text == "description X"));
    }

    #[test]
    fn reverse_commands_put_the_block_back() {
        let before = parse(RUNNING);
        let cmds = lines(&["interface Gi1/0/24", " description LAB-PORT", " no shutdown", "exit", "ip route 198.51.100.0 255.255.255.0 192.0.2.1"]);
        let after = apply(&before, &cmds);
        let back = reverse(&before, &after);
        assert_eq!(back, lines(&["interface GigabitEthernet1/0/24", " description spare", " shutdown", "exit", "no ip route 198.51.100.0 255.255.255.0 192.0.2.1"]));
        assert_eq!(apply(&after, &back), before, "the reverse commands restore the configuration exactly");
    }

    #[test]
    fn a_new_block_is_removed_and_a_removed_block_is_typed_again() {
        let before = parse(RUNNING);
        let after = apply(&before, &lines(&["interface Loopback0", " description test", "exit", "no interface Vlan10"]));
        let back = reverse(&before, &after);
        assert!(back.contains(&"no interface Loopback0".to_string()), "{back:?}");
        assert!(back.windows(2).any(|w| w[0] == "interface Vlan10" && w[1] == " ip address 192.0.2.10 255.255.255.0"), "{back:?}");
        // A block typed again lands at the end; the device files it in
        // its own order, so the order of blocks is not compared.
        let sorted = |t: Tree| {
            let mut l = t.lines;
            l.sort_by(|a, b| a.text.cmp(&b.text));
            l
        };
        assert_eq!(sorted(apply(&after, &back)), sorted(before));
    }

    #[test]
    fn took_effect_names_what_the_device_did_not_take() {
        let before = parse(RUNNING);
        let missing = took_effect(&before, &lines(&["interface Gi1/0/24", " description LAB-PORT"])).unwrap_err();
        assert_eq!(missing, ["interface GigabitEthernet1/0/24 / description LAB-PORT"]);
    }

    #[test]
    fn interface_abbreviations_are_spelled_out() {
        assert_eq!(full_interface("Gi1/0/5"), "GigabitEthernet1/0/5");
        assert_eq!(full_interface("te1/1/1"), "TenGigabitEthernet1/1/1");
        assert_eq!(full_interface("Po10"), "Port-channel10");
        assert_eq!(full_interface("vlan 10"), "Vlan10");
        assert_eq!(canonical("int gi1/0/5"), "interface GigabitEthernet1/0/5");
    }

    #[test]
    fn a_line_diff_shows_the_change_with_its_context_and_ignores_timestamps() {
        let before = "! Last configuration change at 10:00\nhostname SW1\na\nb\nc\nd\ne\nf\ng\nh\n";
        let after = "! Last configuration change at 10:05\nhostname SW1\na\nb\nc\nd2\ne\nf\ng\nh\n";
        let d = render(&line_diff(before, after));
        assert_eq!(d, " a\n b\n c\n-d\n+d2\n e\n f\n g\n");
        assert!(line_diff("x\ny\n", "x\ny\n").is_empty());
    }
}
