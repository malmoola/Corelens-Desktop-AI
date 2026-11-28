//! ICMP via a controlled invocation of the platform `ping` binary.
//!
//! Security note: the target is parsed by `validate::parse_target` first, and
//! the binary is invoked with an argument vector. There is no shell, no
//! `cmd.exe`, no string concatenation, and no user-controlled flags.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::types::{Outcome, ProbeResult};
use crate::validate::{parse_target, Target};

/// Which `ping` this machine has, because the three disagree about what `-W`
/// means.
///
/// A parameter rather than a `cfg!`, so every platform's argument vector can
/// be asserted from any machine. It was a `cfg!`, and the macOS branch was
/// wrong for months: tests run on Linux and Windows, and a flag that is wrong
/// on a third platform cannot be seen by a test that only asks the machine it
/// is running on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PingFlavour {
    /// `ping.exe`: `-n` for the count, `-w` a timeout in milliseconds.
    Windows,
    /// iputils, on Linux: `-W` a timeout in **seconds**.
    Linux,
    /// macOS and the BSDs: `-W` the wait for a reply in **milliseconds**.
    Bsd,
}

impl PingFlavour {
    pub fn here() -> Self {
        if cfg!(windows) {
            PingFlavour::Windows
        } else if cfg!(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly",
        )) {
            PingFlavour::Bsd
        } else {
            PingFlavour::Linux
        }
    }
}

/// The program that pings this target on a platform of this flavour.
///
/// macOS's `ping`, and the BSDs', speak IPv4 only: an IPv6 address given to
/// it is refused, so every ICMP check of one failed there. Theirs for IPv6
/// is `ping6`. iputils' `ping` and `ping.exe` take either family. A host
/// name stays with `ping`: which family it resolves to is not known here.
/// From the macOS manual pages; not run on a Mac.
pub fn ping_program_for(flavour: PingFlavour, target: &Target) -> &'static str {
    match (flavour, target) {
        (PingFlavour::Windows, _) => "ping.exe",
        (PingFlavour::Bsd, Target::Ip(ip)) if ip.is_ipv6() => "ping6",
        _ => "ping",
    }
}

/// Build the argument vector for a single-shot ping, for this machine.
pub fn ping_args(target: &Target, timeout_ms: u64) -> Vec<String> {
    ping_args_for(PingFlavour::here(), target, timeout_ms)
}

/// The same, for a named flavour. Pure, so the exact flags every platform gets
/// are covered by tests run anywhere.
pub fn ping_args_for(flavour: PingFlavour, target: &Target, timeout_ms: u64) -> Vec<String> {
    let t = target.as_str();
    match flavour {
        PingFlavour::Windows => {
            let mut args = vec!["-n".into(), "1".into(), "-w".into(), timeout_ms.max(1).to_string()];
            match target {
                Target::Ip(ip) if ip.is_ipv6() => args.push("-6".into()),
                Target::Ip(_) => args.push("-4".into()),
                Target::Host(_) => {}
            }
            args.push(t);
            args
        }
        // Seconds, rounded up: asking iputils for less than one is asking for
        // zero, which is no wait at all.
        PingFlavour::Linux => {
            let secs = timeout_ms.div_ceil(1000).max(1).to_string();
            vec!["-c".into(), "1".into(), "-W".into(), secs, t]
        }
        // `ping6`, for an IPv6 address (`ping_program_for`), has no wait
        // for a reply to set: its `-W` is another thing and takes no value.
        // One packet, and the wall clock in `ping_once` is its timeout.
        PingFlavour::Bsd if matches!(target, Target::Ip(ip) if ip.is_ipv6()) => {
            vec!["-c".into(), "1".into(), t]
        }
        // Milliseconds, as given. Rounding these up to a second would be
        // harmless; rounding them *down* to one is what broke the sweep.
        PingFlavour::Bsd => {
            vec!["-c".into(), "1".into(), "-W".into(), timeout_ms.max(1).to_string(), t]
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PingParse {
    pub outcome: Outcome,
    pub rtt_ms: Option<f64>,
    pub summary: String,
}

/// Parse ping output. Exit code alone is not enough: Windows `ping.exe` returns
/// 0 for "Destination host unreachable" in some cases, so the text is
/// authoritative and the exit code is a fallback.
pub fn parse_ping_output(stdout: &str, stderr: &str, exit_code: Option<i32>) -> PingParse {
    let text = format!("{stdout}\n{stderr}");
    let lower = text.to_lowercase();

    if lower.contains("could not find host")
        || lower.contains("name or service not known")
        || lower.contains("unknown host")
        || lower.contains("temporary failure in name resolution")
    {
        return PingParse {
            outcome: Outcome::DnsFailure,
            rtt_ms: None,
            summary: "Host name could not be resolved".into(),
        };
    }

    // A router on the way saying the packet's time to live ran out: a
    // loop, or a path longer than the packet was allowed. Not the target.
    if lower.contains("ttl expired in transit") || lower.contains("time to live exceeded") {
        return PingParse {
            outcome: Outcome::Unreachable,
            rtt_ms: None,
            summary: "A router on the way answered that the packet's time to live ran out (a routing loop, or too long a path); the target did not answer".into(),
        };
    }

    if lower.contains("destination host unreachable")
        || lower.contains("destination net unreachable")
        || lower.contains("destination unreachable")
        || lower.contains("network is unreachable")
    {
        return PingParse {
            outcome: Outcome::Unreachable,
            rtt_ms: None,
            summary: "Destination unreachable".into(),
        };
    }

    if lower.contains("request timed out")
        || lower.contains("100% packet loss")
        || lower.contains("100.0% packet loss")
        || lower.contains("request timeout")
    {
        return PingParse {
            outcome: Outcome::Timeout,
            rtt_ms: None,
            summary: "Request timed out".into(),
        };
    }

    if let Some(rtt) = extract_rtt(&lower) {
        return PingParse {
            outcome: Outcome::Success,
            rtt_ms: Some(rtt),
            summary: format!("Reply, {} ms", format_rtt(rtt)),
        };
    }

    match exit_code {
        // An exit code of 0 is not a reply by itself: `ping.exe` gives it
        // for anything that came back, an ICMP error from a router
        // included, in whatever language Windows speaks. An echo reply
        // carries the reply's own TTL (a hop limit, for IPv6) even where
        // its time could not be read; that is believed.
        Some(0) if lower.contains("ttl=") || lower.contains("hlim=") => PingParse {
            outcome: Outcome::Success,
            rtt_ms: None,
            summary: "Reply received".into(),
        },
        // Something answered, and nothing in it is an echo reply from the
        // target: an error from a router, worded in a way not known here.
        Some(0) => PingParse {
            outcome: Outcome::Unreachable,
            rtt_ms: None,
            summary: first_meaningful_reply(&text).unwrap_or_else(|| "Something answered, but not the target with an echo reply".into()),
        },
        _ => PingParse {
            outcome: Outcome::OsError,
            rtt_ms: None,
            summary: first_meaningful_line(&text)
                .unwrap_or_else(|| "Ping failed with no output".into()),
        },
    }
}

fn format_rtt(rtt: f64) -> String {
    if rtt < 1.0 {
        "<1".into()
    } else if rtt.fract() == 0.0 {
        format!("{rtt:.0}")
    } else {
        format!("{rtt:.1}")
    }
}

/// Handles `time=2ms`, `time<1ms`, `time=0.123 ms`, and `tiempo=2ms`-style
/// localized output by anchoring on `=`/`<` followed by digits and `ms`.
fn extract_rtt(lower: &str) -> Option<f64> {
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '=' || c == '<' {
            let rest = &lower[i + 1..];
            let trimmed = rest.trim_start();
            let num: String = trimmed
                .chars()
                .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
                .collect();
            if !num.is_empty() {
                let after = trimmed[num.len()..].trim_start();
                if after.starts_with("ms") {
                    let parsed: f64 = num.parse().ok()?;
                    // "time<1ms" means "under one millisecond".
                    return Some(if c == '<' { 0.5_f64.min(parsed) } else { parsed });
                }
            }
        }
        i += 1;
    }
    None
}

/// The line of ping's output that says what came back: the one after its
/// opening line, which only says what was pinged.
fn first_meaningful_reply(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .nth(1)
        .map(|l| l.chars().take(200).collect())
}

fn first_meaningful_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(200).collect())
}

/// A ping that could not be run or did not return, as opposed to one that ran
/// and reported the host unreachable.
#[derive(Debug, Clone)]
pub struct PingFailure {
    pub outcome: Outcome,
    pub summary: String,
}

/// Runs one ping and parses it. The single place a `ping` process is spawned,
/// so the sweep and the validation engine cannot drift apart on argument
/// construction, timeout handling, or console suppression.
pub async fn ping_once(target: &Target, timeout_ms: u64) -> Result<PingParse, PingFailure> {
    let program = ping_program_for(PingFlavour::here(), target);
    let mut cmd = Command::new(program);
    // In its own words, not the system language's: what came back is told
    // by its text. (`ping.exe` has no such switch; see `parse_ping_output`.)
    #[cfg(not(windows))]
    cmd.env("LC_ALL", "C");
    cmd.args(ping_args(target, timeout_ms))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    {
        // Suppress the console flash that ping.exe would otherwise cause.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    // Spawned and waited on explicitly rather than through `output()`.
    //
    // `output()` consumes the child, so a timeout can only drop the future —
    // and dropping is not enough. `kill_on_drop` does kill the process, but
    // tokio reaps orphans lazily, when its process driver is next polled. Once
    // validation stops nothing spawns, nothing polls, and the killed child sits
    // as `[ping] <defunct>` indefinitely. That was seen once during Phase 3,
    // written off as a transient reaping window, and is not transient: one
    // survived nearly two minutes after Stop.
    //
    // Holding the child means a timeout can kill *and* wait, which reaps it
    // there and then.
    let mut child = cmd.spawn().map_err(|e| PingFailure {
        outcome: Outcome::OsError,
        summary: format!("Could not run ping: {e}"),
    })?;

    // Hard wall-clock bound so a wedged child cannot hold a scheduler slot.
    let wall = Duration::from_millis(timeout_ms + 2_000);
    let status = match timeout(wall, child.wait()).await {
        Err(_) => {
            // kill() is start_kill followed by wait, so this reaps.
            let _ = child.kill().await;
            return Err(PingFailure {
                outcome: Outcome::Timeout,
                summary: "Ping did not return within the timeout".into(),
            });
        }
        Ok(Err(e)) => {
            let _ = child.kill().await;
            return Err(PingFailure {
                outcome: Outcome::OsError,
                summary: format!("Could not run ping: {e}"),
            });
        }
        Ok(Ok(status)) => status,
    };

    // Read after the exit. A single ping writes a few hundred bytes, far under
    // the pipe buffer, so nothing can have blocked waiting to be drained.
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout).await;
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr).await;
    }

    Ok(parse_ping_output(&stdout, &stderr, status.code()))
}

/// Run a single ICMP check. Cancellation is handled by the caller dropping the
/// future; the child process is killed on drop.
pub async fn probe_icmp(probe_id: &str, raw_target: &str, timeout_ms: u64, now_ms: i64) -> ProbeResult {
    let target = match parse_target(raw_target) {
        Ok(t) => t,
        Err(e) => {
            return ProbeResult::failed(probe_id, now_ms, Outcome::InvalidTarget, &e.to_string())
        }
    };

    let parsed = match ping_once(&target, timeout_ms).await {
        Ok(p) => p,
        Err(e) => return ProbeResult::failed(probe_id, now_ms, e.outcome, &e.summary),
    };

    ProbeResult {
        probe_id: probe_id.to_string(),
        timestamp_ms: now_ms,
        outcome: parsed.outcome,
        rtt_ms: parsed.rtt_ms,
        resolved: vec![],
        summary: parsed.summary.clone(),
        error_message: if parsed.outcome.is_success() {
            None
        } else {
            Some(parsed.summary)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_reply_is_parsed_with_rtt() {
        let out = "Pinging 10.10.10.1 with 32 bytes of data:\r\n\
                   Reply from 10.10.10.1: bytes=32 time=2ms TTL=255\r\n";
        let p = parse_ping_output(out, "", Some(0));
        assert_eq!(p.outcome, Outcome::Success);
        assert_eq!(p.rtt_ms, Some(2.0));
        assert_eq!(p.summary, "Reply, 2 ms");
    }

    #[test]
    fn sub_millisecond_reply_is_success() {
        let out = "Reply from 127.0.0.1: bytes=32 time<1ms TTL=128\r\n";
        let p = parse_ping_output(out, "", Some(0));
        assert_eq!(p.outcome, Outcome::Success);
        assert_eq!(p.summary, "Reply, <1 ms");
    }

    #[test]
    fn linux_reply_is_parsed() {
        let out = "64 bytes from 10.10.10.1: icmp_seq=1 ttl=64 time=0.123 ms\n";
        let p = parse_ping_output(out, "", Some(0));
        assert_eq!(p.outcome, Outcome::Success);
        assert_eq!(p.rtt_ms, Some(0.123));
    }

    // Verbatim output captured from iputils ping on Ubuntu 26.04, rather than
    // a hand-written approximation. The multi-line form matters: the parser
    // scans for the first '=' followed by "ms", so it has to step over
    // icmp_seq= and ttl= before reaching time=.
    #[test]
    fn real_iputils_success_output_is_parsed() {
        let out = "PING 127.0.0.1 (127.0.0.1) 56(84) bytes of data.\n\
64 bytes from 127.0.0.1: icmp_seq=1 ttl=64 time=0.044 ms\n\
\n\
--- 127.0.0.1 ping statistics ---\n\
1 packets transmitted, 1 received, 0% packet loss, time 0ms\n\
rtt min/avg/max/mdev = 0.044/0.044/0.044/0.000 ms\n";
        let p = parse_ping_output(out, "", Some(0));
        assert_eq!(p.outcome, Outcome::Success);
        assert_eq!(p.rtt_ms, Some(0.044));
    }

    #[test]
    fn real_iputils_timeout_output_is_a_timeout() {
        let out = "PING 192.0.2.1 (192.0.2.1) 56(84) bytes of data.\n\
\n\
--- 192.0.2.1 ping statistics ---\n\
1 packets transmitted, 0 received, 100% packet loss, time 0ms\n";
        let p = parse_ping_output(out, "", Some(1));
        assert_eq!(p.outcome, Outcome::Timeout);
        assert_eq!(p.rtt_ms, None);
    }

    #[test]
    fn real_iputils_dns_failure_is_dns_failure() {
        // iputils writes this to stderr and exits 2.
        let err = "ping: no-such-host.invalid: Name or service not known\n";
        let p = parse_ping_output("", err, Some(2));
        assert_eq!(p.outcome, Outcome::DnsFailure);
    }

    /// `ping.exe` exits 0 whenever anything at all came back, an ICMP error
    /// from a router included, and counts it as "Received = 1". Only an
    /// echo reply from the target is a reply: it has a time on it, or at
    /// the least a TTL. The outputs are reconstructed from ping.exe's
    /// documented messages, not captured, with documentation addresses.
    #[test]
    fn an_icmp_error_that_ping_counts_as_a_reply_is_not_a_reply() {
        // A routing loop: a router on the way answers that the packet's
        // time to live ran out.
        let looped = "\r\nPinging 192.0.2.9 with 32 bytes of data:\r\n\
                      Reply from 198.51.100.1: TTL expired in transit.\r\n\r\n\
                      Ping statistics for 192.0.2.9:\r\n    Packets: Sent = 1, Received = 1, Lost = 0 (0% loss),\r\n";
        let p = parse_ping_output(looped, "", Some(0));
        assert_eq!(p.outcome, Outcome::Unreachable, "{p:?}");
        // A Windows in another language: the words are not the ones looked
        // for, the exit code is still 0, and nothing in it is an echo reply.
        let german = "\r\nPing wird ausgef\u{fc}hrt f\u{fc}r 192.0.2.5 mit 32 Bytes Daten:\r\n\
                      Antwort von 192.0.2.1: Zielhost nicht erreichbar.\r\n\r\n\
                      Ping-Statistik f\u{fc}r 192.0.2.5:\r\n    Pakete: Gesendet = 1, Empfangen = 1, Verloren = 0\r\n";
        let p = parse_ping_output(german, "", Some(0));
        assert_ne!(p.outcome, Outcome::Success, "{p:?}");
        assert_eq!(p.outcome, Outcome::Unreachable);
        // The same Windows, the target answering: a reply, with its time.
        let answered = "Antwort von 192.0.2.5: Bytes=32 Zeit=3ms TTL=64\r\n";
        let p = parse_ping_output(answered, "", Some(0));
        assert_eq!((p.outcome, p.rtt_ms), (Outcome::Success, Some(3.0)));
        // An echo reply with no time that can be read is still one: it has
        // the reply's own TTL on it.
        let no_time = "Reply from 192.0.2.5: bytes=32 TTL=64\r\n";
        assert_eq!(parse_ping_output(no_time, "", Some(0)).outcome, Outcome::Success);
        // iputils says it in its own words, and exits 1.
        let linux = "From 198.51.100.1 icmp_seq=1 Time to live exceeded\n\n--- 192.0.2.9 ping statistics ---\n1 packets transmitted, 0 received, +1 errors, 100% packet loss, time 0ms\n";
        assert_eq!(parse_ping_output(linux, "", Some(1)).outcome, Outcome::Unreachable);
    }

    /// macOS's `ping` speaks IPv4 only; an IPv6 address is `ping6`'s, which
    /// has no flag for how long to wait for a reply (its `-W` means
    /// something else and takes no value), so the wall clock is its
    /// timeout. From the macOS manual pages; not run on a Mac.
    #[test]
    fn an_ipv6_address_goes_to_the_program_that_speaks_it() {
        let v4 = parse_target("192.0.2.1").unwrap();
        let v6 = parse_target("2001:db8::1").unwrap();
        let name = parse_target("core-sw1.example.test").unwrap();
        assert_eq!(ping_program_for(PingFlavour::Bsd, &v4), "ping");
        assert_eq!(ping_program_for(PingFlavour::Bsd, &v6), "ping6");
        assert_eq!(ping_program_for(PingFlavour::Bsd, &name), "ping");
        assert_eq!(ping_args_for(PingFlavour::Bsd, &v6, 1500), ["-c", "1", "2001:db8::1"]);
        // iputils and ping.exe take either family themselves.
        assert_eq!(ping_program_for(PingFlavour::Linux, &v6), "ping");
        assert_eq!(ping_program_for(PingFlavour::Windows, &v6), "ping.exe");
        assert_eq!(ping_program_for(PingFlavour::Windows, &v4), "ping.exe");
        assert_eq!(ping_args_for(PingFlavour::Linux, &v6, 1500), ["-c", "1", "-W", "2", "2001:db8::1"]);
    }

    #[test]
    fn timeout_is_distinct_from_unreachable() {
        let t = parse_ping_output("Request timed out.\r\n", "", Some(1));
        assert_eq!(t.outcome, Outcome::Timeout);
        let u = parse_ping_output(
            "Reply from 10.10.10.9: Destination host unreachable.\r\n",
            "",
            Some(0),
        );
        assert_eq!(u.outcome, Outcome::Unreachable);
    }

    #[test]
    fn dns_failure_is_distinct() {
        let p = parse_ping_output("Ping request could not find host nope.invalid.", "", Some(1));
        assert_eq!(p.outcome, Outcome::DnsFailure);
    }

    #[test]
    fn hundred_percent_loss_is_a_timeout() {
        let out = "1 packets transmitted, 0 received, 100% packet loss, time 0ms\n";
        assert_eq!(parse_ping_output(out, "", Some(1)).outcome, Outcome::Timeout);
    }

    #[test]
    fn unparseable_output_is_an_os_error_not_a_success() {
        let p = parse_ping_output("", "ping: socket: Operation not permitted", Some(2));
        assert_eq!(p.outcome, Outcome::OsError);
        assert!(p.summary.contains("Operation not permitted"));
    }

    /// The bug a Mac found and no test here could: `-W` is seconds to
    /// iputils and milliseconds to macOS and the BSDs, so one argument vector
    /// for "everything that is not Windows" asked a Mac to wait a single
    /// millisecond for a reply. Every host on a real network answered too late
    /// and read as silent — a sweep that finds nothing rather than one that
    /// fails, which is why it looked like it simply did not work.
    ///
    /// The flavour is a parameter so that all three can be asserted from any
    /// machine. CI tests on Linux and Windows; a flag wrong on a third
    /// platform is invisible to a test that only asks the machine it runs on.
    #[test]
    fn each_ping_is_asked_for_the_timeout_in_its_own_units() {
        let t = parse_target("192.0.2.1").unwrap();
        // iputils: seconds, rounded up, never zero.
        assert_eq!(ping_args_for(PingFlavour::Linux, &t, 1500), ["-c", "1", "-W", "2", "192.0.2.1"]);
        assert_eq!(ping_args_for(PingFlavour::Linux, &t, 200), ["-c", "1", "-W", "1", "192.0.2.1"]);
        // macOS and the BSDs: milliseconds, passed straight through.
        assert_eq!(ping_args_for(PingFlavour::Bsd, &t, 1500), ["-c", "1", "-W", "1500", "192.0.2.1"]);
        assert_eq!(ping_args_for(PingFlavour::Bsd, &t, 200), ["-c", "1", "-W", "200", "192.0.2.1"]);
        // A timeout of nothing is still a wait of something, on both.
        assert_eq!(ping_args_for(PingFlavour::Bsd, &t, 0)[3], "1");
        assert_eq!(ping_args_for(PingFlavour::Linux, &t, 0)[3], "1");
        // Windows: milliseconds, and the family is named explicitly.
        let win = ping_args_for(PingFlavour::Windows, &t, 1500);
        assert_eq!(win, ["-n", "1", "-w", "1500", "-4", "192.0.2.1"]);
    }

    #[test]
    fn the_platform_this_was_built_for_gets_its_own_flavour() {
        // Whatever machine runs this, `ping_args` must agree with the vector
        // for that machine's flavour — the one line tying the pure function
        // to the real one.
        let t = parse_target("192.0.2.1").unwrap();
        assert_eq!(ping_args(&t, 1000), ping_args_for(PingFlavour::here(), &t, 1000));
        assert_eq!(PingFlavour::here() == PingFlavour::Windows, cfg!(windows));
    }

    #[test]
    fn arg_vector_sends_exactly_one_probe() {
        let t = parse_target("10.10.10.1").unwrap();
        let args = ping_args(&t, 1000);
        assert!(args.contains(&"1".to_string()));
        assert_eq!(args.last().unwrap(), "10.10.10.1");
        // The target is always a single argv element, never spliced.
        assert_eq!(args.iter().filter(|a| a.contains("10.10.10.1")).count(), 1);
    }
}
