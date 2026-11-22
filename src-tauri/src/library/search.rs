//! A document's sections, and the search over them: each section is the
//! text under a heading, named by the headings above it, cut where it is
//! long; the search is BM25 over words, with a section's headings and the
//! document's name counted as well, so "rollback part b" finds Part B's
//! rollback. Addresses, interface names and commands are whole words too:
//! "192.0.2.10", "eth1/2", "port-channel20".

use std::collections::HashMap;

use super::extract::Block;

/// The longest a section is before it is cut; a cut keeps its headings.
pub const SECTION_CHARS: usize = 1800;

#[derive(Debug, Clone)]
pub struct Section {
    /// "Window 1 Runbook > Part A – …"
    pub path: String,
    pub text: String,
    /// Which piece of its heading's text this is, from 1, and of how many.
    pub piece: usize,
    pub pieces: usize,
}

/// A block as text: commands indented, a table as rows.
pub fn render(b: &Block) -> Option<String> {
    match b {
        Block::Heading { text, level } => Some(format!("{} {text}", "#".repeat(*level as usize))),
        Block::Para { text, mono: true } => Some(format!("    {text}")),
        Block::Para { text, mono: false } => Some(text.clone()),
        Block::Table { rows } => Some(rows.iter().map(|r| format!("| {} |", r.join(" | "))).collect::<Vec<_>>().join("\n")),
    }
}

pub fn sections(blocks: &[Block]) -> Vec<Section> {
    let mut out = Vec::new();
    let mut trail: Vec<(u8, String)> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let flush = |trail: &[(u8, String)], lines: &mut Vec<String>, out: &mut Vec<Section>| {
        if lines.iter().all(|l| l.trim().is_empty()) {
            lines.clear();
            return;
        }
        let path = trail.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(" > ");
        let mut pieces: Vec<String> = Vec::new();
        let mut cur = String::new();
        for l in lines.drain(..) {
            if !cur.is_empty() && cur.len() + l.len() > SECTION_CHARS {
                pieces.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push('\n');
            }
            cur.push_str(&l);
        }
        if !cur.is_empty() {
            pieces.push(cur);
        }
        let n = pieces.len();
        out.extend(pieces.into_iter().enumerate().map(|(i, text)| Section { path: path.clone(), text, piece: i + 1, pieces: n }));
    };
    for b in blocks {
        if let Block::Heading { level, text } = b {
            flush(&trail, &mut lines, &mut out);
            while trail.last().is_some_and(|(l, _)| *l >= *level) {
                trail.pop();
            }
            trail.push((*level, text.clone()));
            continue;
        }
        if let Some(t) = render(b) {
            lines.push(t);
        }
    }
    flush(&trail, &mut lines, &mut out);
    out
}

const STOP: &[&str] = &["the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "is", "are", "be", "it", "this", "that", "with", "as", "at", "by", "from", "then", "how", "what", "do", "we", "our", "i", "you"];

/// The words of a text: runs of letters and digits, lower-cased, and also
/// each run that holds `.`, `/`, `-` or `:` between them whole.
pub fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    // "Part A", "Window 1", "Step 3" as one word as well, or the letter is
    // lost and Part A cannot be told from Part B.
    let plain: Vec<String> = text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
    for pair in plain.windows(2) {
        if ["part", "phase", "stage", "window", "step", "section", "wave"].contains(&pair[0].as_str()) && pair[1].len() <= 3 {
            out.push(format!("{}_{}", pair[0], pair[1]));
        }
    }
    for raw in text.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | '|' | '>' | '<' | '=' | '`' | '*')) {
        let w = raw.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if w.is_empty() {
            continue;
        }
        let parts: Vec<&str> = w.split(|c: char| !c.is_alphanumeric()).filter(|p| !p.is_empty()).collect();
        if parts.len() > 1 {
            out.push(w.clone());
        }
        for p in parts {
            if !STOP.contains(&p) {
                out.push(p.to_string());
            }
        }
    }
    out
}

/// What a search is over: every section of every document, with the
/// statistics BM25 needs.
pub struct Index {
    pub docs: Vec<IndexedSection>,
    df: HashMap<String, usize>,
    avg_len: f64,
}

pub struct IndexedSection {
    pub document: String,
    pub section: Section,
    /// The project's own documents rank ahead of the library's on a tie.
    pub project: bool,
    tf: HashMap<String, f64>,
    len: f64,
}

/// One section found.
pub struct Hit<'a> {
    pub score: f64,
    pub item: &'a IndexedSection,
}

impl Index {
    pub fn new(items: Vec<(String, bool, Section)>) -> Index {
        let mut df: HashMap<String, usize> = HashMap::new();
        let docs: Vec<IndexedSection> = items
            .into_iter()
            .map(|(document, project, section)| {
                let mut tf: HashMap<String, f64> = HashMap::new();
                let body = words(&section.text);
                let len = body.len() as f64 + 1.0;
                for w in body {
                    *tf.entry(w).or_default() += 1.0;
                }
                // The headings and the file name say what the section is
                // about, so they weigh more than a word in passing.
                for w in words(&section.path).into_iter().chain(words(&document.replace(['_', '.'], " "))) {
                    *tf.entry(w).or_default() += 2.0;
                }
                for w in tf.keys() {
                    *df.entry(w.clone()).or_default() += 1;
                }
                IndexedSection { document, section, project, tf, len }
            })
            .collect();
        let avg_len = if docs.is_empty() { 1.0 } else { docs.iter().map(|d| d.len).sum::<f64>() / docs.len() as f64 };
        Index { docs, df, avg_len }
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit<'_>> {
        let q: Vec<String> = {
            let mut q = words(query);
            q.sort();
            q.dedup();
            q
        };
        if q.is_empty() {
            return Vec::new();
        }
        let n = self.docs.len() as f64;
        let (k1, b) = (1.2, 0.75);
        let mut hits: Vec<Hit<'_>> = self
            .docs
            .iter()
            .filter_map(|d| {
                let mut score = 0.0;
                let mut matched = 0;
                for w in &q {
                    let Some(&f) = d.tf.get(w) else { continue };
                    matched += 1;
                    let df = *self.df.get(w).unwrap_or(&0) as f64;
                    let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                    score += idf * f * (k1 + 1.0) / (f + k1 * (1.0 - b + b * d.len / self.avg_len));
                }
                // A section holding more of the words asked for ranks ahead
                // of one holding a single word many times.
                (matched > 0).then(|| Hit { score: score * (matched as f64 / q.len() as f64).sqrt() + if d.project { 0.01 } else { 0.0 }, item: d })
            })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(limit);
        hits
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

    #[test]
    fn sections_are_named_by_their_headings_and_cut_when_long() {
        let long: String = (0..400).map(|i| format!("word{i} ")).collect();
        let s = sections(&[h(1, "Design"), p("intro"), h(2, "Routing"), p("static to the core"), h(2, "Firewall"), p(&long), p(&long), h(1, "Appendix"), p("end")]);
        assert_eq!(s.iter().map(|s| s.path.as_str()).collect::<Vec<_>>(), ["Design", "Design > Routing", "Design > Firewall", "Design > Firewall", "Appendix"]);
        assert_eq!((s[2].piece, s[2].pieces), (1, 2));
    }

    #[test]
    fn addresses_and_interfaces_are_words() {
        let w = words("ping 192.0.2.10 on Eth1/2, then port-channel20.");
        for must in ["192.0.2.10", "eth1/2", "port-channel20", "ping", "eth1", "2", "channel20"] {
            assert!(w.iter().any(|x| x == must), "{must} in {w:?}");
        }
        assert!(!w.iter().any(|x| x == "then"));
        assert!(words("Window 2 – Part A").contains(&"part_a".to_string()));
    }

    #[test]
    fn the_section_with_more_of_the_words_ranks_first() {
        let sec = |path: &str, text: &str| Section { path: path.into(), text: text.into(), piece: 1, pieces: 1 };
        let ix = Index::new(vec![
            ("Runbook.docx".into(), true, sec("Part A", "shut the old uplink; rollback: no shutdown")),
            ("Runbook.docx".into(), true, sec("Part B – Routing", "static route to 10.0.0.0/8 via the transit")),
            ("Design.docx".into(), false, sec("Routing", "BGP to the fabric; route maps")),
            ("Guide.pdf".into(), false, sec("Intro", "nothing about it")),
        ]);
        let hits = ix.search("part b static route rollback", 3);
        assert_eq!(hits[0].item.section.path, "Part B – Routing");
        assert!(ix.search("10.0.0.0/8", 5).iter().any(|h| h.item.section.path == "Part B – Routing"));
        assert!(ix.search("the and of", 5).is_empty());
        let ab = Index::new(vec![
            ("R.docx".into(), true, sec("Part A – Swap", "rollback: no shutdown")),
            ("R.docx".into(), true, sec("Part B – Move", "rollback: move it back")),
        ]);
        assert_eq!(ab.search("part b rollback", 1)[0].item.section.path, "Part B – Move");
    }
}
