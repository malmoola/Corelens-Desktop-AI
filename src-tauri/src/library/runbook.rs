//! A runbook read from a document: its Parts (a heading such as
//! "Part A – Swap the edge", "Phase 2: Routing" or "Window 1"), each Part's
//! numbered steps ("A3.", "3.", "Step 3."), and in each step the device it
//! names, the command blocks set in a fixed-width font, what to expect and
//! the Part's rollback. Read only; the engineer's document is never changed.

use super::extract::Block;

/// What a block of commands is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CmdKind {
    /// Every line is a read-only command run_show_command can run.
    Check,
    /// It enters configuration mode: a proposal.
    Config,
    /// Neither: clear, ping, write, copy — run by the engineer, or saved
    /// through a save proposal.
    Exec,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Commands {
    pub kind: CmdKind,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Step {
    /// As the document numbers it: "A3", "3".
    pub id: String,
    pub title: String,
    /// The devices the step names, as written.
    pub devices: Vec<String>,
    pub commands: Vec<Commands>,
    /// Every other line under the step, tables as `a | b` rows.
    pub notes: Vec<String>,
}

impl Step {
    /// Manual: no commands — a controller, onsite hands, a phone call.
    pub fn kind(&self) -> &'static str {
        if self.commands.is_empty() {
            "manual"
        } else if self.commands.iter().any(|c| c.kind == CmdKind::Config) {
            "config"
        } else if self.commands.iter().all(|c| c.kind == CmdKind::Check) {
            "check"
        } else {
            "exec"
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Part {
    /// The Part's own name: "A", "2", or the heading itself for a section
    /// such as "Start" or "Closeout".
    pub id: String,
    /// The heading above it that groups Parts ("Window 2"), when there is one.
    pub group: String,
    pub title: String,
    /// Lines before the first step.
    pub intro: Vec<String>,
    pub steps: Vec<Step>,
    pub rollback: Vec<String>,
}

impl Part {
    /// "Window 2 · Part A", or "Start".
    pub fn label(&self) -> String {
        let own = self.own();
        if self.group.is_empty() { own } else { format!("{} · {own}", self.group) }
    }

    /// "Part A" for a Part heading, else the heading itself.
    fn own(&self) -> String {
        part_heading(&self.title).filter(|(w, _, _)| !w.eq_ignore_ascii_case("step")).map(|(w, id, _)| format!("{w} {id}")).unwrap_or_else(|| self.title.clone())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Runbook {
    pub title: String,
    pub parts: Vec<Part>,
}

/// "4.2 Part A – …" → "Part A – …": a section number in front is not
/// part of the name.
fn without_section_number(text: &str) -> &str {
    let t = text.trim();
    match t.split_once(char::is_whitespace) {
        Some((first, rest)) if !first.is_empty() && first.trim_end_matches('.').split('.').all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit())) => rest.trim_start(),
        _ => t,
    }
}

/// "Part A – Swap the edge" → ("Part", "A", "Swap the edge"); a section
/// number in front ("4. Window 1 – Firewalls") is passed over.
pub fn part_heading(text: &str) -> Option<(String, String, String)> {
    let t = without_section_number(text);
    let mut words = t.splitn(3, char::is_whitespace);
    let word = words.next()?;
    let lower = word.to_ascii_lowercase();
    if !["part", "phase", "stage", "window", "section", "wave", "day", "step"].contains(&lower.as_str()) {
        return None;
    }
    let id_raw = words.next()?;
    let id: String = id_raw.trim_end_matches(['.', ':', ',', ')']).to_string();
    let ok_id = !id.is_empty() && id.len() <= 4 && id.chars().all(|c| c.is_ascii_alphanumeric()) && (id.chars().all(|c| c.is_ascii_digit()) || id.chars().all(|c| c.is_ascii_uppercase()) || (id.len() <= 2 && id.chars().next().is_some_and(|c| c.is_ascii_uppercase())));
    if !ok_id {
        return None;
    }
    let rest = words.next().unwrap_or("").trim().trim_start_matches(['–', '—', '-', ':', '.']).trim().to_string();
    Some((word.to_string(), id, rest))
}

/// "A3. EDGE-1 – shut it:" → ("A3", "EDGE-1 – shut it:"); also "3.",
/// "Step 3." and "Step 6 (after the rest). …". Never an address:
/// "10.0.0.0/8 …" has no space after its dot.
pub fn step_line(text: &str) -> Option<(String, String)> {
    let t = text.trim_start();
    let worded = t.strip_prefix("Step ").or_else(|| t.strip_prefix("step "));
    let t = worded.unwrap_or(t);
    let end = if worded.is_some() { t.find(['.', ')', ':', ' ', '(']) } else { t.find(['.', ')']) }?;
    let (id, after) = t.split_at(end);
    let after = if after.starts_with([' ', '(']) { after } else { &after[1..] };
    if !after.starts_with([' ', '(']) || after.trim().is_empty() {
        return None;
    }
    let letters = id.chars().take_while(char::is_ascii_uppercase).count();
    let digits = &id[letters..];
    let digits_ok = !digits.is_empty() && digits.len() <= 3 && digits.chars().all(|c| c.is_ascii_digit());
    (letters <= 2 && digits_ok).then(|| (id.to_string(), after.trim().to_string()))
}

/// Words that start a command or an instruction, never a device's name.
const NOT_A_DEVICE: &[&str] = &["show", "ping", "clear", "write", "copy", "configure", "conf", "no", "interface", "debug", "traceroute", "get", "diagnose", "run", "verify", "check", "save", "rollback", "expect", "onsite", "notes", "note", "start", "all", "both", "every", "each", "the"];

/// The devices a step's title names: what comes before its dash or its
/// colon, split on "then", "and" and commas; a note in brackets is passed
/// over. A device's name has a capital past its first letter, a digit, a
/// hyphen or an underscore, so "Verify" and "right away" name none.
pub fn devices_of(title: &str) -> Vec<String> {
    // Brackets out first: "EDGE-1 (the second one: EDGE-2):".
    let mut t = String::new();
    let mut depth = 0;
    for c in title.trim().chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => t.push(c),
            _ => {}
        }
    }
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    // "Isolate the old firewall. EDGE-1:" — the last sentence.
    let t = t.rsplit_once(". ").map(|(_, b)| b.to_string()).unwrap_or(t);
    let dash = t.find(" – ").or_else(|| t.find(" — ")).or_else(|| t.find(" - "));
    let colon = t.find(':');
    let head = match (dash, colon) {
        (Some(d), Some(c)) => &t[..d.min(c)],
        (Some(d), None) => &t[..d],
        (None, Some(c)) => &t[..c],
        (None, None) => return Vec::new(),
    };
    let mut out = Vec::new();
    for piece in head.split(',').flat_map(|p| p.split(" and ")).flat_map(|p| p.split(" & ")).flat_map(|p| p.split(" / ")) {
        let p = piece.trim();
        let p = p.strip_prefix("then ").unwrap_or(p).trim();
        let p = p.strip_suffix(" right after").or_else(|| p.strip_suffix(" right away")).unwrap_or(p).trim();
        let p = p.strip_suffix(" CLI").or_else(|| p.strip_suffix(" cli")).or_else(|| p.strip_suffix(" shell")).unwrap_or(p).trim();
        let first = p.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        let name_like = |w: &str| w.chars().skip(1).any(|c| c.is_ascii_uppercase()) || w.chars().any(|c| c.is_ascii_digit() || c == '-' || c == '_') || (w.len() >= 2 && w.chars().all(|c| c.is_ascii_uppercase()));
        // "EDGE 1" is a name; "DR hands" and "the UPD device" are not.
        let named = p.split_whitespace().all(name_like);
        if p.is_empty() || p.len() > 40 || p.split_whitespace().count() > 3 || NOT_A_DEVICE.contains(&first.as_str()) || !named {
            continue;
        }
        out.push(p.to_string());
    }
    out.truncate(8);
    out
}

fn classify(lines: &[String]) -> CmdKind {
    let first = lines.iter().map(|l| l.trim().to_ascii_lowercase()).find(|l| !l.is_empty()).unwrap_or_default();
    let config_entry = ["configure terminal", "conf t", "config t", "configure", "configure private", "configure exclusive", "config system", "config firewall", "config router", "config vpn", "config user", "edit"].iter().any(|p| first == *p || first.starts_with(&format!("{p} ")));
    if config_entry {
        return CmdKind::Config;
    }
    if lines.iter().all(|l| crate::mcp::show::precheck(l).is_ok()) {
        CmdKind::Check
    } else {
        CmdKind::Exec
    }
}

fn table_lines(rows: &[Vec<String>]) -> Vec<String> {
    rows.iter().map(|r| r.join(" | ")).collect()
}

/// A Part's own blocks, before its steps are read.
struct RawPart {
    id: String,
    group: String,
    title: String,
    blocks: Vec<Block>,
    /// Which heading above it this is under, counted.
    parent: usize,
}

/// The steps of one Part. Numbered lines are steps; a Part with none,
/// written as a device line and its commands ("CORE-1 and CORE-2:" then
/// the block) pair after pair, has a step for each pair, numbered from 1.
fn steps_of(raw: RawPart) -> Part {
    let numbered = raw.blocks.iter().any(|b| matches!(b, Block::Para { text, mono: false } if step_line(text).is_some()));
    let mut part = Part { id: raw.id, group: raw.group, title: raw.title, intro: Vec::new(), steps: Vec::new(), rollback: Vec::new() };
    let mut buf: Vec<String> = Vec::new();
    let flush = |part: &mut Part, buf: &mut Vec<String>| {
        if buf.is_empty() {
            return;
        }
        let lines = std::mem::take(buf);
        let cmds = Commands { kind: classify(&lines), lines };
        match part.steps.last_mut() {
            Some(s) => s.commands.push(cmds),
            None => part.intro.extend(cmds.lines),
        }
    };
    let note = |part: &mut Part, line: String| match part.steps.last_mut() {
        Some(s) => s.notes.push(line),
        None => part.intro.push(line),
    };
    let mut implicit = 0;
    for (i, b) in raw.blocks.iter().enumerate() {
        match b {
            Block::Para { text, mono: true } => buf.push(text.clone()),
            Block::Para { text, mono: false } => {
                flush(&mut part, &mut buf);
                let next_is_commands = matches!(raw.blocks.get(i + 1), Some(Block::Para { mono: true, .. }));
                if text.to_ascii_lowercase().starts_with("rollback") {
                    part.rollback.push(text.clone());
                } else if let Some((id, title)) = step_line(text).filter(|_| numbered) {
                    let devices = devices_of(&title);
                    part.steps.push(Step { id, title, devices, commands: Vec::new(), notes: Vec::new() });
                } else if !numbered && next_is_commands {
                    implicit += 1;
                    let devices = devices_of(text);
                    part.steps.push(Step { id: implicit.to_string(), title: text.clone(), devices, commands: Vec::new(), notes: Vec::new() });
                } else {
                    note(&mut part, text.clone());
                }
            }
            Block::Table { rows } => {
                flush(&mut part, &mut buf);
                for l in table_lines(rows) {
                    note(&mut part, l);
                }
            }
            Block::Heading { text, .. } => {
                flush(&mut part, &mut buf);
                note(&mut part, text.clone());
            }
        }
    }
    flush(&mut part, &mut buf);
    part
}

/// The runbook in these blocks, when they hold one: at least one Part
/// heading with steps under it.
pub fn read(blocks: &[Block], fallback_title: &str) -> Option<Runbook> {
    let is_part = |text: &str| part_heading(text).is_some_and(|(w, _, _)| !w.eq_ignore_ascii_case("step"));
    // Parts are the deepest heading level that has Part-like headings; a
    // shallower Part-like heading (Window 1) groups them.
    let deepest = blocks.iter().filter_map(|b| match b {
        Block::Heading { level, text } if is_part(text) => Some(*level),
        _ => None,
    }).max()?;
    let title = blocks
        .iter()
        .find_map(|b| match b {
            Block::Heading { level: 1, text } if part_heading(text).is_none() => Some(text.clone()),
            Block::Para { text, mono: false } if text.len() < 120 => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| fallback_title.to_string());

    let mut raws: Vec<RawPart> = Vec::new();
    let mut group = String::new();
    let mut in_part = false;
    let mut parent = 0usize;
    for b in blocks {
        if let Block::Heading { level, text } = b {
            if *level < deepest {
                // Above the Parts: a group when Part-like ("Window 2 –
                // Routing"), otherwise the end of one.
                group = part_heading(text).map(|(w, id, _)| format!("{w} {id}")).unwrap_or_default();
                in_part = false;
                parent += 1;
                continue;
            }
            if *level == deepest {
                if text.to_ascii_lowercase().starts_with("rollback") && in_part {
                    // A Rollback heading belongs to the Part before it.
                    if let Some(r) = raws.last_mut() {
                        r.blocks.push(Block::Para { text: text.clone(), mono: false });
                    }
                    continue;
                }
                let clean = without_section_number(text).to_string();
                let id = part_heading(text).filter(|(w, _, _)| !w.eq_ignore_ascii_case("step")).map(|(_, id, _)| id).unwrap_or_else(|| clean.clone());
                raws.push(RawPart { id, group: group.clone(), title: clean, blocks: Vec::new(), parent });
                in_part = true;
                continue;
            }
            // Deeper: a step set as a heading, or a heading within a step.
            if in_part {
                if let Some(r) = raws.last_mut() {
                    r.blocks.push(Block::Para { text: text.clone(), mono: false });
                }
            }
            continue;
        }
        if in_part {
            if let Some(r) = raws.last_mut() {
                r.blocks.push(b.clone());
            }
        }
    }
    // A section not named like a Part ("Start", "Closeout") is one only
    // beside real Parts; under a heading with none ("Before Window 2"),
    // it is prework or design, not a Part.
    let with_parts: std::collections::HashSet<usize> = raws.iter().filter(|r| is_part(&r.title)).map(|r| r.parent).collect();
    raws.retain(|r| with_parts.contains(&r.parent));
    let mut parts: Vec<Part> = raws.into_iter().map(steps_of).collect();
    parts.retain(|p| !p.steps.is_empty());
    let named: Vec<&Part> = parts.iter().filter(|p| is_part(&p.title)).collect();
    if named.is_empty() || !named.iter().any(|p| p.steps.len() >= 2) {
        return None;
    }
    // A design that says "Window 1" over and over in its checklists is not
    // a runbook: a runbook's Parts have names of their own.
    let mut labels: Vec<String> = parts.iter().map(Part::label).collect();
    let n = labels.len();
    labels.sort();
    labels.dedup();
    if n >= 4 && labels.len() * 10 < n * 7 {
        return None;
    }
    Some(Runbook { title, parts })
}

impl Runbook {
    /// The Part a person names: "A", "Part A", "window 2 part a", "2A", or
    /// words of its title. Ambiguous or unknown: the Parts there are.
    pub fn find(&self, asked: &str) -> Result<&Part, String> {
        let norm = |s: &str| s.to_ascii_lowercase().replace(['·', '–', '—', '-', ':', ','], " ").split_whitespace().collect::<Vec<_>>().join(" ");
        let a = norm(asked);
        let a_core = a.trim_start_matches("start ").trim_start_matches("run ").trim().to_string();
        let names = || self.parts.iter().map(Part::label).collect::<Vec<_>>().join(", ");
        let exact: Vec<&Part> = self
            .parts
            .iter()
            .filter(|p| {
                let id = p.id.to_ascii_lowercase();
                let label = norm(&p.label());
                let own = norm(&p.own());
                [id.clone(), label.clone(), own.clone(), norm(&p.title), format!("{} {own}", norm(&p.group)), format!("{} {id}", norm(&p.group))].iter().any(|n| n.trim() == a_core)
            })
            .collect();
        match exact.len() {
            1 => return Ok(exact[0]),
            n if n > 1 => return Err(format!("More than one Part is called that: {}. Say which, with its window or phase.", exact.iter().map(|p| p.label()).collect::<Vec<_>>().join(", "))),
            _ => {}
        }
        // "Part Z" names a Part by its letter: no Part has it, so words of
        // the titles are not tried.
        if part_heading(asked).is_some() {
            return Err(format!("No Part of {} is called {asked}. Its Parts: {}.", self.title, names()));
        }
        let words: Vec<&str> = a_core.split_whitespace().filter(|w| w.len() > 2 && !["part", "phase", "stage", "window", "section", "wave", "step"].contains(w)).collect();
        let by_words: Vec<&Part> = self.parts.iter().filter(|p| !words.is_empty() && words.iter().all(|w| norm(&format!("{} {}", p.group, p.title)).contains(w))).collect();
        match by_words.len() {
            1 => Ok(by_words[0]),
            0 => Err(format!("No Part of {} is called {asked}. Its Parts: {}.", self.title, names())),
            _ => Err(format!("{asked} could be {}. Say which.", by_words.iter().map(|p| p.label()).collect::<Vec<_>>().join(", "))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(level: u8, t: &str) -> Block {
        Block::Heading { level, text: t.into() }
    }
    fn p(t: &str) -> Block {
        Block::Para { text: t.into(), mono: false }
    }
    fn c(t: &str) -> Block {
        Block::Para { text: t.into(), mono: true }
    }

    /// A made-up runbook in the shape engineers write them: a title line,
    /// a Start section, two Parts with lettered steps, commands in a
    /// fixed-width font, an Expect line, a table, a rollback line.
    fn sample() -> Vec<Block> {
        vec![
            p("Example Co – Window 1 Runbook – Edge swap"),
            h(1, "Start"),
            p("1. Open the bridge and confirm the change freeze."),
            p("2. EDGE-1:"),
            c("show interfaces status"),
            h(1, "Part A – Old edge out, new edge in"),
            p("A1. Controller: push the new policy."),
            p("A2. NEW-FW CLI:"),
            c("show failover"),
            p("Expect: both units up."),
            p("A3. EDGE-1, then EDGE-2 – shut the old uplinks:"),
            c("configure terminal"),
            c("interface Port-channel10"),
            c(" shutdown"),
            c("end"),
            p("A4. Onsite: move the cable from OLD-FW Eth1/1 to NEW-FW Eth1/2."),
            p("A5. EDGE-1:"),
            c("clear ip arp 192.0.2.10"),
            c("ping 192.0.2.10"),
            p("A6. Verify:"),
            Block::Table { rows: vec![vec!["Check".into(), "Expected".into()], vec!["ping 192.0.2.10".into(), "replies".into()]] },
            p("Rollback A: EDGE-1 no shut Po10, then EDGE-2."),
            h(1, "Part B – Routing"),
            p("B1. CORE-1:"),
            c("show ip route 10.0.0.0/8"),
        ]
    }

    #[test]
    fn parts_steps_devices_and_commands_are_read() {
        let rb = read(&sample(), "file").unwrap();
        assert_eq!(rb.title, "Example Co – Window 1 Runbook – Edge swap");
        assert_eq!(rb.parts.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["Start", "A", "B"]);
        let a = &rb.parts[1];
        assert_eq!(a.label(), "Part A");
        assert_eq!(a.steps.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["A1", "A2", "A3", "A4", "A5", "A6"]);
        assert_eq!(a.steps.iter().map(Step::kind).collect::<Vec<_>>(), ["manual", "check", "config", "manual", "exec", "manual"]);
        assert_eq!(a.steps[1].devices, ["NEW-FW"]);
        assert_eq!(a.steps[2].devices, ["EDGE-1", "EDGE-2"]);
        assert!(a.steps[5].devices.is_empty(), "Verify is not a device");
        assert_eq!(a.steps[2].commands[0].lines, ["configure terminal", "interface Port-channel10", " shutdown", "end"]);
        assert_eq!(a.steps[1].notes, ["Expect: both units up."]);
        assert_eq!(a.steps[5].notes, ["Check | Expected", "ping 192.0.2.10 | replies"]);
        assert_eq!(a.rollback, ["Rollback A: EDGE-1 no shut Po10, then EDGE-2."]);
        assert_eq!(rb.parts[0].steps[1].kind(), "check");
    }

    #[test]
    fn parts_under_windows_are_named_with_their_window() {
        let blocks = vec![
            h(1, "3. Before Window 1 (no impact)"),
            h(2, "3.1 Controller build"),
            p("1. Build the policy."),
            p("2. Do not deploy."),
            h(1, "Window 1 – Firewalls"),
            h(2, "Part A – Swap"),
            p("Step 1. Record state."),
            h(1, "Window 2 – Routing"),
            h(2, "Part A – Routing"),
            p("Step 1. CORE-1:"),
            c("show ip bgp summary"),
            p("Step 2. CORE-2:"),
            c("show ip bgp summary"),
            h(2, "Part B – A subnet"),
            h(3, "Step 1. CORE-2:"),
            c("show ip route"),
        ];
        let rb = read(&blocks, "MOP.docx").unwrap();
        assert_eq!(rb.parts.iter().map(Part::label).collect::<Vec<_>>(), ["Window 1 · Part A", "Window 2 · Part A", "Window 2 · Part B"], "the prework under a heading with no Parts is not one");
        assert_eq!(rb.parts[2].steps[0].devices, ["CORE-2"], "a step set as a heading");
        assert!(rb.find("Part A").unwrap_err().contains("More than one"));
        assert_eq!(rb.find("window 2 part a").unwrap().title, "Part A – Routing");
        assert_eq!(rb.find("Window 2 · Part B").unwrap().id, "B");
        assert_eq!(rb.find("subnet").unwrap().id, "B");
        assert!(rb.find("Part Z").unwrap_err().contains("Its Parts"));
    }

    #[test]
    fn a_part_of_device_lines_and_commands_has_a_step_for_each_and_a_numbered_window_groups() {
        let blocks = vec![
            h(1, "4. Window 1 – Firewalls"),
            h(2, "Part C – Routing to the fabric"),
            p("Notes before the steps."),
            p("CORE-1 (a note in brackets):"),
            c("configure terminal"),
            c("router bgp 65000"),
            c("end"),
            p("CORE-2, right after, with its own router id:"),
            c("configure terminal"),
            c("end"),
            p("Rollback: undo both."),
        ];
        let rb = read(&blocks, "x").unwrap();
        let part = &rb.parts[0];
        assert_eq!(part.label(), "Window 1 · Part C");
        assert_eq!(part.steps.iter().map(|s| (s.id.as_str(), s.devices.clone())).collect::<Vec<_>>(), [("1", vec!["CORE-1".to_string()]), ("2", vec!["CORE-2".to_string()])]);
        assert_eq!(part.intro, ["Notes before the steps."]);
        assert_eq!(part.rollback.len(), 1);
    }

    #[test]
    fn a_document_without_parts_or_steps_is_not_a_runbook() {
        assert!(read(&[h(1, "Design"), p("The core is a pair.")], "x").is_none());
        // A design whose checklists repeat the windows' names.
        let mut design = Vec::new();
        for _ in 0..4 {
            design.extend([h(2, "Window 1"), p("1. CORE-1:"), c("configure terminal"), p("2. CORE-2:"), c("configure terminal"), h(2, "Window 2"), p("1. EDGE-1:"), c("show version")]);
        }
        assert!(read(&design, "x").is_none());
        assert!(read(&[h(1, "Part A – Notes"), p("Nothing numbered here.")], "x").is_none());
    }

    #[test]
    fn step_lines_are_not_addresses_or_sentences() {
        assert_eq!(step_line("A12. Save: EDGE-1 write memory"), Some(("A12".into(), "Save: EDGE-1 write memory".into())));
        assert_eq!(step_line("Step 3. Isolate the old firewall. EDGE-1:"), Some(("3".into(), "Isolate the old firewall. EDGE-1:".into())));
        assert_eq!(step_line("10.0.0.0/8 goes to the core"), None);
        assert_eq!(step_line("Expect: up."), None);
        assert_eq!(devices_of("Isolate the old firewall. EDGE-1:"), ["EDGE-1"]);
        assert_eq!(devices_of("SPINE-1 and SPINE-2 – bring the links up:"), ["SPINE-1", "SPINE-2"]);
        assert!(devices_of("Open the bridge and confirm.").is_empty());
        assert_eq!(devices_of("CORE-1 (a note: in brackets):"), ["CORE-1"]);
        assert_eq!(devices_of("EDGE-2, right after, with its own router id: below"), ["EDGE-2"]);
        assert!(devices_of("Site hands – move the cable:").is_empty());
        assert_eq!(devices_of("EDGE 1 and CORE:"), ["EDGE 1", "CORE"]);
        assert!(devices_of("Every switch on the floor, right away:").is_empty());
        assert!(devices_of("Cores: show version – note it.").is_empty());
        assert_eq!(step_line("Step 6 (after the others). Controller: save."), Some(("6".into(), "(after the others). Controller: save.".into())));
    }
}
