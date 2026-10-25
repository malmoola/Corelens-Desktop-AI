//! A command's output into rows, as its playbook entry says.

use std::path::Path;
use std::sync::OnceLock;

use corelens_catalog::textfsm::{Engine, Index};
use regex::Regex;
use serde_json::{Map, Value};

use crate::playbook::{BlocksSpec, LinesSpec, Named, Parser};

pub type Row = Map<String, Value>;

/// The ntc templates, loaded once per directory.
pub struct Templates {
    index: Index,
    engine: Engine,
}

impl Templates {
    pub fn load(dir: &Path) -> Result<Templates, String> {
        Ok(Templates { index: Index::load(dir)?, engine: Engine::new(dir) })
    }

    /// Rows from the template the index names, or why there are none.
    pub fn parse(&self, platform: &str, command: &str, output: &str) -> Result<Vec<Row>, String> {
        let names = self.index.lookup(platform, command.trim()).ok_or_else(|| format!("no template reads `{command}` on {platform}"))?;
        let (first, also) = names.split_first().ok_or("an empty index row")?;
        self.engine.parse(first, also, output)
    }
}

/// The templates under `dir`, loaded the first time they are wanted.
pub fn templates(dir: &Path) -> Option<&'static Templates> {
    static T: OnceLock<Option<(std::path::PathBuf, Templates)>> = OnceLock::new();
    T.get_or_init(|| Templates::load(dir).ok().map(|t| (dir.to_path_buf(), t))).as_ref().filter(|(d, _)| d == dir).map(|(_, t)| t)
}

/// What a device says when it does not know a command.
pub fn refused(output: &str) -> bool {
    let l = output.to_ascii_lowercase();
    l.contains("% invalid input")
        || l.contains("% incomplete command")
        || l.contains("% unrecognized command")
        || l.contains("invalid command")
        || l.contains("unknown command")
        || l.contains("syntax error")
        || l.contains("command parse error")
        || l.contains("command fail")
        || l.contains("% ambiguous command")
}

fn captures(re: &Regex, line: &str) -> Option<Row> {
    let c = re.captures(line)?;
    let mut row = Row::new();
    for name in re.capture_names().flatten() {
        if let Some(m) = c.name(name) {
            row.insert(name.to_string(), Value::String(m.as_str().trim().to_string()));
        }
    }
    Some(row)
}

/// Rows from `output`, read the way `parser` says.
pub fn rows(parser: &Parser, templates: Option<&Templates>, platform: &str, command: &str, output: &str) -> Result<Vec<Row>, String> {
    let output = output.replace("\r\n", "\n");
    match parser {
        Parser::Named(Named::Ntc) => templates.ok_or("the parser templates are not available")?.parse(platform, command, &output),
        Parser::Named(Named::Text) => Ok(Vec::new()),
        Parser::Lines { lines: LinesSpec { pattern, after } } => {
            let re = Regex::new(pattern).map_err(|e| e.to_string())?;
            let after = after.as_deref().map(Regex::new).transpose().map_err(|e| e.to_string())?;
            let mut started = after.is_none();
            let mut out = Vec::new();
            for line in output.lines() {
                if !started {
                    started = after.as_ref().is_some_and(|a| a.is_match(line));
                    continue;
                }
                if let Some(row) = captures(&re, line) {
                    out.push(row);
                }
            }
            Ok(out)
        }
        Parser::Blocks { blocks: BlocksSpec { start, fields } } => {
            let start = Regex::new(start).map_err(|e| e.to_string())?;
            let fields: Vec<(String, Regex)> = fields.iter().map(|(k, p)| Regex::new(p).map(|r| (k.clone(), r))).collect::<Result<_, _>>().map_err(|e| e.to_string())?;
            let mut blocks: Vec<(Row, String)> = Vec::new();
            for line in output.lines() {
                if let Some(row) = captures(&start, line) {
                    blocks.push((row, String::new()));
                } else if let Some((_, text)) = blocks.last_mut() {
                    text.push_str(line);
                    text.push('\n');
                }
            }
            Ok(blocks
                .into_iter()
                .map(|(mut row, text)| {
                    for (name, re) in &fields {
                        if let Some(v) = re.captures(&text).and_then(|c| c.get(1)) {
                            row.insert(name.clone(), Value::String(v.as_str().trim().to_string()));
                        }
                    }
                    row
                })
                .collect())
        }
    }
}

/// A field as text: a list is joined with commas.
pub fn text(row: &Row, field: &str) -> String {
    match row.get(field) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(","),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// The first number in a field ("5%", "1,234", "-3.2 dBm"), if any.
pub fn number(s: &str) -> Option<f64> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"-?\d[\d,]*(?:\.\d+)?").expect("a fixed pattern"));
    re.find(s).and_then(|m| m.as_str().replace(',', "").parse().ok())
}

/// A duration as devices write one — "2 days, 9 hours, 12 minutes",
/// "1 week, 3 days", "3w2d", "02:14:09", "5 minutes" — in minutes.
pub fn minutes(s: &str) -> Option<f64> {
    static WORDS: OnceLock<Regex> = OnceLock::new();
    static CLOCK: OnceLock<Regex> = OnceLock::new();
    let words = WORDS.get_or_init(|| Regex::new(r"(?i)(\d+)\s*(years?|y|weeks?|w|days?|d|hours?|h|minutes?|mins?|m|seconds?|secs?|s)").expect("fixed"));
    let clock = CLOCK.get_or_init(|| Regex::new(r"^(?:(\d+)\s*(?:days?|d),?\s*)?(\d{1,2}):(\d{2})(?::(\d{2}))?$").expect("fixed"));
    let s = s.trim();
    if let Some(c) = clock.captures(s) {
        let d: f64 = c.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0.0);
        let h: f64 = c[2].parse().ok()?;
        let m: f64 = c[3].parse().ok()?;
        return Some(d * 1440.0 + h * 60.0 + m);
    }
    let mut total = 0.0;
    let mut any = false;
    for c in words.captures_iter(s) {
        let n: f64 = c[1].parse().ok()?;
        let unit = c[2].to_ascii_lowercase();
        total += n * match unit.as_str() {
            u if u.starts_with('y') => 525_600.0,
            u if u.starts_with('w') => 10_080.0,
            u if u.starts_with('d') => 1_440.0,
            u if u.starts_with('h') => 60.0,
            u if u.starts_with("mi") || u == "m" => 1.0,
            _ => 1.0 / 60.0,
        };
        any = true;
    }
    any.then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_as_devices_write_them() {
        assert_eq!(minutes("2 days, 9 hours, 12 minutes"), Some(2.0 * 1440.0 + 9.0 * 60.0 + 12.0));
        assert_eq!(minutes("1 week, 3 days, 4 hours, 1 minute"), Some(10080.0 + 3.0 * 1440.0 + 240.0 + 1.0));
        assert_eq!(minutes("3w2d"), Some(3.0 * 10080.0 + 2.0 * 1440.0));
        assert_eq!(minutes("02:14:09"), Some(134.0));
        assert_eq!(minutes("2d 03:07"), Some(2.0 * 1440.0 + 187.0));
        assert_eq!(minutes("12 days, 3:04:05"), Some(12.0 * 1440.0 + 184.0));
        assert_eq!(minutes("5 minutes"), Some(5.0));
        assert_eq!(minutes("never"), None);
    }

    #[test]
    fn numbers_inside_text() {
        assert_eq!(number("5%"), Some(5.0));
        assert_eq!(number("1,234 packets"), Some(1234.0));
        assert_eq!(number("-3.2 dBm"), Some(-3.2));
        assert_eq!(number("N/A"), None);
    }

    #[test]
    fn lines_and_blocks() {
        let p = Parser::Lines { lines: LinesSpec { pattern: r"^(?P<port>\S+)\s+(?P<state>up|down)$".into(), after: Some("^Port".into()) } };
        let r = rows(&p, None, "x", "x", "junk up\nPort State\nGi0/1   up\nGi0/2 down\n").unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(text(&r[1], "port"), "Gi0/2");
        let b = Parser::Blocks { blocks: BlocksSpec { start: r"^RADIUS: id (?P<id>\d+)".into(), fields: [("state".to_string(), r"State: current (\w+)".to_string())].into() } };
        let r = rows(&b, None, "x", "x", "RADIUS: id 1, priority 1, host 192.0.2.5\n  State: current UP, duration 9s\nRADIUS: id 2\n  State: current DEAD\n").unwrap();
        assert_eq!((text(&r[0], "state").as_str(), text(&r[1], "state").as_str()), ("UP", "DEAD"));
    }
}
