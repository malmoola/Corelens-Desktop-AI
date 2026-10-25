//! One device's checks: its outputs read, its playbook's rules applied.

use std::collections::BTreeMap;

use regex::Regex;
use serde_json::Value;

use crate::model::{DeviceInput, Finding, NotChecked};
use crate::parse::{self, Row, Templates};
use crate::playbook::{CheckSpec, Condition, Depth, Op, Playbook, Rule};

/// Device output on its way into a report: every piece passes this.
pub type Redact<'a> = &'a (dyn Fn(&str) -> String + Sync);

/// What one device's run came to.
#[derive(Debug, Default)]
pub struct DeviceEval {
    pub findings: Vec<Finding>,
    /// The checks that ran and found nothing.
    pub passed: Vec<String>,
    pub not_checked: Vec<NotChecked>,
    /// Table name → rows, in the tables' own field names.
    pub tables: BTreeMap<String, Vec<BTreeMap<String, String>>>,
    /// Command id → the rows it parsed into.
    pub rows: BTreeMap<String, Vec<Row>>,
}

fn holds(cond: &Condition, row: &Row) -> bool {
    let v = parse::text(row, &cond.field);
    let want = cond.value.as_ref();
    let want_s = || want.map(|w| match w {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    });
    let want_n = || want.and_then(|w| w.as_f64().or_else(|| w.as_str().and_then(parse::number)));
    match cond.op {
        Op::Eq => want_s().is_some_and(|w| v.eq_ignore_ascii_case(&w)),
        Op::Ne => want_s().is_some_and(|w| !v.eq_ignore_ascii_case(&w)),
        Op::In | Op::NotIn => {
            let list: Vec<String> = want.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_lowercase).collect()).unwrap_or_default();
            let found = list.contains(&v.to_lowercase());
            if cond.op == Op::In {
                found
            } else {
                !found
            }
        }
        Op::Matches | Op::NotMatches => {
            let Some(re) = want_s().and_then(|p| Regex::new(&p).ok()) else { return false };
            re.is_match(&v) == (cond.op == Op::Matches)
        }
        Op::Gt | Op::Ge | Op::Lt | Op::Le => {
            let (Some(n), Some(w)) = (parse::number(&v), want_n()) else { return false };
            match cond.op {
                Op::Gt => n > w,
                Op::Ge => n >= w,
                Op::Lt => n < w,
                _ => n <= w,
            }
        }
        Op::Empty => v.trim().is_empty(),
        Op::NotEmpty => !v.trim().is_empty(),
        Op::PctGt | Op::PctLt => {
            let of = cond.of.as_deref().map(|f| parse::text(row, f)).and_then(|t| parse::number(&t));
            match (parse::number(&v), of, want_n()) {
                (Some(n), Some(d), Some(w)) if d > 0.0 => {
                    let pct = n / d * 100.0;
                    if cond.op == Op::PctGt {
                        pct > w
                    } else {
                        pct < w
                    }
                }
                _ => false,
            }
        }
        Op::MinutesLt => match (parse::minutes(&v), want_n()) {
            (Some(m), Some(w)) => m < w,
            _ => false,
        },
    }
}

fn all_hold(when: &[Condition], row: &Row) -> bool {
    when.iter().all(|c| holds(c, row))
}

/// `{name}` replaced by the field, `{device}` by the device.
pub fn fill(template: &str, device: &str, fields: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &rest[start + 1..start + end];
        let value = if name == "device" { device.to_string() } else { fields.get(name).cloned().unwrap_or_else(|| "?".into()) };
        out.push_str(if value.is_empty() { "?" } else { &value });
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

pub fn finding_id(check: &str, device: &str, key: &[String], fields: &BTreeMap<String, String>) -> String {
    let mut parts = vec![check.to_string(), device.to_lowercase()];
    for k in key {
        parts.push(fields.get(k).map(|v| v.to_lowercase()).unwrap_or_default());
    }
    parts.join("|")
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// Minutes since the start of the year, from a month's name, a day and
/// `hh:mm[:ss]`.
fn minute_of_year(month: &str, day: &str, time: &str) -> Option<i64> {
    let m = MONTHS.iter().position(|x| month.to_ascii_lowercase().starts_with(x))? as i64;
    let d: i64 = day.trim().parse().ok()?;
    let mut hm = time.trim().trim_start_matches('*').split(':');
    let h: i64 = hm.next()?.parse().ok()?;
    let mi: i64 = hm.next()?.parse().ok()?;
    const DAYS: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    Some(((DAYS[m as usize] + d - 1) * 24 + h) * 60 + mi)
}

fn evidence_lines(text: &str, needle: &str, redact: Redact<'_>) -> Vec<String> {
    if needle.trim().is_empty() {
        return Vec::new();
    }
    text.lines().filter(|l| l.contains(needle)).take(3).map(|l| redact(l.trim_end())).collect()
}

fn row_fields(row: &Row) -> BTreeMap<String, String> {
    row.keys().map(|k| (k.clone(), parse::text(row, k))).collect()
}

fn finding(pb: &Playbook, check: &CheckSpec, device: &str, fields: BTreeMap<String, String>, evidence: Vec<String>, redact: Redact<'_>) -> Finding {
    let fields: BTreeMap<String, String> = fields.into_iter().map(|(k, v)| (k, redact(&v))).collect();
    let mut ev: Vec<String> = check.evidence.iter().filter_map(|f| fields.get(f).filter(|v| !v.is_empty()).map(|v| format!("{f}: {v}"))).collect();
    ev.extend(evidence);
    let _ = pb;
    Finding {
        id: finding_id(&check.id, device, &check.key, &fields),
        check: check.id.clone(),
        title: check.title.clone(),
        layer: check.layer,
        severity: check.severity,
        device: device.to_string(),
        says: fill(&check.says, device, &fields),
        fields,
        evidence: ev,
        fix: check.fix.clone(),
    }
}

/// Reads one device's outputs with its playbook.
pub fn evaluate(pb: &Playbook, dev: &DeviceInput, templates: Option<&Templates>, depth: Depth, path: bool, redact: Redact<'_>) -> DeviceEval {
    let mut eval = DeviceEval::default();
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    let mut unread: BTreeMap<String, String> = BTreeMap::new();
    for spec in pb.commands_for(depth, path) {
        let Some(out) = dev.outputs.iter().find(|o| o.command_id == spec.id) else {
            unread.insert(spec.id.clone(), "it was not asked: the run stopped before it".into());
            continue;
        };
        let Some(text) = &out.text else {
            unread.insert(spec.id.clone(), out.error.clone().unwrap_or_else(|| "no answer".into()));
            continue;
        };
        if parse::refused(text) {
            unread.insert(spec.id.clone(), if spec.optional { format!("this device does not have `{}`", spec.command) } else { format!("the device refused `{}`", spec.command) });
            continue;
        }
        let rows = match parse::rows(&spec.parser, templates, &pb.platform, &out.command, text) {
            Ok(r) => r,
            Err(e) => {
                unread.insert(spec.id.clone(), format!("the output of `{}` could not be read: {e}", spec.command));
                continue;
            }
        };
        for map in &spec.tables {
            let table = eval.tables.entry(map.name.clone()).or_default();
            for row in &rows {
                let mut t = BTreeMap::new();
                for (to, from) in &map.fields {
                    // `a b` is two of the row's fields, joined by a space.
                    let mut v = from.split_whitespace().map(|f| parse::text(row, f)).collect::<Vec<_>>().join(" ");
                    if let Some(vals) = map.values.get(to) {
                        let squeezed = v.split_whitespace().collect::<Vec<_>>().join(" ");
                        if let Some(n) = vals.iter().find(|(k, _)| k.eq_ignore_ascii_case(&squeezed)).map(|(_, n)| n.clone()) {
                            v = n;
                        }
                    }
                    t.insert(to.clone(), redact(&v));
                }
                table.push(t);
            }
        }
        texts.insert(spec.id.clone(), text.replace("\r\n", "\n"));
        eval.rows.insert(spec.id.clone(), rows);
    }
    // The device's clock, for "in the last 24 hours".
    let now = eval.rows.get("clock").and_then(|r| r.first()).and_then(|r| minute_of_year(&parse::text(r, "month"), &parse::text(r, "day"), &parse::text(r, "time")));
    for check in &pb.checks {
        let Some(spec) = pb.command(&check.command) else { continue };
        if spec.depth > depth || (spec.path && !path) {
            continue;
        }
        if let Some(why) = unread.get(&check.command) {
            eval.not_checked.push(NotChecked { device: dev.name.clone(), check: check.id.clone(), why: why.clone() });
            continue;
        }
        let (Some(rows), Some(text)) = (eval.rows.get(&check.command), texts.get(&check.command)) else { continue };
        let before = eval.findings.len();
        match &check.rule {
            Rule::Each { when } => {
                for row in rows.iter().filter(|r| all_hold(when, r)) {
                    let fields = row_fields(row);
                    let needle = check.key.first().and_then(|k| fields.get(k)).cloned().unwrap_or_default();
                    let ev = evidence_lines(text, &needle, redact);
                    eval.findings.push(finding(pb, check, &dev.name, fields, ev, redact));
                }
            }
            Rule::None { when } => {
                if !rows.iter().any(|r| all_hold(when, r)) {
                    let ev = if rows.is_empty() { text.lines().filter(|l| !l.trim().is_empty()).take(3).map(redact).collect() } else { Vec::new() };
                    eval.findings.push(finding(pb, check, &dev.name, BTreeMap::new(), ev, redact));
                }
            }
            Rule::Text { pattern, within_hours, when, min_count } => {
                let Ok(re) = Regex::new(pattern) else { continue };
                let mut hits = 0;
                // Finding id → how many matched, the first one's fields, up to three lines.
                type Counted = (u32, BTreeMap<String, String>, Vec<String>);
                let mut counted: BTreeMap<String, Counted> = BTreeMap::new();
                for line in text.lines() {
                    let Some(c) = re.captures(line) else { continue };
                    let mut row = Row::new();
                    for name in re.capture_names().flatten() {
                        if let Some(m) = c.name(name) {
                            row.insert(name.to_string(), Value::String(m.as_str().trim().to_string()));
                        }
                    }
                    if !all_hold(when, &row) {
                        continue;
                    }
                    let fields = row_fields(&row);
                    if let (Some(hours), Some(now)) = (within_hours, now) {
                        let at = minute_of_year(fields.get("month").map(String::as_str).unwrap_or(""), fields.get("day").map(String::as_str).unwrap_or(""), fields.get("time").map(String::as_str).unwrap_or(""));
                        let Some(at) = at else { continue };
                        let mut age = now - at;
                        if age < 0 {
                            age += 365 * 24 * 60;
                        }
                        if age > i64::from(*hours) * 60 {
                            continue;
                        }
                    }
                    hits += 1;
                    if min_count.is_some() {
                        let key = finding_id(&check.id, &dev.name, &check.key, &fields);
                        let e = counted.entry(key).or_insert_with(|| (0, fields.clone(), Vec::new()));
                        e.0 += 1;
                        if e.2.len() < 3 {
                            e.2.push(redact(line.trim_end()));
                        }
                        continue;
                    }
                    if hits > 20 {
                        break;
                    }
                    eval.findings.push(finding(pb, check, &dev.name, fields, vec![redact(line.trim_end())], redact));
                }
                if let Some(min) = min_count {
                    for (_, (n, mut fields, ev)) in counted {
                        if n >= *min {
                            fields.insert("count".into(), n.to_string());
                            eval.findings.push(finding(pb, check, &dev.name, fields, ev, redact));
                        }
                    }
                }
                if within_hours.is_some() && now.is_none() && hits > 0 {
                    eval.not_checked.push(NotChecked { device: dev.name.clone(), check: format!("{} (time window)", check.id), why: "the device's clock could not be read, so every matching line in its buffer counted, not only the last day's".into() });
                }
            }
        }
        // Several rows saying the same thing are one finding.
        let mut seen = std::collections::BTreeSet::new();
        let mut i = before;
        while i < eval.findings.len() {
            if !seen.insert(eval.findings[i].id.clone()) {
                eval.findings.remove(i);
            } else {
                i += 1;
            }
        }
        if eval.findings.len() == before {
            eval.passed.push(check.id.clone());
        }
    }
    eval
}
