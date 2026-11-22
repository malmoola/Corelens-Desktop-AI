//! A document read into blocks: headings with their level, paragraphs
//! (marked when they are set in a fixed-width font, which is how a runbook
//! sets its commands apart), and tables. Word, Excel and PowerPoint files are
//! read from their XML; Markdown and text as they are; PDF by its text.
//! Nothing here writes, and nothing leaves the computer.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// One piece of a document, in reading order.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading { level: u8, text: String },
    Para { text: String, mono: bool },
    Table { rows: Vec<Vec<String>> },
}

/// The kinds of file read, by extension.
pub const KINDS: &[&str] = &["docx", "xlsx", "pptx", "pdf", "md", "markdown", "txt"];

/// The largest file read. A design document with screenshots can be large;
/// past this it is listed as too big rather than read.
pub const MAX_FILE_BYTES: u64 = 80 * 1024 * 1024;

/// The most text one part of an Office file may unpack to.
const MAX_XML_BYTES: u64 = 64 * 1024 * 1024;

pub fn kind_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    KINDS.iter().copied().find(|k| *k == ext)
}

/// The blocks of the file at `path`.
pub fn read(path: &Path) -> Result<Vec<Block>, String> {
    let kind = kind_of(path).ok_or("not a kind of document Corelens reads")?;
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE_BYTES {
        return Err(format!("larger than {} MB", MAX_FILE_BYTES / 1024 / 1024));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    from_bytes(kind, &bytes)
}

pub fn from_bytes(kind: &str, bytes: &[u8]) -> Result<Vec<Block>, String> {
    match kind {
        "docx" => docx(bytes),
        "xlsx" => xlsx(bytes),
        "pptx" => pptx(bytes),
        "pdf" => pdf(bytes),
        "md" | "markdown" => Ok(markdown(&String::from_utf8_lossy(bytes))),
        _ => Ok(text(&String::from_utf8_lossy(bytes))),
    }
}

type Archive<'a> = zip::ZipArchive<std::io::Cursor<&'a [u8]>>;

fn archive(bytes: &[u8]) -> Result<Archive<'_>, String> {
    zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| "not a readable Office file".to_string())
}

fn part(zip: &mut Archive<'_>, name: &str) -> Option<String> {
    let f = zip.by_name(name).ok()?;
    let mut s = String::new();
    f.take(MAX_XML_BYTES).read_to_string(&mut s).ok()?;
    Some(s)
}

fn parse(xml: &str) -> Result<roxmltree::Document<'_>, String> {
    roxmltree::Document::parse_with_options(xml, roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: u32::MAX }).map_err(|_| "its XML could not be read".to_string())
}

fn attr<'a>(n: roxmltree::Node<'a, '_>, local: &str) -> Option<&'a str> {
    n.attributes().find(|a| a.name() == local).map(|a| a.value())
}

fn child<'a, 'b>(n: roxmltree::Node<'a, 'b>, local: &str) -> Option<roxmltree::Node<'a, 'b>> {
    n.children().find(|c| c.tag_name().name() == local)
}

/// Fonts a command is set in.
fn is_mono_font(f: &str) -> bool {
    let f = f.to_ascii_lowercase();
    ["consolas", "courier", "lucida console", "menlo", "monaco", "cascadia", "source code", "mono", "fira code", "inconsolata", "jetbrains", "andale", "ocr"].iter().any(|m| f.contains(m))
}

/// Paragraph styles a command is set in.
fn is_code_style(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    ["code", "command", "console", "cli", "terminal", "preformat", "plaintext", "macro", "html pre"].iter().any(|m| s.contains(m))
}

/// What Word's styles say: each style's heading level (from its name or
/// its outline level), whether it is a code style, and its font.
#[derive(Default)]
struct Styles {
    level: HashMap<String, u8>,
    code: HashMap<String, bool>,
    mono: HashMap<String, bool>,
}

fn heading_level_of_name(name: &str) -> Option<u8> {
    let n = name.trim().to_ascii_lowercase();
    if n == "title" {
        return Some(1);
    }
    let rest = n.strip_prefix("heading").map(str::trim)?;
    rest.parse::<u8>().ok().filter(|l| (1..=9).contains(l))
}

fn styles(zip: &mut Archive<'_>) -> Styles {
    let mut out = Styles::default();
    let Some(xml) = part(zip, "word/styles.xml") else { return out };
    let Ok(doc) = parse(&xml) else { return out };
    for s in doc.descendants().filter(|n| n.tag_name().name() == "style") {
        let Some(id) = attr(s, "styleId") else { continue };
        let name = child(s, "name").and_then(|n| attr(n, "val")).unwrap_or(id);
        let outline = child(s, "pPr").and_then(|p| child(p, "outlineLvl")).and_then(|o| attr(o, "val")).and_then(|v| v.parse::<u8>().ok()).filter(|l| *l < 9).map(|l| l + 1);
        if let Some(l) = heading_level_of_name(name).or_else(|| heading_level_of_name(id)).or(outline) {
            out.level.insert(id.to_string(), l);
        }
        if is_code_style(name) || is_code_style(id) {
            out.code.insert(id.to_string(), true);
        }
        let font = child(s, "rPr").and_then(|r| child(r, "rFonts")).and_then(|f| attr(f, "ascii").or_else(|| attr(f, "hAnsi")));
        if let Some(f) = font {
            out.mono.insert(id.to_string(), is_mono_font(f));
        }
    }
    out
}

/// A Word paragraph's text, and how much of it is set in a fixed-width font.
fn docx_para(p: roxmltree::Node<'_, '_>, st: &Styles) -> (String, Option<u8>, bool) {
    let style = child(p, "pPr").and_then(|pp| child(pp, "pStyle")).and_then(|s| attr(s, "val")).unwrap_or("");
    let outline = child(p, "pPr").and_then(|pp| child(pp, "outlineLvl")).and_then(|o| attr(o, "val")).and_then(|v| v.parse::<u8>().ok()).filter(|l| *l < 9).map(|l| l + 1);
    let level = st.level.get(style).copied().or(outline);
    let style_mono = st.code.get(style).copied().unwrap_or(false) || st.mono.get(style).copied().unwrap_or(false);
    let (mut text, mut mono_chars, mut all_chars) = (String::new(), 0usize, 0usize);
    for r in p.descendants().filter(|n| n.tag_name().name() == "r") {
        // A run inside a deleted change is not the document's text.
        if r.ancestors().any(|a| a.tag_name().name() == "del") {
            continue;
        }
        let font = child(r, "rPr").and_then(|rp| child(rp, "rFonts")).and_then(|f| attr(f, "ascii").or_else(|| attr(f, "hAnsi")).or_else(|| attr(f, "cs")));
        let run_style_mono = child(r, "rPr").and_then(|rp| child(rp, "rStyle")).and_then(|s| attr(s, "val")).is_some_and(|s| st.code.get(s).copied().unwrap_or(false) || st.mono.get(s).copied().unwrap_or(false) || is_code_style(s));
        let mono = font.map(is_mono_font).unwrap_or(style_mono) || run_style_mono;
        let mut piece = String::new();
        for c in r.children() {
            match c.tag_name().name() {
                "t" => piece.push_str(c.text().unwrap_or("")),
                "tab" => piece.push('\t'),
                "br" | "cr" => piece.push('\n'),
                _ => {}
            }
        }
        let n = piece.chars().filter(|c| !c.is_whitespace()).count();
        all_chars += n;
        if mono {
            mono_chars += n;
        }
        text.push_str(&piece);
    }
    let mono = all_chars > 0 && mono_chars * 10 >= all_chars * 8;
    (text, level, mono)
}

fn docx_table(t: roxmltree::Node<'_, '_>, st: &Styles) -> Vec<Vec<String>> {
    t.children()
        .filter(|r| r.tag_name().name() == "tr")
        .map(|r| {
            r.children()
                .filter(|c| c.tag_name().name() == "tc")
                .map(|c| c.children().filter(|p| p.tag_name().name() == "p").map(|p| docx_para(p, st).0).collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" "))
                .collect()
        })
        .filter(|row: &Vec<String>| row.iter().any(|c| !c.is_empty()))
        .collect()
}

fn docx(bytes: &[u8]) -> Result<Vec<Block>, String> {
    let mut zip = archive(bytes)?;
    let st = styles(&mut zip);
    let xml = part(&mut zip, "word/document.xml").ok_or("no document text in it")?;
    let doc = parse(&xml)?;
    let body = doc.descendants().find(|n| n.tag_name().name() == "body").ok_or("no document body")?;
    let mut out = Vec::new();
    walk_docx(body, &st, &mut out);
    Ok(out)
}

/// The body's paragraphs and tables in order; content controls and the
/// like are looked into.
fn walk_docx(n: roxmltree::Node<'_, '_>, st: &Styles, out: &mut Vec<Block>) {
    for c in n.children() {
        match c.tag_name().name() {
            "p" => {
                let (text, level, mono) = docx_para(c, st);
                push_para(out, text, level, mono);
            }
            "tbl" => {
                let rows = docx_table(c, st);
                if !rows.is_empty() {
                    out.push(Block::Table { rows });
                }
            }
            "sdt" | "sdtContent" | "customXml" | "smartTag" => walk_docx(c, st, out),
            _ => {}
        }
    }
}

fn push_para(out: &mut Vec<Block>, text: String, level: Option<u8>, mono: bool) {
    let trimmed = text.trim_end();
    if trimmed.trim().is_empty() {
        return;
    }
    match level {
        Some(level) => out.push(Block::Heading { level, text: trimmed.trim().to_string() }),
        None if mono => {
            // A line break inside a code paragraph is a new command.
            for line in trimmed.split('\n').filter(|l| !l.trim().is_empty()) {
                out.push(Block::Para { text: line.trim_end().to_string(), mono: true });
            }
        }
        None => out.push(Block::Para { text: trimmed.split_whitespace().collect::<Vec<_>>().join(" "), mono: false }),
    }
}

/// An Excel workbook: a heading per sheet, then its rows.
fn xlsx(bytes: &[u8]) -> Result<Vec<Block>, String> {
    let mut zip = archive(bytes)?;
    let shared: Vec<String> = part(&mut zip, "xl/sharedStrings.xml")
        .and_then(|x| {
            let d = parse(&x).ok()?;
            Some(d.root_element().children().filter(|n| n.tag_name().name() == "si").map(|si| si.descendants().filter(|t| t.tag_name().name() == "t").filter_map(|t| t.text()).collect::<String>()).collect())
        })
        .unwrap_or_default();
    let wb = part(&mut zip, "xl/workbook.xml").ok_or("no workbook in it")?;
    let rels = part(&mut zip, "xl/_rels/workbook.xml.rels").unwrap_or_default();
    let targets: HashMap<String, String> = parse(&rels)
        .map(|d| d.descendants().filter(|n| n.tag_name().name() == "Relationship").filter_map(|r| Some((attr(r, "Id")?.to_string(), attr(r, "Target")?.to_string()))).collect())
        .unwrap_or_default();
    let sheets: Vec<(String, String)> = {
        let d = parse(&wb)?;
        d.descendants().filter(|n| n.tag_name().name() == "sheet").filter_map(|s| Some((attr(s, "name")?.to_string(), targets.get(attr(s, "id")?)?.clone()))).collect()
    };
    let mut out = Vec::new();
    for (name, target) in sheets {
        let path = if let Some(abs) = target.strip_prefix('/') { abs.to_string() } else { format!("xl/{}", target.trim_start_matches("./")) };
        let Some(x) = part(&mut zip, &path) else { continue };
        let Ok(d) = parse(&x) else { continue };
        let mut rows = Vec::new();
        for row in d.descendants().filter(|n| n.tag_name().name() == "row") {
            let mut cells: Vec<String> = Vec::new();
            for c in row.children().filter(|n| n.tag_name().name() == "c") {
                let col = attr(c, "r").map(column_of).unwrap_or(cells.len());
                while cells.len() < col {
                    cells.push(String::new());
                }
                let v = child(c, "v").and_then(|v| v.text()).unwrap_or("");
                let value = match attr(c, "t") {
                    Some("s") => v.parse::<usize>().ok().and_then(|i| shared.get(i)).cloned().unwrap_or_default(),
                    Some("inlineStr") => c.descendants().filter(|t| t.tag_name().name() == "t").filter_map(|t| t.text()).collect(),
                    Some("b") => (if v == "1" { "TRUE" } else { "FALSE" }).to_string(),
                    _ => v.to_string(),
                };
                cells.push(value.split_whitespace().collect::<Vec<_>>().join(" "));
            }
            while cells.last().is_some_and(String::is_empty) {
                cells.pop();
            }
            if !cells.is_empty() {
                rows.push(cells);
            }
        }
        if !rows.is_empty() {
            out.push(Block::Heading { level: 1, text: name });
            out.push(Block::Table { rows });
        }
    }
    Ok(out)
}

/// "C7" → 2.
fn column_of(r: &str) -> usize {
    r.chars().take_while(|c| c.is_ascii_alphabetic()).fold(0usize, |n, c| n * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1)).saturating_sub(1)
}

/// A PowerPoint deck: a heading per slide, then what the slide says.
fn pptx(bytes: &[u8]) -> Result<Vec<Block>, String> {
    let mut zip = archive(bytes)?;
    let names: Vec<String> = zip.file_names().filter_map(|n| n.ok()).map(|n| n.to_string()).collect();
    let mut slides: Vec<(usize, String)> = names
        .iter()
        .filter_map(|n| n.strip_prefix("ppt/slides/slide").and_then(|r| r.strip_suffix(".xml")).and_then(|d| d.parse::<usize>().ok()).map(|i| (i, n.to_string())))
        .collect();
    slides.sort();
    let mut out = Vec::new();
    for (i, name) in slides {
        let Some(x) = part(&mut zip, &name) else { continue };
        let Ok(d) = parse(&x) else { continue };
        let paras: Vec<String> = d
            .descendants()
            .filter(|n| n.tag_name().name() == "p" && n.tag_name().namespace().is_some_and(|ns| ns.contains("drawingml")))
            .map(|p| p.descendants().filter(|t| t.tag_name().name() == "t").filter_map(|t| t.text()).collect::<String>())
            .filter(|t| !t.trim().is_empty())
            .collect();
        let title = paras.first().cloned().unwrap_or_default();
        out.push(Block::Heading { level: 1, text: if title.is_empty() { format!("Slide {i}") } else { format!("Slide {i}: {title}") } });
        for p in paras.into_iter().skip(1) {
            push_para(&mut out, p, None, false);
        }
    }
    Ok(out)
}

/// A PDF's text. A PDF keeps no headings or fonts that say what a line is,
/// so it is paragraphs only; a line that looks like a numbered heading
/// ("3.2 Routing", "Part A – …") is taken for one.
fn pdf(bytes: &[u8]) -> Result<Vec<Block>, String> {
    let text = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes)).map_err(|_| "its text could not be read".to_string())?.map_err(|_| "its text could not be read (scanned, or protected)".to_string())?;
    if text.trim().is_empty() {
        return Err("no text in it (a scan, perhaps)".into());
    }
    Ok(text_with_headings(&text))
}

fn text_with_headings(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut para = String::new();
    let flush = |para: &mut String, out: &mut Vec<Block>| {
        if !para.trim().is_empty() {
            out.push(Block::Para { text: para.split_whitespace().collect::<Vec<_>>().join(" "), mono: false });
        }
        para.clear();
    };
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            flush(&mut para, &mut out);
        } else if l.len() < 90 && looks_like_heading(l) {
            flush(&mut para, &mut out);
            out.push(Block::Heading { level: 2, text: l.to_string() });
        } else {
            para.push(' ');
            para.push_str(l);
        }
    }
    flush(&mut para, &mut out);
    out
}

fn looks_like_heading(l: &str) -> bool {
    super::runbook::part_heading(l).is_some() || {
        let mut w = l.split_whitespace();
        let first = w.next().unwrap_or("");
        let numbered = !first.is_empty() && first.trim_end_matches('.').split('.').all(|p| !p.is_empty() && p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit()));
        numbered && w.next().is_some_and(|n| n.chars().next().is_some_and(char::is_uppercase)) && !l.ends_with('.')
    }
}

/// Markdown: `#` headings, fenced and indented code, pipe tables.
pub fn markdown(src: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut fence = false;
    let mut para = String::new();
    let mut table: Vec<Vec<String>> = Vec::new();
    let flush = |para: &mut String, table: &mut Vec<Vec<String>>, out: &mut Vec<Block>| {
        if !para.trim().is_empty() {
            out.push(Block::Para { text: para.split_whitespace().collect::<Vec<_>>().join(" "), mono: false });
        }
        para.clear();
        if !table.is_empty() {
            out.push(Block::Table { rows: std::mem::take(table) });
        }
    };
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            flush(&mut para, &mut table, &mut out);
            fence = !fence;
            continue;
        }
        if fence {
            if !t.is_empty() {
                out.push(Block::Para { text: line.trim_end().to_string(), mono: true });
            }
            continue;
        }
        if t.is_empty() {
            flush(&mut para, &mut table, &mut out);
            continue;
        }
        let hashes = t.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
            flush(&mut para, &mut table, &mut out);
            out.push(Block::Heading { level: hashes as u8, text: t[hashes..].trim().trim_end_matches('#').trim().to_string() });
            continue;
        }
        if t.starts_with('|') && t.ends_with('|') && t.len() > 1 {
            if !para.trim().is_empty() {
                let p = std::mem::take(&mut para);
                out.push(Block::Para { text: p.split_whitespace().collect::<Vec<_>>().join(" "), mono: false });
            }
            let cells: Vec<String> = t[1..t.len() - 1].split('|').map(|c| c.trim().to_string()).collect();
            if !cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' '))) {
                table.push(cells);
            }
            continue;
        }
        if line.starts_with("    ") || line.starts_with('\t') {
            flush(&mut para, &mut table, &mut out);
            out.push(Block::Para { text: line.trim_start_matches('\t').strip_prefix("    ").unwrap_or(line).trim_end().to_string(), mono: true });
            continue;
        }
        // A list item or a numbered step starts a paragraph of its own.
        let starts_item = t.starts_with("- ") || t.starts_with("* ") || super::runbook::step_line(t).is_some();
        if starts_item {
            flush(&mut para, &mut table, &mut out);
            para.push_str(t.trim_start_matches("- ").trim_start_matches("* "));
            continue;
        }
        if t.starts_with('`') && t.ends_with('`') && t.len() > 2 && !t[1..t.len() - 1].contains('`') {
            flush(&mut para, &mut table, &mut out);
            out.push(Block::Para { text: t[1..t.len() - 1].to_string(), mono: true });
            continue;
        }
        para.push(' ');
        para.push_str(t);
    }
    flush(&mut para, &mut table, &mut out);
    out
}

/// Plain text: paragraphs between blank lines, numbered-heading lines kept
/// as headings.
pub fn text(src: &str) -> Vec<Block> {
    text_with_headings(src)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// A Word file built in the test: `paras` are (style, font, text); a
    /// style of `"table"` makes a one-row table of the text split on `|`.
    pub(crate) fn docx_of(paras: &[(&str, &str, &str)]) -> Vec<u8> {
        let mut body = String::new();
        for (style, font, text) in paras {
            if *style == "table" {
                let cells: String = text.split('|').map(|c| format!("<w:tc><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:tc>", esc(c.trim()))).collect();
                body.push_str(&format!("<w:tbl><w:tr>{cells}</w:tr></w:tbl>"));
                continue;
            }
            let ppr = if style.is_empty() { String::new() } else { format!("<w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>") };
            let rpr = if font.is_empty() { String::new() } else { format!("<w:rPr><w:rFonts w:ascii=\"{font}\" w:hAnsi=\"{font}\"/></w:rPr>") };
            body.push_str(&format!("<w:p>{ppr}<w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>", esc(text)));
        }
        let ns = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";
        let document = format!("<?xml version=\"1.0\"?><w:document {ns}><w:body>{body}</w:body></w:document>");
        let styles = format!("<?xml version=\"1.0\"?><w:styles {ns}><w:style w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/></w:style><w:style w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/></w:style><w:style w:styleId=\"Kop3\"><w:name w:val=\"Kop 3\"/><w:pPr><w:outlineLvl w:val=\"2\"/></w:pPr></w:style><w:style w:styleId=\"Code\"><w:name w:val=\"Code\"/></w:style></w:styles>");
        zip_of(&[("word/document.xml", &document), ("word/styles.xml", &styles)])
    }

    fn esc(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    pub(crate) fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            for (name, body) in files {
                z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn word_headings_commands_and_tables_are_told_apart() {
        let bytes = docx_of(&[
            ("Heading1", "", "Part A – Swap the edge"),
            ("", "Calibri", "A1. EDGE-1 – shut the old uplink:"),
            ("", "Consolas", "interface Gi0/1"),
            ("Code", "", " shutdown"),
            ("table", "", "Check | Expected"),
            ("Kop3", "", "Outline-level heading"),
        ]);
        let b = from_bytes("docx", &bytes).unwrap();
        assert_eq!(
            b,
            vec![
                Block::Heading { level: 1, text: "Part A – Swap the edge".into() },
                Block::Para { text: "A1. EDGE-1 – shut the old uplink:".into(), mono: false },
                Block::Para { text: "interface Gi0/1".into(), mono: true },
                Block::Para { text: " shutdown".into(), mono: true },
                Block::Table { rows: vec![vec!["Check".into(), "Expected".into()]] },
                Block::Heading { level: 3, text: "Outline-level heading".into() },
            ]
        );
    }

    #[test]
    fn a_workbook_is_a_heading_per_sheet_and_its_rows() {
        let ns = "xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";
        let wb = format!("<workbook {ns}><sheets><sheet name=\"VLANs\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>");
        let rels = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Target=\"worksheets/sheet1.xml\" Type=\"x\"/></Relationships>";
        let ss = format!("<sst {ns}><si><t>VLAN</t></si><si><t>Name</t></si><si><t>Users</t></si></sst>");
        let sheet = format!("<worksheet {ns}><sheetData><row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\" t=\"s\"><v>1</v></c></row><row r=\"2\"><c r=\"A2\"><v>10</v></c><c r=\"C2\" t=\"s\"><v>2</v></c></row></sheetData></worksheet>");
        let bytes = zip_of(&[("xl/workbook.xml", &wb), ("xl/_rels/workbook.xml.rels", rels), ("xl/sharedStrings.xml", &ss), ("xl/worksheets/sheet1.xml", &sheet)]);
        let b = from_bytes("xlsx", &bytes).unwrap();
        assert_eq!(b[0], Block::Heading { level: 1, text: "VLANs".into() });
        assert_eq!(b[1], Block::Table { rows: vec![vec!["VLAN".into(), "Name".into()], vec!["10".into(), String::new(), "Users".into()]] });
    }

    #[test]
    fn markdown_keeps_headings_code_and_tables() {
        let b = markdown("# Runbook\n\n## Part B – Routing\n\n1. CORE-1:\n\n```\nshow ip route\n```\n\n| Check | Expected |\n|---|---|\n| route | present |\n");
        assert_eq!(b[0], Block::Heading { level: 1, text: "Runbook".into() });
        assert_eq!(b[1], Block::Heading { level: 2, text: "Part B – Routing".into() });
        assert_eq!(b[2], Block::Para { text: "1. CORE-1:".into(), mono: false });
        assert_eq!(b[3], Block::Para { text: "show ip route".into(), mono: true });
        assert_eq!(b[4], Block::Table { rows: vec![vec!["Check".into(), "Expected".into()], vec!["route".into(), "present".into()]] });
    }

    #[test]
    fn a_broken_file_says_so_rather_than_panicking() {
        assert!(from_bytes("docx", b"not a zip").is_err());
        assert!(from_bytes("xlsx", b"").is_err());
        assert!(from_bytes("pdf", b"%PDF-1.4 nonsense").is_err());
    }
}
