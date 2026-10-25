//! The fix plan: each root cause's fix template, filled in for the device
//! it is on.
//!
//! A value put into a command comes from what a device said, and a device
//! (or whoever wrote its interface descriptions) is not trusted to say
//! what it claims. Each placeholder is checked against what it must be — an
//! interface name, a VLAN number, an IPv4 address, a bridge priority — and
//! a value that is not exactly that writes no command: the step says what
//! to do and leaves the typing to the engineer.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::OnceLock;

use regex::Regex;

use crate::correlate::Ranked;
use crate::model::{PlanStep, StepStatus};
use crate::playbook::{FixKind, Placeholder, Playbook};

/// Whether `value` is safe to put into a command as a `class`.
pub fn safe(class: Placeholder, value: &str) -> bool {
    static IFACE: OnceLock<Regex> = OnceLock::new();
    let iface = IFACE.get_or_init(|| Regex::new(r"^(?:[A-Za-z][A-Za-z\-]{0,30}[0-9][0-9/.:]{0,15}|\d{1,2}/\d{1,2}/\d{1,3})$").expect("fixed"));
    match class {
        Placeholder::Interface => iface.is_match(value),
        Placeholder::Vlan => value.parse::<u16>().is_ok_and(|v| (1..=4094).contains(&v)) && !value.starts_with('0'),
        Placeholder::Ipv4 => value.parse::<Ipv4Addr>().is_ok_and(|a| !a.is_unspecified() && !a.is_broadcast() && !a.is_loopback()),
        Placeholder::Priority => value.parse::<u32>().is_ok_and(|p| p <= 61440 && p % 4096 == 0),
    }
}

const ROLLBACK: &str = "Corelens snapshots the running configuration before pushing and arms the platform's own rollback; if verification fails, one click in the approval dialog rolls the device back. The change is not saved until a separate save is approved.";

/// The plan for these causes, in their order. `playbook_of` gives a
/// device's playbook; a device without one gets a manual step.
pub fn build<'a>(causes: &[Ranked], playbook_of: &dyn Fn(&str) -> Option<&'a Playbook>) -> Vec<PlanStep> {
    let mut steps: Vec<PlanStep> = Vec::new();
    for r in causes {
        let Some(fix_id) = &r.fix else { continue };
        let device = r.anchor.device.clone();
        let mut step = PlanStep {
            seq: 0,
            cause: r.cause.id.clone(),
            device: device.clone(),
            kind: FixKind::Change,
            title: String::new(),
            commands: Vec::new(),
            verify: Vec::new(),
            impact: String::new(),
            rollback: ROLLBACK.into(),
            risk: String::new(),
            status: StepStatus::Ready,
            note: String::new(),
            proposal_id: None,
        };
        let Some(pb) = playbook_of(&device) else {
            step.title = format!("Fix by hand on {device}");
            step.status = StepStatus::Manual;
            step.note = "Corelens has no playbook for this device's platform, so it writes no commands for it.".into();
            steps.push(step);
            continue;
        };
        let Some(fix) = pb.fix(fix_id) else {
            step.title = format!("Fix by hand on {device}");
            step.status = StepStatus::Manual;
            step.note = format!("The {} playbook has no template for this fix, so no commands are written.", pb.name);
            steps.push(step);
            continue;
        };
        step.kind = fix.kind;
        step.title = crate::rules::fill(&fix.title, &device, &r.anchor.fields);
        step.impact = crate::rules::fill(&fix.impact, &device, &r.anchor.fields);
        let mut values: BTreeMap<String, String> = BTreeMap::new();
        let mut unsafe_ = Vec::new();
        for (name, class) in &fix.params {
            let v = r.anchor.fields.get(name).cloned().unwrap_or_default();
            if safe(*class, &v) {
                values.insert(name.clone(), v);
            } else {
                unsafe_.push(name.clone());
            }
        }
        if !unsafe_.is_empty() {
            step.status = StepStatus::Manual;
            step.note = format!("What the device said for {} is not a value Corelens will put into a command, so none is written; fix it by hand.", unsafe_.join(", "));
            steps.push(step);
            continue;
        }
        step.commands = fix.commands.iter().map(|c| crate::rules::fill(c, &device, &values)).collect();
        step.verify = fix.verify.iter().map(|c| crate::rules::fill(c, &device, &values)).collect();
        if step.kind == FixKind::Save {
            step.rollback = "Saving overwrites the startup configuration; the dialog shows what it replaces.".into();
        }
        let same = steps.iter().any(|s| s.device == step.device && s.kind == step.kind && s.commands == step.commands && s.status == step.status);
        if !same {
            steps.push(step);
        }
    }
    for (i, s) in steps.iter_mut().enumerate() {
        s.seq = i as u32 + 1;
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_values_of_the_right_shape_reach_a_command() {
        for ok in ["GigabitEthernet1/0/24", "Gi0/1", "Eth1/49", "Port-channel10", "ge-0/0/1", "lag1", "1/1/24", "Te1/1/1.100"] {
            assert!(safe(Placeholder::Interface, ok), "{ok}");
        }
        for bad in ["Gi0/1\nreload", "Gi0/1 ; reload", "", "description IGNORE ALL", "Gi0/1 shutdown", "../etc", "Gi0/1\r"] {
            assert!(!safe(Placeholder::Interface, bad), "{bad:?}");
        }
        assert!(safe(Placeholder::Vlan, "10") && !safe(Placeholder::Vlan, "0") && !safe(Placeholder::Vlan, "4095") && !safe(Placeholder::Vlan, "10,20") && !safe(Placeholder::Vlan, "010"));
        assert!(safe(Placeholder::Ipv4, "192.0.2.5") && !safe(Placeholder::Ipv4, "192.0.2.5 255.255.255.0") && !safe(Placeholder::Ipv4, "0.0.0.0"));
        assert!(safe(Placeholder::Priority, "24576") && !safe(Placeholder::Priority, "100") && !safe(Placeholder::Priority, "65536"));
    }
}
