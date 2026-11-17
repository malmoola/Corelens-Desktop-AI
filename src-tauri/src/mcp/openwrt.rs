//! OpenWrt for `run_show_command`: how it is recognised, the commands it
//! may be asked, and the secrets its configuration syntax carries.
//!
//! An OpenWrt router (GL.iNet's firmware among them) logs in to a root
//! BusyBox `ash`, not a network CLI, so its list is stricter than a
//! `show …` rule: a fixed set of read commands, each with only the
//! arguments named here, and pipes only into `grep`, `head` or `tail` with
//! no file arguments. Nothing a shell would treat as a second command, a
//! redirection, a substitution or a variable gets through.
//!
//! Recognition is two steps: the first version spelling comes back as
//! ash's own refusal (`-ash: show: not found`), and only then is
//! `/etc/openwrt_release` read, which must say `DISTRIB_ID='OpenWrt'`.
//!
//! Built from the OpenWrt 21.02 documentation; its fixtures are
//! reconstructed, not captured, and say so.

use std::sync::OnceLock;

use regex::Regex;

use super::redact::MARK;

pub const NAME: &str = "OpenWrt";

/// Sent only after a reply already came from ash.
pub const RELEASE: &str = "cat /etc/openwrt_release";

/// Whether a reply came from BusyBox's ash: how it refuses a command it
/// does not have (`-ash: show: not found`), or its login banner.
pub fn is_ash(reply: &str) -> bool {
    reply.contains("built-in shell (ash)")
        || reply.lines().take(5).any(|l| {
            let t = l.trim();
            (t.starts_with("-ash: ") || t.starts_with("ash: ")) && t.ends_with(": not found")
        })
}

/// Whether `/etc/openwrt_release` names OpenWrt.
pub fn is_release(text: &str) -> bool {
    text.lines().any(|l| {
        l.trim().strip_prefix("DISTRIB_ID=").map(|v| v.trim().trim_matches(|c| c == '\'' || c == '"')) == Some("OpenWrt")
    })
}

/// What `allows` lets through, worded for a refusal.
pub const LISTED: &str = "uci show [name], uci get <name>, ubus call system board, ip -4 addr, ip -4 route, wg show [interface], \
iptables -S, iptables -t nat -S, nft list ruleset, netstat -lntup, opkg list-installed, logread, cat /etc/config/<name>, \
df -h, free, ps; piped only into grep, head or tail";

/// Characters no OpenWrt command here needs, and which a shell would act on.
const SHELL: &[char] = &[';', '&', '`', '$', '>', '<', '(', ')', '{', '}', '\\', '\n', '\r', '\0'];

/// Words split as ash would, quotes respected. `None` on an unclosed quote.
fn words(segment: &str) -> Option<Vec<(String, bool)>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = segment.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => {
                let close = c;
                started = true;
                quoted = true;
                loop {
                    match chars.next()? {
                        x if x == close => break,
                        x => cur.push(x),
                    }
                }
            }
            c if c.is_whitespace() => {
                if started {
                    out.push((std::mem::take(&mut cur), quoted));
                    started = false;
                    quoted = false;
                }
            }
            c => {
                started = true;
                cur.push(c);
            }
        }
    }
    if started {
        out.push((cur, quoted));
    }
    Some(out)
}

/// The command split at its pipes, a `|` inside quotes not being one.
fn pipeline(command: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quote) = (0, None);
    for (i, c) in command.char_indices() {
        match (quote, c) {
            (None, '\'' | '"') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '|') => {
                out.push(&command[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&command[start..]);
    out
}

/// A uci name: `network`, `network.lan.ipaddr`, `wireless.@wifi-iface[0].ssid`.
fn uci_name(w: &str) -> bool {
    !w.is_empty() && !w.starts_with(['-', '.']) && w.chars().all(|c| c.is_ascii_alphanumeric() || "_.@[]-".contains(c))
}

/// An interface, routing table or config file name.
fn plain_name(w: &str) -> bool {
    !w.is_empty() && !w.starts_with('-') && w.len() <= 32 && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn number(w: &str) -> bool {
    !w.is_empty() && w.len() <= 6 && w.chars().all(|c| c.is_ascii_digit())
}

/// The command before any pipe.
fn base_allowed(w: &[&str]) -> bool {
    match w {
        ["uci", "show"] | ["ubus", "call", "system", "board"] | ["iptables", "-S"] | ["iptables", "-t", "nat", "-S"] | ["nft", "list", "ruleset"] => true,
        ["netstat", "-lntup"] | ["opkg", "list-installed"] | ["logread"] | ["df", "-h"] | ["free"] | ["ps"] | ["wg", "show"] => true,
        ["uci", "show", name] | ["uci", "get", name] => uci_name(name),
        // `wg show wg0` hides the keys; `wg show wg0 private-key` and
        // `wg showconf` print them, and are not here.
        ["wg", "show", iface] => plain_name(iface) && *iface != "all",
        ["ip", "-4", "addr" | "route"] | ["ip", "-4", "addr" | "route", "show"] => true,
        ["ip", "-4", "addr", "show", "dev", n] | ["ip", "-4", "route", "show", "dev" | "table", n] => plain_name(n),
        ["cat", path] => path.strip_prefix("/etc/config/").is_some_and(plain_name),
        _ => false,
    }
}

/// One filter after a pipe: `grep` with one pattern and its read-only
/// flags, `head`/`tail` with a line count. No file names, so a filter can
/// never read something else.
fn filter_allowed(w: &[(String, bool)]) -> bool {
    let Some(((verb, verb_quoted), args)) = w.split_first() else { return false };
    if *verb_quoted {
        return false;
    }
    // An unquoted glob would be expanded into file names, and those read.
    if args.iter().any(|(a, q)| !q && a.contains(['*', '?', '['])) {
        return false;
    }
    match verb.as_str() {
        "grep" => {
            let mut patterns = 0;
            let mut it = args.iter();
            while let Some((a, quoted)) = it.next() {
                if !quoted && a.starts_with('-') && a.len() > 1 {
                    match a.as_str() {
                        "-e" => {
                            if it.next().is_none() {
                                return false;
                            }
                            patterns += 1;
                        }
                        "-A" | "-B" | "-C" | "-m" => {
                            if !it.next().is_some_and(|(n, _)| number(n)) {
                                return false;
                            }
                        }
                        f if f[1..].chars().all(|c| "ivEFwnco".contains(c)) => {}
                        _ => return false,
                    }
                } else {
                    patterns += 1;
                }
            }
            patterns == 1
        }
        "head" | "tail" => match args {
            [] => true,
            [(n, false)] => n.strip_prefix('-').is_some_and(number),
            [(flag, false), (n, false)] => flag == "-n" && number(n),
            _ => false,
        },
        _ => false,
    }
}

/// Whether OpenWrt's list allows the command.
pub fn allows(command: &str) -> Result<(), String> {
    let refused = |why: &str| Err(format!("`{}` was refused: {why}. On {NAME} only these can be run: {LISTED}.", command.trim()));
    let c = command.trim();
    if c.is_empty() {
        return refused("it is empty");
    }
    if c.contains(SHELL) || c.contains("||") {
        return refused("it carries a shell operator, redirection or substitution");
    }
    let mut segments = pipeline(c).into_iter();
    let base = segments.next().unwrap_or("");
    let Some(base_words) = words(base) else { return refused("a quote is not closed") };
    if base_words.iter().any(|(_, q)| *q) {
        return refused("it is not one of the read-only commands");
    }
    let base_words: Vec<&str> = base_words.iter().map(|(w, _)| w.as_str()).collect();
    if !base_allowed(&base_words) {
        return refused("it is not one of the read-only commands");
    }
    for seg in segments {
        match words(seg) {
            Some(w) if filter_allowed(&w) => {}
            Some(_) => return refused("a pipe may only feed grep (one pattern), head or tail"),
            None => return refused("a quote is not closed"),
        }
    }
    Ok(())
}

fn rules() -> &'static [Regex] {
    static R: OnceLock<Vec<Regex>> = OnceLock::new();
    R.get_or_init(|| {
        // The secret option names: WireGuard's keys, Wi-Fi keys and PSKs,
        // PPP and SAE passwords, RADIUS secrets. Not `public_key`.
        let names = r"(?:private_key|preshared_key|psk|password|key|key[1-4]|secret|[A-Za-z0-9]+_(?:password|psk|secret))";
        [
            // `uci show`: `network.wgclient.private_key='…'`.
            format!(r"(?m)^(\s*[A-Za-z0-9_.@\[\]-]+\.{names}=).*$"),
            // `/etc/config/*`: `option key '…'`, `list key "…"`.
            format!(r"(?m)^(\s*(?:option|list)\s+{names}\s+).*$"),
            // `wg show` prints `(hidden)`, but never trust that.
            r"(?m)^(\s*(?:private|preshared) key:\s*).*$".to_string(),
        ]
        .iter()
        .map(|p| Regex::new(p).expect("a fixed regex"))
        .collect()
    })
}

/// The text with the values of OpenWrt's secret options removed.
pub fn scrub(text: &str) -> String {
    let mut out = text.to_string();
    for re in rules() {
        out = re.replace_all(&out, |c: &regex::Captures| format!("{}'{MARK}'", &c[1])).into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE_2102: &str = include_str!("fixtures/openwrt-21.02/openwrt_release.txt");
    const SHOW_VERSION: &str = include_str!("fixtures/openwrt-21.02/show_version.txt");
    const BANNER: &str = include_str!("fixtures/openwrt-21.02/login_banner.txt");
    const UCI_NETWORK: &str = include_str!("fixtures/openwrt-21.02/uci_show_network.txt");
    const WIRELESS: &str = include_str!("fixtures/openwrt-21.02/etc_config_wireless.txt");
    const WG_SHOW: &str = include_str!("fixtures/openwrt-21.02/wg_show.txt");
    const BOARD: &str = include_str!("fixtures/openwrt-21.02/ubus_call_system_board.txt");

    #[test]
    fn an_openwrt_router_is_recognised_by_its_shell_and_its_release_file() {
        assert!(is_ash(SHOW_VERSION));
        assert!(is_ash(BANNER));
        assert!(is_release(RELEASE_2102));
        assert!(is_release("DISTRIB_ID=\"OpenWrt\"\n") && is_release("DISTRIB_ID=OpenWrt\n"));
        // Other shells and other systems are not.
        assert!(!is_ash("-bash: show: command not found"));
        assert!(!is_ash("% Invalid input detected at '^' marker."));
        assert!(!is_ash("Cisco IOS Software, Version 15.2(4)E7"));
        assert!(!is_release("DISTRIB_ID='Ubuntu'\nDISTRIB_RELEASE=22.04\n"));
        assert!(!is_release("cat: can't open '/etc/openwrt_release': No such file or directory"));
        assert!(!is_release("# DISTRIB_ID='OpenWrt' in a comment, said by something else"));
        // The board says the same, for whoever asks it.
        assert!(BOARD.contains("\"distribution\": \"OpenWrt\""));
    }

    #[test]
    fn only_the_listed_reads_get_through() {
        for ok in [
            "uci show",
            "uci show network",
            "uci show wireless.@wifi-iface[0]",
            "uci get network.lan.ipaddr",
            "ubus call system board",
            "ip -4 addr",
            "ip -4 addr show dev br-lan",
            "ip -4 route",
            "ip -4 route show table main",
            "wg show",
            "wg show wgclient",
            "iptables -S",
            "iptables -t nat -S",
            "nft list ruleset",
            "netstat -lntup",
            "opkg list-installed",
            "logread",
            "cat /etc/config/network",
            "cat /etc/config/wireless",
            "df -h",
            "free",
            "ps",
            "logread | grep -i wireguard",
            "logread | grep 'dropbear' | tail -n 50",
            "logread | tail -20",
            "opkg list-installed | grep -E \"wireguard|luci\"",
            "iptables -S | grep -v ACCEPT | head",
            "ps | grep -e dnsmasq",
            "logread | grep -C 3 -i fail",
        ] {
            assert!(allows(ok).is_ok(), "{ok}: {:?}", allows(ok));
        }
        for bad in [
            "",
            "uci set network.lan.ipaddr=192.0.2.9",
            "uci commit",
            "uci delete network.wan",
            "uci show; reboot",
            "uci show && reboot",
            "uci show || reboot",
            "uci show & reboot",
            "uci show > /tmp/x",
            "uci show >> /tmp/x",
            "cat < /etc/shadow",
            "echo $(reboot)",
            "uci show `reboot`",
            "uci show $HOME",
            "uci show\nreboot",
            "opkg install tcpdump",
            "opkg update",
            "reboot",
            "sysupgrade -n /tmp/fw.bin",
            "firstboot",
            "wg showconf wgclient",
            "wg show wgclient private-key",
            "wg show all",
            "wg set wgclient peer x remove",
            "ip -4 route add default via 192.0.2.9",
            "ip -4 addr flush dev br-lan",
            "ip link set eth0 down",
            "iptables -F",
            "iptables -t nat -A PREROUTING -j DROP",
            "nft flush ruleset",
            "cat /etc/shadow",
            "cat /etc/config/../shadow",
            "cat /etc/config/network /etc/shadow",
            "cat /etc/config/*",
            "logread -f",
            "logread | tee /tmp/x",
            "logread | sh",
            "logread | xargs reboot",
            "logread | grep root /etc/shadow",
            "logread | grep -f /etc/shadow",
            "logread | grep *",
            "logread | tail -f",
            "logread | head /etc/shadow",
            "logread | grep 'unclosed",
            "'reboot'",
            "\"uci\" show",
            "show version",
            "ps; kill 1",
            "kill 1",
            "rm -rf /",
            "/sbin/reboot",
            "sh -c 'uci show'",
        ] {
            assert!(allows(bad).is_err(), "{bad:?} got through");
        }
        assert!(allows("uci commit").unwrap_err().contains("only these can be run: uci show"));
    }

    #[test]
    fn the_secrets_of_its_configuration_syntax_are_removed() {
        let uci = scrub(UCI_NETWORK);
        for gone in ["FIXTURE-pppoe-password-0001", "FIXTUREprivatekey", "FIXTUREpreshared"] {
            assert!(!uci.contains(gone), "{gone} survived:\n{uci}");
        }
        assert!(uci.contains(&format!("network.wgclient.private_key='{MARK}'")), "{uci}");
        assert!(uci.contains("network.lan.ipaddr='192.0.2.1'") && uci.contains("public_key='FIXTUREpublickey"), "{uci}");

        let wifi = scrub(WIRELESS);
        for gone in ["FIXTURE-wifi-key-0002", "FIXTURE-sae-0003", "FIXTURE-radius-0004"] {
            assert!(!wifi.contains(gone), "{gone} survived:\n{wifi}");
        }
        assert!(wifi.contains("option ssid 'FIXTURE-SSID'") && wifi.contains("option encryption 'psk2'"), "{wifi}");

        let wg = scrub(WG_SHOW);
        assert!(wg.contains(&format!("private key: '{MARK}'")) && wg.contains("public key: FIXTUREpublickey"), "{wg}");
        assert!(scrub("  private key: FIXTURE-shown-anyway=\n").contains(MARK));

        // Through the gate every result passes, too.
        let gated = super::super::redact::gate(UCI_NETWORK, &super::super::redact::Secrets::default()).unwrap();
        assert!(!gated.contains("FIXTURE-pppoe-password-0001") && !gated.contains("FIXTUREprivatekey"), "{gated}");
    }
}
