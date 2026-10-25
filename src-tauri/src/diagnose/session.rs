//! One device's part of a diagnosis: log in, identify, ask the playbook's
//! commands in order, keep what each one said.
//!
//! Every command goes through the same checks as `run_show_command`'s
//! (the collector's read-only allowlist and the refusals every platform
//! shares) and must be one of the playbook's own, which are read-only by
//! the playbook's validation. A command the device refuses is kept as its
//! answer: the playbook says whether that means "not checked". When the
//! session drops partway, what was asked is kept and the rest is said to be
//! unasked, with why.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use corelens_diagnose::model::Output;
use corelens_diagnose::playbook::{Depth, PathValues, Playbook};
use corelens_discover::dialect::{family_of, identifies, Family, VERSION_COMMANDS};
use corelens_discover::hostkeys::HostKeyStore;
use corelens_discover::ssh::{connect_first_accepted, Credentials, SshOptions};

use crate::mcp::show;

pub const CONNECT: Duration = Duration::from_secs(10);
/// A ping or a traceroute takes longer than a show.
pub const COMMAND: Duration = Duration::from_secs(60);

/// The ntc platform a family's playbook is filed under.
pub fn ntc_platform(f: Family) -> Option<&'static str> {
    Some(match f {
        Family::CiscoIos => "cisco_ios",
        Family::CiscoNxOs => "cisco_nxos",
        Family::ArubaOsCx => "aruba_aoscx",
        Family::FortiOs => "fortinet",
        Family::PanOs => "paloalto_panos",
        Family::Junos => "juniper_junos",
        _ => return None,
    })
}

/// What one device came to.
#[derive(Debug, Default)]
pub struct Collected {
    pub platform: String,
    pub outputs: Vec<Output>,
    /// Why nothing was asked.
    pub unreachable: Option<String>,
}

/// Asks one device everything its playbook has for this depth.
#[allow(clippy::too_many_arguments)]
pub async fn collect(
    label: &str,
    address: &str,
    port: u16,
    logins: &[Credentials],
    store: Arc<Mutex<HostKeyStore>>,
    depth: Depth,
    path: Option<&PathValues>,
    progress: &(dyn Fn(usize, usize, &str) + Send + Sync),
) -> Collected {
    let lib = corelens_diagnose::Library::embedded();
    let options = SshOptions { port, connect_timeout: CONNECT, command_timeout: COMMAND, ..SshOptions::default() };
    progress(0, 0, "logging in");
    let (mut session, which) = match connect_first_accepted(address, logins, options, store, None).await {
        Ok(s) => s,
        Err(e) => return Collected { unreachable: Some(show::explain(address, port, &e)), ..Default::default() },
    };
    let mut version = String::new();
    for spelling in VERSION_COMMANDS {
        let out = session.run(spelling).await.unwrap_or_default();
        if identifies(&out) {
            version = out;
            break;
        }
    }
    let family = family_of(&version);
    let Some(platform) = ntc_platform(family) else {
        session.close().await;
        return Collected { unreachable: Some(format!("{label} ({address}) answered, but its platform is not one Corelens has a diagnose playbook for.")), ..Default::default() };
    };
    let Some(pb) = lib.playbook(platform) else {
        session.close().await;
        return Collected { platform: platform.into(), unreachable: Some(format!("Corelens has no diagnose playbook for {platform}.")), ..Default::default() };
    };
    // The configuration commands need privilege where a platform has enable.
    if matches!(family, Family::CiscoIos | Family::CiscoNxOs) && !session.prompt.enabled {
        let enable = logins.get(which).and_then(|c| c.enable_password.clone());
        let _ = session.enable(enable.as_ref()).await;
    }
    let plan = plan(pb, depth, path);
    let total = plan.len();
    let mut outputs = Vec::new();
    // A command two checks read is asked once.
    let mut asked: BTreeMap<String, Result<String, String>> = BTreeMap::new();
    let mut ended: Option<String> = None;
    for (i, (id, line)) in plan.into_iter().enumerate() {
        progress(i, total, &line);
        let answer = if let Some(why) = &ended {
            Err(format!("not asked: the session ended before it ({why})"))
        } else if let Some(a) = asked.get(&line) {
            a.clone()
        } else if line.eq_ignore_ascii_case("show version") && !version.is_empty() {
            Ok(version.clone())
        } else if let Err(why) = show::precheck(&line) {
            Err(why)
        } else {
            let a = session.run(&line).await.map_err(|e| show::explain(address, port, &e));
            if let Err(e) = &a {
                ended = Some(e.clone());
            }
            asked.insert(line.clone(), a.clone());
            a
        };
        let (text, error) = match answer {
            Ok(t) => (Some(t.replace("\r\n", "\n")), None),
            Err(e) => (None, Some(e)),
        };
        outputs.push(Output { command_id: id, command: line, text, error });
    }
    progress(total, total, "done");
    session.close().await;
    Collected { platform: platform.into(), outputs, unreachable: None }
}

/// The commands, in order, as they are sent; a path command whose values
/// it cannot take is left out.
pub fn plan(pb: &Playbook, depth: Depth, path: Option<&PathValues>) -> Vec<(String, String)> {
    pb.commands_for(depth, path.is_some()).filter_map(|c| c.line(path).map(|l| (c.id.clone(), l))).collect()
}
