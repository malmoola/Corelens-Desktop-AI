//! The playbooks: what to ask each platform, how to read the answer, and
//! what in it is a problem. Data, under `resources/diagnose/`, one file per
//! platform, versioned; this module is their shape and the checks that a
//! file means what it says before anything runs it.

use std::collections::BTreeMap;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// The playbook format this build reads.
pub const FORMAT: u32 = 1;

/// How thorough a run is. A quick run asks the commands marked quick; a
/// full one asks them all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Depth {
    Quick,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl Severity {
    pub fn word(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Critical => "critical",
        }
    }
}

/// The part of the network a check is about, as the report groups them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Health,
    L1,
    L2,
    L3,
    Services,
    Firewall,
    Drift,
    Path,
}

impl Layer {
    pub fn name(self) -> &'static str {
        match self {
            Layer::Health => "Management and health",
            Layer::L1 => "Layer 1",
            Layer::L2 => "Layer 2",
            Layer::L3 => "Layer 3",
            Layer::Services => "Services",
            Layer::Firewall => "Firewall",
            Layer::Drift => "Configuration drift",
            Layer::Path => "Path",
        }
    }
}

/// Where a playbook's knowledge comes from: met on a device, or read in
/// the vendor's documentation and not yet met.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verified {
    Lab,
    Docs,
}

/// How a command's output becomes rows: `ntc`, `text`, `{ lines: … }` or
/// `{ blocks: … }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Parser {
    Named(Named),
    Lines { lines: LinesSpec },
    Blocks { blocks: BlocksSpec },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Named {
    /// The ntc template the index names for this platform and command.
    Ntc,
    /// No rows: the checks on it read the text.
    Text,
}

/// One row per line matching `pattern`; its named groups are the fields.
/// Lines before one matching `after`, when given, are skipped.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinesSpec {
    pub pattern: String,
    #[serde(default)]
    pub after: Option<String>,
}

/// One row per block, a block starting at each line matching `start`
/// (whose named groups are fields); each of `fields` is a pattern whose
/// first group, found anywhere in the block, is that field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlocksSpec {
    pub start: String,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}

/// Rows of one command, renamed into one of the tables the analyses
/// across devices read (`interfaces`, `trunks`, `neighbors`, …).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableMap {
    pub name: String,
    /// The table's field ← the row's field.
    pub fields: BTreeMap<String, String>,
    /// Per table field, values spelt the platform's way → the table's.
    #[serde(default)]
    pub values: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    pub id: String,
    pub command: String,
    pub parser: Parser,
    pub depth: Depth,
    /// Feeds tables the analyses read.
    #[serde(default)]
    pub tables: Vec<TableMap>,
    /// Some models or licences lack it: a refusal means "not checked",
    /// not a failure of the run.
    #[serde(default)]
    pub optional: bool,
    /// Asked only in path mode, with `{src}`, `{dst}`, `{port}` and
    /// `{proto}` filled from the path (each checked to be an address or a
    /// number first).
    #[serde(default)]
    pub path: bool,
}

/// What path mode fills a path command with.
#[derive(Debug, Clone, Default)]
pub struct PathValues {
    pub src: String,
    pub dst: String,
    pub port: String,
    pub proto: String,
}

impl CommandSpec {
    /// The command as it is sent: a path command filled in, or `None` when
    /// a value is not one it can take.
    pub fn line(&self, path: Option<&PathValues>) -> Option<String> {
        if !self.path {
            return Some(self.command.clone());
        }
        let p = path?;
        let ip = |v: &str| v.parse::<std::net::Ipv4Addr>().is_ok();
        let num = |v: &str| !v.is_empty() && v.len() <= 5 && v.chars().all(|c| c.is_ascii_digit());
        let mut out = self.command.clone();
        for (name, v, ok) in [("src", &p.src, ip(&p.src)), ("dst", &p.dst, ip(&p.dst)), ("port", &p.port, num(&p.port)), ("proto", &p.proto, num(&p.proto))] {
            let token = format!("{{{name}}}");
            if out.contains(&token) {
                if !ok {
                    return None;
                }
                out = out.replace(&token, v);
            }
        }
        Some(out)
    }
}

/// How a condition compares a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Eq,
    Ne,
    In,
    NotIn,
    Matches,
    NotMatches,
    Gt,
    Ge,
    Lt,
    Le,
    Empty,
    NotEmpty,
    /// The field as a percentage of `of` is above `value`.
    PctGt,
    /// … below `value`.
    PctLt,
    /// The field, a duration as a device writes one ("2 days, 3 hours"),
    /// is shorter than `value` minutes.
    MinutesLt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub field: String,
    pub op: Op,
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    /// The other field of a `pct_gt`.
    #[serde(default)]
    pub of: Option<String>,
}

/// What makes a check fire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Rule {
    /// A finding for every row where all of `when` hold.
    Each { when: Vec<Condition> },
    /// One finding when no row has all of `when` (or there are no rows).
    None {
        #[serde(default)]
        when: Vec<Condition>,
    },
    /// A finding for every match of `pattern` in the output; its named
    /// groups are the fields. With `within_hours`, only lines whose
    /// `month`, `day` and `time` groups fall within that many hours of the
    /// device's own clock (the `clock` command) count.
    Text {
        pattern: String,
        #[serde(default)]
        within_hours: Option<u32>,
        /// Only matches whose groups meet these.
        #[serde(default)]
        when: Vec<Condition>,
        /// Fire only when the same key matched at least this many times;
        /// `{count}` says how many.
        #[serde(default)]
        min_count: Option<u32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckSpec {
    pub id: String,
    pub title: String,
    pub layer: Layer,
    /// The command (by id) whose output it reads.
    pub command: String,
    pub rule: Rule,
    pub severity: Severity,
    /// The finding as a sentence; `{field}` is the row's field, `{device}`
    /// the device.
    pub says: String,
    /// The fields that tell one finding of this check from another.
    #[serde(default)]
    pub key: Vec<String>,
    /// The fields shown as evidence.
    #[serde(default)]
    pub evidence: Vec<String>,
    /// The fix (by id, this playbook's) a root cause of this check uses.
    #[serde(default)]
    pub fix: Option<String>,
    /// What a pass means, said in the report's "what passed".
    #[serde(default)]
    pub passed: Option<String>,
}

/// What a fix template fills a placeholder with must look like; anything
/// else and no command is written (a value comes from the network and is
/// never trusted to be what it claims).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placeholder {
    Interface,
    Vlan,
    Ipv4,
    Priority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FixKind {
    /// Configuration lines, proposed through `propose_config_change`.
    Change,
    /// Saving the configuration, its own proposal.
    Save,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixSpec {
    pub id: String,
    pub title: String,
    #[serde(default = "change_kind")]
    pub kind: FixKind,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub verify: Vec<String>,
    pub impact: String,
    #[serde(default)]
    pub params: BTreeMap<String, Placeholder>,
}

fn change_kind() -> FixKind {
    FixKind::Change
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Playbook {
    pub format: u32,
    /// This playbook's own version, raised with each change to it.
    pub version: u32,
    /// The ntc platform: `cisco_ios`, `cisco_nxos`, …
    pub platform: String,
    pub name: String,
    pub verified: Verified,
    /// Where the commands and the rules come from.
    pub source: String,
    pub commands: Vec<CommandSpec>,
    pub checks: Vec<CheckSpec>,
    #[serde(default)]
    pub fixes: Vec<FixSpec>,
    /// How this platform's configurations are compared.
    #[serde(default)]
    pub drift: Option<DriftSpec>,
}

/// The running configuration against the saved one and against the last
/// backup. Lines matching `ignore` change by themselves and are not drift.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriftSpec {
    /// The command id whose output is the running configuration.
    pub running: String,
    /// The one whose output is the startup configuration, where the
    /// platform keeps one apart.
    #[serde(default)]
    pub startup: Option<String>,
    #[serde(default)]
    pub ignore: Vec<String>,
    /// The fix that saves (for unsaved changes).
    #[serde(default)]
    pub save_fix: Option<String>,
}

impl Playbook {
    pub fn command(&self, id: &str) -> Option<&CommandSpec> {
        self.commands.iter().find(|c| c.id == id)
    }
    pub fn fix(&self, id: &str) -> Option<&FixSpec> {
        self.fixes.iter().find(|f| f.id == id)
    }
    /// The commands a run of this depth asks, in order; path commands
    /// only when it is a path.
    pub fn commands_for(&self, depth: Depth, path: bool) -> impl Iterator<Item = &CommandSpec> {
        self.commands.iter().filter(move |c| c.depth <= depth && (path || !c.path))
    }

    /// Everything a file can get wrong that would make a run lie: an
    /// unknown command, an id twice, a pattern that does not compile, a
    /// placeholder the template never declared, a command that is not a
    /// read.
    pub fn validate(&self) -> Result<(), String> {
        let at = |what: &str| format!("{}: {what}", self.platform);
        if self.format != FORMAT {
            return Err(at(&format!("format {} is not the {FORMAT} this build reads", self.format)));
        }
        let mut seen = std::collections::BTreeSet::new();
        for c in &self.commands {
            if !seen.insert(&c.id) {
                return Err(at(&format!("command id {} twice", c.id)));
            }
            let sample = PathValues { src: "192.0.2.1".into(), dst: "198.51.100.1".into(), port: "443".into(), proto: "6".into() };
            let line = c.line(Some(&sample)).unwrap_or_default();
            if !corelens_catalog::allowlist::verdict(&line).is_ok() {
                return Err(at(&format!("`{}` is not a read-only command", c.command)));
            }
            match &c.parser {
                Parser::Lines { lines: LinesSpec { pattern, after } } => {
                    Regex::new(pattern).map_err(|e| at(&format!("{}: {e}", c.id)))?;
                    if let Some(a) = after {
                        Regex::new(a).map_err(|e| at(&format!("{}: {e}", c.id)))?;
                    }
                }
                Parser::Blocks { blocks: BlocksSpec { start, fields } } => {
                    Regex::new(start).map_err(|e| at(&format!("{}: {e}", c.id)))?;
                    for p in fields.values() {
                        Regex::new(p).map_err(|e| at(&format!("{}: {e}", c.id)))?;
                    }
                }
                Parser::Named(_) => {}
            }
        }
        let mut checks = std::collections::BTreeSet::new();
        for k in &self.checks {
            if !checks.insert(&k.id) {
                return Err(at(&format!("check id {} twice", k.id)));
            }
            if self.command(&k.command).is_none() {
                return Err(at(&format!("check {} reads command {}, which is not defined", k.id, k.command)));
            }
            if let Rule::Text { pattern, .. } = &k.rule {
                Regex::new(pattern).map_err(|e| at(&format!("{}: {e}", k.id)))?;
            }
            if let Some(f) = &k.fix {
                if self.fix(f).is_none() {
                    return Err(at(&format!("check {} names fix {f}, which is not defined", k.id)));
                }
            }
            for cond in rule_conditions(&k.rule) {
                if matches!(cond.op, Op::Matches | Op::NotMatches) {
                    let p = cond.value.as_ref().and_then(|v| v.as_str()).ok_or_else(|| at(&format!("{}: {:?} needs a pattern", k.id, cond.op)))?;
                    Regex::new(p).map_err(|e| at(&format!("{}: {e}", k.id)))?;
                }
                if matches!(cond.op, Op::PctGt | Op::PctLt) && cond.of.is_none() {
                    return Err(at(&format!("{}: pct_gt needs `of`", k.id)));
                }
            }
        }
        if let Some(d) = &self.drift {
            for id in std::iter::once(&d.running).chain(d.startup.iter()) {
                if self.command(id).is_none() {
                    return Err(at(&format!("drift reads command {id}, which is not defined")));
                }
            }
            for p in &d.ignore {
                Regex::new(p).map_err(|e| at(&format!("drift: {e}")))?;
            }
            if let Some(f) = &d.save_fix {
                if self.fix(f).is_none() {
                    return Err(at(&format!("drift names fix {f}, which is not defined")));
                }
            }
        }
        for f in &self.fixes {
            for line in f.commands.iter().chain(&f.verify) {
                for name in placeholders(line) {
                    if !f.params.contains_key(&name) {
                        return Err(at(&format!("fix {} uses {{{name}}} without declaring it", f.id)));
                    }
                }
            }
            for v in &f.verify {
                if !corelens_catalog::allowlist::verdict(&v.replace(['{', '}'], "")).is_ok() {
                    return Err(at(&format!("fix {}: verify `{v}` is not read-only", f.id)));
                }
            }
            if f.kind == FixKind::Change && f.commands.is_empty() {
                return Err(at(&format!("fix {} has no commands", f.id)));
            }
        }
        Ok(())
    }
}

pub fn rule_conditions(rule: &Rule) -> &[Condition] {
    match rule {
        Rule::Each { when } | Rule::None { when } | Rule::Text { when, .. } => when,
    }
}

/// The `{names}` a line uses.
pub fn placeholders(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}') else { break };
        out.push(rest[start + 1..start + end].to_string());
        rest = &rest[start + end + 1..];
    }
    out
}
