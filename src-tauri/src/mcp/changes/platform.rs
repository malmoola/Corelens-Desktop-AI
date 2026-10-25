//! How each platform takes a change, and gives it back.
//!
//! One procedure per platform, the same five steps:
//!
//! | Platform | Before the push | Push | After verification | Rollback | Save |
//! | --- | --- | --- | --- | --- | --- |
//! | IOS / IOS-XE | `copy running-config flash:corelens-<id>.cfg`; with `archive` configured, `configure terminal revert timer` too | `configure terminal` … `end` | `configure confirm` (revert timer only) | `configure revert now` while unconfirmed, else `configure replace flash:… force`, else the reverse commands | `write memory` |
//! | NX-OS | `checkpoint corelens-<id>` | `configure terminal` … `end` | — | `rollback running-config checkpoint corelens-<id>`, else the reverse commands | `copy running-config startup-config` |
//! | AOS-CX | `copy running-config checkpoint corelens-<id>` | `configure terminal` … `end` | — | `checkpoint rollback corelens-<id>`, else the reverse commands | `write memory` |
//! | Junos | — | `configure exclusive`, the lines, `show \| compare`, `commit check`, `commit confirmed 5` | `commit` (confirms) | `rollback 1` + `commit`; unconfirmed, the device rolls back by itself after 5 minutes | already persistent |
//! | FortiOS | — | `execute config-transaction start`, the lines, `execute config-transaction commit` (abort on an error) | — | none once committed | already persistent |
//! | PAN-OS | `save config to corelens-<id>.xml` | `configure`, the lines, `commit` (`revert config` on an error) | — | `load config from corelens-<id>.xml` + `commit` | already persistent |
//!
//! **Built from vendor documentation**, as the dialect readers were: every
//! procedure has been run against Corelens's own simulator
//! ([`crate::mcp::fake_device`]) and none yet against hardware, and the
//! approval dialog says so. A device's own rollback is always tried first;
//! the reverse commands (computed from the snapshot and the configuration
//! read back after the push) are the fallback on the Cisco-style platforms.

use corelens_discover::dialect::Family;
use corelens_discover::ssh::Device;

/// How long Junos waits for a confirming commit before it rolls back.
pub const CONFIRM_MINUTES: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Platform {
    CiscoIos,
    CiscoNxOs,
    ArubaOsCx,
    Junos,
    FortiOs,
    PanOs,
}

impl Platform {
    pub fn from_family(f: Family) -> Option<Platform> {
        Some(match f {
            Family::CiscoIos => Platform::CiscoIos,
            Family::CiscoNxOs => Platform::CiscoNxOs,
            Family::ArubaOsCx => Platform::ArubaOsCx,
            Family::Junos => Platform::Junos,
            Family::FortiOs => Platform::FortiOs,
            Family::PanOs => Platform::PanOs,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Platform::CiscoIos => "Cisco IOS / IOS-XE",
            Platform::CiscoNxOs => "Cisco NX-OS",
            Platform::ArubaOsCx => "Aruba AOS-CX",
            Platform::Junos => "Junos",
            Platform::FortiOs => "FortiOS",
            Platform::PanOs => "PAN-OS",
        }
    }

    /// An indented, `no`-negated configuration, which [`super::diff`] reads.
    pub fn cisco_like(self) -> bool {
        matches!(self, Platform::CiscoIos | Platform::CiscoNxOs | Platform::ArubaOsCx)
    }

    /// The command that prints the running configuration as text.
    pub fn running_command(self) -> &'static str {
        match self {
            Platform::CiscoIos | Platform::CiscoNxOs | Platform::ArubaOsCx => "show running-config",
            Platform::Junos => "show configuration | display set",
            Platform::FortiOs => "show",
            Platform::PanOs => "show config running",
        }
    }

    /// What saving the running configuration is, or `None` when every
    /// change is already persistent (a commit).
    pub fn save_command(self) -> Option<&'static str> {
        match self {
            Platform::CiscoIos | Platform::ArubaOsCx => Some("write memory"),
            Platform::CiscoNxOs => Some("copy running-config startup-config"),
            Platform::Junos | Platform::FortiOs | Platform::PanOs => None,
        }
    }

    /// The rollback, as the dialog states it before approval.
    pub fn rollback_plan(self) -> &'static str {
        match self {
            Platform::CiscoIos => "Snapshot copied to flash first; rollback with configure replace (or the revert timer when archive is configured), else the reverse commands.",
            Platform::CiscoNxOs => "A checkpoint first; rollback to it, else the reverse commands.",
            Platform::ArubaOsCx => "A checkpoint first; rollback to it, else the reverse commands.",
            Platform::Junos => "Commit confirmed: unless Corelens confirms after verification, the device rolls back by itself after 5 minutes; rollback 1 any time.",
            Platform::FortiOs => "A configuration transaction, aborted on any error; there is no rollback once committed.",
            Platform::PanOs => "The configuration saved to a named file first; rollback loads it and commits.",
        }
    }

    /// Lines the procedure types itself, which a proposal must not.
    pub fn framing(self, line: &str) -> bool {
        let l = line.trim().to_ascii_lowercase();
        match self {
            Platform::CiscoIos | Platform::CiscoNxOs | Platform::ArubaOsCx => ["configure terminal", "conf t", "config t", "configure"].contains(&l.as_str()) || l == "end",
            Platform::Junos => l.starts_with("configure") || l.starts_with("commit") || l == "exit" || l == "quit",
            Platform::FortiOs => l.starts_with("execute config-transaction"),
            Platform::PanOs => l == "configure" || l.starts_with("commit") || l == "exit",
        }
    }
}

/// Whether a device said no: the refusals every reader already knows,
/// and the `error:` / `commit failed` of the commit-based platforms.
pub fn refused(output: &str) -> Option<String> {
    if let Some(r) = corelens_discover::cli::command_was_rejected(output) {
        return Some(r);
    }
    output
        .lines()
        .map(str::trim)
        .find(|l| {
            let l = l.to_ascii_lowercase();
            l.starts_with("error:") || l.contains("commit failed") || l.contains("configuration check-out failed") || l.starts_with("validation error") || l.contains("command fail")
        })
        .map(str::to_string)
}

/// What the device holds so a change can be taken back.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum Checkpoint {
    /// Nothing on the device: rollback is the reverse commands.
    None,
    /// IOS: a copy on flash, and whether the revert timer runs.
    Flash { file: String, revert_timer: bool },
    NxOs(String),
    AosCx(String),
    /// Junos: committed with a confirm timer; `confirmed` once Corelens
    /// has confirmed it.
    JunosConfirm { confirmed: bool },
    FortiTransaction,
    PanOsFile(String),
}

/// A command and what it printed, as the record keeps it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Exchange {
    pub command: String,
    pub output: String,
}

/// A step that failed: the commands so far and the line refused.
#[derive(Debug, Clone)]
pub struct Failed {
    pub transcript: Vec<Exchange>,
    pub why: String,
}

/// The few calls a procedure makes on a session.
pub struct Cli<'a> {
    device: &'a mut Device,
    pub transcript: Vec<Exchange>,
}

impl<'a> Cli<'a> {
    pub fn new(device: &'a mut Device) -> Cli<'a> {
        Cli { device, transcript: Vec::new() }
    }

    pub async fn run(&mut self, command: &str) -> Result<String, String> {
        let out = self.device.run(command).await.map_err(|e| e.to_string())?;
        self.transcript.push(Exchange { command: command.to_string(), output: out.clone() });
        Ok(out)
    }

    async fn run_answering(&mut self, command: &str, answers: &[(&str, &[u8])]) -> Result<String, String> {
        let out = self.device.run_answering(command, answers).await.map_err(|e| e.to_string())?;
        self.transcript.push(Exchange { command: command.to_string(), output: out.clone() });
        Ok(out)
    }

    /// Runs `command`, failing when the device refuses it.
    async fn must(&mut self, command: &str) -> Result<String, String> {
        let out = self.run(command).await?;
        match refused(&out) {
            Some(why) => Err(format!("`{command}` was refused: {why}")),
            None => Ok(out),
        }
    }

    pub fn take(&mut self) -> Vec<Exchange> {
        std::mem::take(&mut self.transcript)
    }
}

fn has_archive(running: &str) -> bool {
    super::diff::parse(running).lines.iter().any(|n| n.text.eq_ignore_ascii_case("archive") && n.children.iter().any(|c| c.text.to_ascii_lowercase().starts_with("path ")))
}

/// Whatever the platform keeps before a push, named for the change.
pub async fn prepare(cli: &mut Cli<'_>, platform: Platform, id: &str, running: &str) -> (Checkpoint, Vec<Exchange>) {
    let name = format!("corelens-{id}");
    let cp = match platform {
        Platform::CiscoIos => {
            let file = format!("flash:{name}.cfg");
            match cli.run_answering(&format!("copy running-config {file}"), &[("destination filename", b"\n"), ("[confirm]", b"\n")]).await {
                Ok(out) if refused(&out).is_none() => Checkpoint::Flash { file, revert_timer: has_archive(running) },
                // No copy (a full flash, a filesystem by another name):
                // the reverse commands are the rollback.
                _ => Checkpoint::None,
            }
        }
        Platform::CiscoNxOs => match cli.must(&format!("checkpoint {name}")).await {
            Ok(_) => Checkpoint::NxOs(name),
            Err(_) => Checkpoint::None,
        },
        Platform::ArubaOsCx => match cli.must(&format!("copy running-config checkpoint {name}")).await {
            Ok(_) => Checkpoint::AosCx(name),
            Err(_) => Checkpoint::None,
        },
        Platform::Junos => Checkpoint::JunosConfirm { confirmed: false },
        Platform::FortiOs => Checkpoint::FortiTransaction,
        Platform::PanOs => {
            let file = format!("{name}.xml");
            let ok = cli.must("configure").await.is_ok() && cli.must(&format!("save config to {file}")).await.is_ok();
            let _ = cli.run("exit").await;
            if ok {
                Checkpoint::PanOsFile(file)
            } else {
                Checkpoint::None
            }
        }
    };
    (cp, cli.take())
}

/// The lines, in the platform's framing. A refused line stops the push
/// there, and whatever was half-entered is discarded where the platform
/// can.
pub async fn push(cli: &mut Cli<'_>, platform: Platform, checkpoint: &Checkpoint, commands: &[String]) -> Result<Vec<Exchange>, Failed> {
    let result: Result<(), String> = async {
        match platform {
            Platform::CiscoIos | Platform::CiscoNxOs | Platform::ArubaOsCx => {
                let enter = match checkpoint {
                    Checkpoint::Flash { revert_timer: true, .. } => format!("configure terminal revert timer {CONFIRM_MINUTES}"),
                    _ => "configure terminal".to_string(),
                };
                cli.must(&enter).await?;
                for line in commands {
                    if let Err(e) = cli.must(line.trim_end()).await {
                        let _ = cli.run("end").await;
                        return Err(e);
                    }
                }
                cli.must("end").await?;
                Ok(())
            }
            Platform::Junos => {
                cli.must("configure exclusive").await?;
                for line in commands {
                    if let Err(e) = cli.must(line.trim()).await {
                        let _ = cli.run("rollback 0").await;
                        let _ = cli.run("exit").await;
                        return Err(e);
                    }
                }
                let _ = cli.run("show | compare").await;
                if let Err(e) = cli.must("commit check").await {
                    let _ = cli.run("rollback 0").await;
                    let _ = cli.run("exit").await;
                    return Err(e);
                }
                if let Err(e) = cli.must(&format!("commit confirmed {CONFIRM_MINUTES} comment \"corelens\"")).await {
                    let _ = cli.run("rollback 0").await;
                    let _ = cli.run("exit").await;
                    return Err(e);
                }
                cli.must("exit").await?;
                Ok(())
            }
            Platform::FortiOs => {
                cli.must("execute config-transaction start").await?;
                for line in commands {
                    if let Err(e) = cli.must(line.trim()).await {
                        let _ = cli.run("execute config-transaction abort").await;
                        return Err(e);
                    }
                }
                cli.must("execute config-transaction commit").await?;
                Ok(())
            }
            Platform::PanOs => {
                cli.must("configure").await?;
                for line in commands {
                    if let Err(e) = cli.must(line.trim()).await {
                        let _ = cli.run("revert config").await;
                        let _ = cli.run("exit").await;
                        return Err(e);
                    }
                }
                if let Err(e) = cli.must("commit").await {
                    let _ = cli.run("revert config").await;
                    let _ = cli.run("exit").await;
                    return Err(e);
                }
                cli.must("exit").await?;
                Ok(())
            }
        }
    }
    .await;
    let transcript = cli.take();
    match result {
        Ok(()) => Ok(transcript),
        Err(why) => Err(Failed { transcript, why }),
    }
}

/// Makes a verified change stick where the platform waits to be told.
pub async fn confirm(cli: &mut Cli<'_>, platform: Platform, checkpoint: &mut Checkpoint) -> Result<Vec<Exchange>, Failed> {
    let r: Result<(), String> = async {
        match (platform, &mut *checkpoint) {
            (Platform::CiscoIos, Checkpoint::Flash { revert_timer, .. }) if *revert_timer => {
                cli.must("configure confirm").await?;
                *revert_timer = false;
                Ok(())
            }
            (Platform::Junos, Checkpoint::JunosConfirm { confirmed }) if !*confirmed => {
                cli.must("configure exclusive").await?;
                let done = cli.must("commit").await;
                let _ = cli.run("exit").await;
                done?;
                *confirmed = true;
                Ok(())
            }
            _ => Ok(()),
        }
    }
    .await;
    let transcript = cli.take();
    r.map(|_| transcript.clone()).map_err(|why| Failed { transcript, why })
}

/// Puts the device back as it was before the push: its own mechanism
/// first, the reverse commands after that.
pub async fn rollback(cli: &mut Cli<'_>, platform: Platform, checkpoint: &Checkpoint, reverse: &[String]) -> Result<Vec<Exchange>, Failed> {
    let own: Result<(), String> = async {
        match (platform, checkpoint) {
            (Platform::CiscoIos, Checkpoint::Flash { file, revert_timer }) => {
                if *revert_timer && cli.must("configure revert now").await.is_ok() {
                    return Ok(());
                }
                let out = cli.run_answering(&format!("configure replace {file} force"), &[("[no]", b"yes\n"), ("[confirm]", b"\n")]).await?;
                match refused(&out) {
                    Some(why) => Err(why),
                    None => Ok(()),
                }
            }
            (Platform::CiscoNxOs, Checkpoint::NxOs(name)) => cli.must(&format!("rollback running-config checkpoint {name}")).await.map(|_| ()),
            (Platform::ArubaOsCx, Checkpoint::AosCx(name)) => cli.must(&format!("checkpoint rollback {name}")).await.map(|_| ()),
            (Platform::Junos, _) => {
                cli.must("configure exclusive").await?;
                let r = async {
                    cli.must("rollback 1").await?;
                    cli.must("commit").await.map(|_| ())
                }
                .await;
                let _ = cli.run("exit").await;
                r
            }
            (Platform::PanOs, Checkpoint::PanOsFile(file)) => {
                cli.must("configure").await?;
                let r = async {
                    cli.must(&format!("load config from {file}")).await?;
                    cli.must("commit").await.map(|_| ())
                }
                .await;
                let _ = cli.run("exit").await;
                r
            }
            (Platform::FortiOs, _) => Err("FortiOS keeps no copy to roll back to once a transaction is committed".into()),
            _ => Err("the device holds no copy to roll back to".into()),
        }
    }
    .await;
    if own.is_ok() {
        return Ok(cli.take());
    }
    if platform.cisco_like() && !reverse.is_empty() {
        let by_hand: Result<(), String> = async {
            cli.must("configure terminal").await?;
            for line in reverse {
                if let Err(e) = cli.must(line).await {
                    let _ = cli.run("end").await;
                    return Err(e);
                }
            }
            cli.must("end").await.map(|_| ())
        }
        .await;
        let transcript = cli.take();
        return by_hand.map(|_| transcript.clone()).map_err(|why| Failed { transcript, why });
    }
    let transcript = cli.take();
    Err(Failed { transcript, why: own.unwrap_err() })
}

/// Saves the running configuration as the one the device starts with.
pub async fn save(cli: &mut Cli<'_>, platform: Platform) -> Result<Vec<Exchange>, Failed> {
    let r = match platform.save_command() {
        Some(cmd) => cli.run_answering(cmd, &[("destination filename", b"\n"), ("[confirm]", b"\n")]).await.and_then(|out| match refused(&out) {
            Some(why) => Err(format!("`{cmd}` was refused: {why}")),
            None => Ok(()),
        }),
        None => Ok(()),
    };
    let transcript = cli.take();
    r.map(|_| transcript.clone()).map_err(|why| Failed { transcript, why })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_platform_the_live_reads_speak_but_one_takes_changes() {
        for (f, p) in [(Family::CiscoIos, Some(Platform::CiscoIos)), (Family::CiscoNxOs, Some(Platform::CiscoNxOs)), (Family::ArubaOsCx, Some(Platform::ArubaOsCx)), (Family::Junos, Some(Platform::Junos)), (Family::FortiOs, Some(Platform::FortiOs)), (Family::PanOs, Some(Platform::PanOs)), (Family::ArubaOsSwitch, None)] {
            assert_eq!(Platform::from_family(f), p, "{f:?}");
        }
    }

    #[test]
    fn a_proposal_never_types_the_framing_the_procedure_types() {
        assert!(Platform::CiscoIos.framing("configure terminal") && Platform::CiscoIos.framing("conf t") && Platform::CiscoIos.framing("end"));
        assert!(!Platform::CiscoIos.framing("interface Gi1/0/1"));
        assert!(Platform::Junos.framing("commit") && Platform::Junos.framing("configure exclusive"));
        assert!(!Platform::Junos.framing("set interfaces ge-0/0/1 description X"));
    }

    #[test]
    fn a_commit_error_is_a_refusal() {
        assert!(refused("error: configuration check-out failed\n").is_some());
        assert!(refused("% Invalid input detected at '^' marker.\n").is_some());
        assert!(refused("Building configuration...\n[OK]\n").is_none());
    }
}
