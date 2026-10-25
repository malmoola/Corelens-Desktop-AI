//! A diagnosis as a PDF: the Markdown report laid out on A4 pages, with
//! the same SVG-to-PDF writer the diagram export uses, so text stays text.
//! Headings are larger, evidence and commands monospace, long lines wrapped
//! by an estimate of their width; nothing in the report is read as markup.

const W: f64 = 595.0;
const H: f64 = 842.0;
const MARGIN: f64 = 48.0;

struct Line {
    text: String,
    size: f64,
    bold: bool,
    mono: bool,
    indent: f64,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Breaks `text` so each piece fits `chars` characters, on spaces where it can.
fn wrap(text: &str, chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let mut word = word.to_string();
        while word.chars().count() > chars {
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            let head: String = word.chars().take(chars).collect();
            word = word.chars().skip(chars).collect();
            out.push(head);
        }
        if line.chars().count() + word.chars().count() + 1 > chars && !line.is_empty() {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    out.push(line);
    out
}

fn lines(md: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut fenced = false;
    for raw in md.lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        let (text, size, bold, mono, indent) = if fenced {
            (raw.trim_end().to_string(), 8.0, false, true, 14.0)
        } else if let Some(t) = raw.strip_prefix("# ") {
            (t.to_string(), 18.0, true, false, 0.0)
        } else if let Some(t) = raw.strip_prefix("## ") {
            (t.to_string(), 14.0, true, false, 0.0)
        } else if let Some(t) = raw.strip_prefix("### ") {
            (t.to_string(), 11.0, true, false, 0.0)
        } else if raw.trim() == "---" {
            (String::new(), 9.0, false, false, 0.0)
        } else {
            let depth = (raw.len() - trimmed.len()) as f64;
            (trimmed.replace("**", "").replace('`', ""), 9.5, false, false, depth * 4.0)
        };
        let per_char = if mono { size * 0.6 } else { size * 0.52 };
        let chars = (((W - 2.0 * MARGIN - indent) / per_char) as usize).max(20);
        for piece in wrap(&text, chars) {
            out.push(Line { text: piece, size, bold, mono, indent });
        }
    }
    out
}

/// The pages as SVG documents.
pub fn pages(md: &str) -> Vec<String> {
    let mut pages = Vec::new();
    let mut body = String::new();
    let mut y = MARGIN;
    let mut n = 1;
    let footer = |n: usize| format!(r##"<text x="{}" y="{}" font-family="sans-serif" font-size="8" fill="#666" text-anchor="end">Corelens Desktop AI · diagnosis · page {n}</text>"##, W - MARGIN, H - 24.0);
    for l in lines(md) {
        let step = l.size * 1.45 + if l.bold { 4.0 } else { 0.0 };
        if y + step > H - MARGIN {
            pages.push(format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}"><rect width="100%" height="100%" fill="#fff"/>{body}{}</svg>"##, footer(n)));
            body.clear();
            y = MARGIN;
            n += 1;
        }
        y += step;
        if l.text.is_empty() {
            continue;
        }
        body.push_str(&format!(
            r##"<text x="{}" y="{y:.1}" font-family="{}" font-size="{}" font-weight="{}" fill="#111" xml:space="preserve">{}</text>"##,
            MARGIN + l.indent,
            if l.mono { "monospace" } else { "sans-serif" },
            l.size,
            if l.bold { "bold" } else { "normal" },
            esc(&l.text)
        ));
    }
    pages.push(format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}"><rect width="100%" height="100%" fill="#fff"/>{body}{}</svg>"##, footer(n)));
    pages
}

pub fn markdown_pdf(md: &str) -> Result<Vec<u8>, String> {
    crate::pdf::svgs_to_pdf(&pages(md))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_report_breaks_into_pages_and_escapes_what_the_network_said() {
        let mut md = String::from("# Diagnosis — <Lab>\n\n## Findings\n\n```text\nGi0/1 description <script>&\"\n```\n");
        for i in 0..200 {
            md.push_str(&format!("- finding {i} with a sentence long enough to need wrapping across the width of an A4 page at this size, twice over\n"));
        }
        let p = pages(&md);
        assert!(p.len() > 3, "{} pages", p.len());
        assert!(p[0].contains("&lt;script&gt;&amp;&quot;") && !p[0].contains("<script>"));
        assert!(p.last().unwrap().contains(&format!("page {}", p.len())));
        let pdf = markdown_pdf(&md).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
    }
}
