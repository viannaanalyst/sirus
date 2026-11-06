//! What the attachment viewer reads: an attachment's facts, the kind of viewer it gets, and the
//! parsers for what the host sends back (`attachment_preview`, ADR-184 in Sirus). No UI types
//! except the thumbnail, so the parsers are tested without a window. Everything here may block:
//! call it from a background thread.
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// What the host says about one attachment (a reply of `paste_prompt_attachments`, or a sent
/// message's `attachments` entry, which has no content).
#[derive(Clone)]
pub struct Info {
    pub id: String,
    pub name: String,
    /// `file` or `folder`.
    pub kind: String,
    pub mime: String,
    pub size: Option<u64>,
    /// The text of a text file (at most 12 KB, see `truncated`); empty for binary files.
    pub content: String,
    pub truncated: bool,
    /// An image's small copy for the chip.
    pub thumb: Option<Arc<gpui::Image>>,
}

impl Info {
    pub fn from_json(value: &Value) -> Info {
        Self::read(value, true)
    }

    /// A sent message's entry: the same, without decoding a thumbnail each time the turns rebuild.
    pub fn from_message(value: &Value) -> Info {
        Self::read(value, false)
    }

    fn read(value: &Value, thumbnail: bool) -> Info {
        let text = |key: &str| value[key].as_str().unwrap_or("").to_owned();
        Info {
            id: text("id"),
            name: text("name"),
            kind: value["kind"].as_str().unwrap_or("file").to_owned(),
            mime: text("mimeType"),
            size: value["size"].as_u64(),
            content: text("content"),
            truncated: value["truncated"].as_bool().unwrap_or(false),
            thumb: if thumbnail {
                value["previewUrl"]
                    .as_str()
                    .or(value["thumbnail"].as_str())
                    .and_then(data_url_image)
            } else {
                None
            },
        }
    }

    pub fn viewer(&self) -> Viewer {
        classify(
            &self.name,
            &self.mime,
            self.kind == "folder",
            !self.content.is_empty(),
        )
    }

    /// True for a picture, which opens in the gallery.
    pub fn is_image(&self) -> bool {
        self.kind == "file" && !self.id.is_empty() && self.viewer() == Viewer::Image
    }

    /// True when the chip opens something.
    pub fn openable(&self) -> bool {
        self.kind == "file" && !self.id.is_empty() && self.viewer() != Viewer::None
    }
}

/// The pictures among `files`, in order: the scope of the gallery for one composer or message.
pub fn images_of(files: &[Info]) -> Vec<Info> {
    files.iter().filter(|f| f.is_image()).cloned().collect()
}

/// Where the file called `id` sits among `gallery`, and the neighbour `by` steps away (None at the ends).
pub fn gallery_step(gallery: &[Info], id: &str, by: isize) -> Option<usize> {
    let at = gallery.iter().position(|f| f.id == id)? as isize;
    let target = at + by;
    (target >= 0 && (target as usize) < gallery.len()).then_some(target as usize)
}

/// The small image behind a `data:image/...;base64,` URL.
fn data_url_image(url: &str) -> Option<Arc<gpui::Image>> {
    let rest = url.strip_prefix("data:image/")?;
    let (format, data) = rest.split_once(";base64,")?;
    let format = image_format(&format!("x.{format}"), "")?;
    let bytes = STANDARD.decode(data).ok()?;
    Some(Arc::new(gpui::Image::from_bytes(format, bytes)))
}

/// Which viewer an attachment gets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Viewer {
    Pdf,
    Docx,
    Xlsx,
    Csv,
    Tsv,
    Pptx,
    /// DOC, XLS and PPT: Sirus asks to convert them; the host says so.
    Legacy,
    Image,
    Html,
    Text,
    None,
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

pub fn classify(name: &str, mime: &str, folder: bool, has_text: bool) -> Viewer {
    if folder {
        return Viewer::None;
    }
    let mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match extension(name).as_str() {
        "pdf" => return Viewer::Pdf,
        "docx" => return Viewer::Docx,
        "xlsx" => return Viewer::Xlsx,
        "csv" => return Viewer::Csv,
        "tsv" => return Viewer::Tsv,
        "pptx" => return Viewer::Pptx,
        "doc" | "xls" | "ppt" => return Viewer::Legacy,
        "html" | "htm" => return Viewer::Html,
        _ => {}
    }
    match mime.as_str() {
        "application/pdf" => return Viewer::Pdf,
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            return Viewer::Docx;
        }
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            return Viewer::Xlsx;
        }
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            return Viewer::Pptx;
        }
        "text/csv" => return Viewer::Csv,
        "text/tab-separated-values" => return Viewer::Tsv,
        "text/html" => return Viewer::Html,
        _ => {}
    }
    if mime.starts_with("image/") || image_format(name, &mime).is_some() {
        return Viewer::Image;
    }
    if has_text { Viewer::Text } else { Viewer::None }
}

pub fn image_format(name: &str, mime: &str) -> Option<gpui::ImageFormat> {
    use gpui::ImageFormat::*;
    Some(match (extension(name).as_str(), mime) {
        ("png", _) | (_, "image/png") => Png,
        ("jpg" | "jpeg", _) | (_, "image/jpeg") => Jpeg,
        ("gif", _) | (_, "image/gif") => Gif,
        ("webp", _) | (_, "image/webp") => Webp,
        ("bmp", _) | (_, "image/bmp") => Bmp,
        ("tif" | "tiff", _) | (_, "image/tiff") => Tiff,
        ("svg", _) | (_, "image/svg+xml") => Svg,
        _ => return None,
    })
}

/// "812 B", "6 KB", "1.4 MB": Sirus's `formatFileSize`.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", (bytes as f64 / 1024.).round() as u64)
    } else {
        format!("{:.1} MB", bytes as f64 / 1024. / 1024.)
    }
}

/// A column's letters: 0 is A, 26 is AA.
pub fn column_name(column: usize) -> String {
    let mut value = column + 1;
    let mut out = Vec::new();
    while value > 0 {
        value -= 1;
        out.push(b'A' + (value % 26) as u8);
        value /= 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

// ---- Word -------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum WordBlock {
    Paragraph {
        text: String,
        heading: u8,
        list: bool,
    },
    Table {
        rows: Vec<Vec<String>>,
    },
}

// ---- Spreadsheets -----------------------------------------------------------------------

/// One worksheet. Rows are as long as their last filled cell; a missing cell is empty.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
    pub columns: usize,
    pub formulas: HashMap<(usize, usize), String>,
    /// Pixel width of each column, from the longest value among the first rows.
    pub widths: Vec<f32>,
}

impl Sheet {
    pub fn value(&self, row: usize, column: usize) -> &str {
        self.rows
            .get(row)
            .and_then(|r| r.get(column))
            .map_or("", String::as_str)
    }

    fn measure(&mut self) {
        self.widths = (0..self.columns)
            .map(|column| {
                let longest = self
                    .rows
                    .iter()
                    .take(200)
                    .map(|row| row.get(column).map_or(0, |v| v.chars().take(40).count()))
                    .max()
                    .unwrap_or(0);
                (longest as f32 * 7.2 + 20.).clamp(64., 300.)
            })
            .collect();
    }
}

/// Rows of text from CSV/TSV text: quotes, doubled quotes and line breaks inside quotes.
pub fn parse_delimited(text: &str, delimiter: char) -> Sheet {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (mut rows, mut row, mut cell) = (Vec::new(), Vec::new(), String::new());
    let (mut quoted, mut chars) = (false, text.chars().peekable());
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == delimiter && !quoted => row.push(std::mem::take(&mut cell)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            c => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut sheet = Sheet {
        name: String::new(),
        rows,
        columns,
        ..Sheet::default()
    };
    sheet.measure();
    sheet
}

// ---- Slides -----------------------------------------------------------------------------

/// The text of the `<a:t>` runs of a slide's XML, one paragraph per line.
pub fn slide_text(xml: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in xml.split("</a:p>") {
        let mut line = String::new();
        let mut rest = paragraph;
        while let Some(open) = rest.find("<a:t") {
            let after = &rest[open + 4..];
            // `<a:t>` or `<a:t xml:space="preserve">`, not `<a:tab>` or `<a:tbl>`.
            if !after.starts_with('>') && !after.starts_with(' ') {
                rest = after;
                continue;
            }
            let Some(start) = after.find('>') else { break };
            let body = &after[start + 1..];
            let Some(end) = body.find("</a:t>") else {
                break;
            };
            line.push_str(&unescape(&body[..end]));
            rest = &body[end + 6..];
        }
        if !line.trim().is_empty() {
            lines.push(line);
        }
    }
    lines
}

pub fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(end) = tail.find(';').filter(|e| *e <= 8) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[1..end];
        match name {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let code = name
                    .strip_prefix("#x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()));
                match code.and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => out.push_str(&tail[..=end]),
                }
            }
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The slides of a PPTX, in order: each is its lines of text. The host has already checked the
/// archive's bounds; `unzip` (always present on macOS) reads the parts.
pub fn pptx_slides(bytes: &[u8]) -> Result<Vec<Vec<String>>, String> {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!(
        "sirus-pptx-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
    let result = (|| {
        let file = dir.join("deck.pptx");
        std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
        let listing = Command::new("/usr/bin/unzip")
            .arg("-Z1")
            .arg(&file)
            .output()
            .map_err(|e| e.to_string())?;
        let mut parts: Vec<(usize, String)> = String::from_utf8_lossy(&listing.stdout)
            .lines()
            .filter_map(|line| {
                let number = line
                    .strip_prefix("ppt/slides/slide")?
                    .strip_suffix(".xml")?
                    .parse()
                    .ok()?;
                Some((number, line.to_owned()))
            })
            .collect();
        parts.sort();
        if parts.is_empty() {
            return Err("no slides".to_owned());
        }
        parts
            .into_iter()
            .take(300)
            .map(|(_, part)| {
                let out = Command::new("/usr/bin/unzip")
                    .arg("-p")
                    .arg(&file)
                    .arg(&part)
                    .output()
                    .map_err(|e| e.to_string())?;
                Ok(slide_text(&String::from_utf8_lossy(&out.stdout)))
            })
            .collect()
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

// ---- The host's answer -----------------------------------------------------------------

/// What `attachment_preview` returned, decoded.
pub enum Preview {
    Pdf(Vec<u8>),
    Word(Vec<WordBlock>),
    Sheets(Vec<Sheet>),
    Office { bytes: Vec<u8> },
}

pub fn parse_preview(value: &Value) -> Result<Preview, String> {
    let bytes = |key: &str| {
        STANDARD
            .decode(value[key].as_str().unwrap_or(""))
            .map_err(|_| "invalid".to_owned())
    };
    match value["type"].as_str() {
        Some("pdf") => Ok(Preview::Pdf(bytes("data")?)),
        Some("office") => Ok(Preview::Office {
            bytes: bytes("data")?,
        }),
        Some("word") => Ok(Preview::Word(
            value["blocks"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|block| match block["type"].as_str() {
                    Some("table") => WordBlock::Table {
                        rows: block["rows"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|row| {
                                row.as_array()
                                    .into_iter()
                                    .flatten()
                                    .map(|c| c.as_str().unwrap_or("").to_owned())
                                    .collect()
                            })
                            .collect(),
                    },
                    _ => WordBlock::Paragraph {
                        text: block["text"].as_str().unwrap_or("").to_owned(),
                        heading: block["heading"].as_u64().unwrap_or(0) as u8,
                        list: block["list"].as_bool().unwrap_or(false),
                    },
                })
                .collect(),
        )),
        Some("spreadsheet") => Ok(Preview::Sheets(
            value["sheets"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|sheet| {
                    let mut out = Sheet {
                        name: sheet["name"].as_str().unwrap_or("").to_owned(),
                        columns: sheet["columns"].as_u64().unwrap_or(0) as usize,
                        ..Sheet::default()
                    };
                    let count = sheet["rows"].as_u64().unwrap_or(0) as usize;
                    out.rows = vec![Vec::new(); count.min(100_000)];
                    for cell in sheet["cells"].as_array().into_iter().flatten() {
                        let (row, column) = (
                            cell["row"].as_u64().unwrap_or(0) as usize,
                            cell["column"].as_u64().unwrap_or(0) as usize,
                        );
                        let Some(line) = out.rows.get_mut(row) else {
                            continue;
                        };
                        if line.len() <= column {
                            line.resize(column + 1, String::new());
                        }
                        line[column] = cell["value"].as_str().unwrap_or("").to_owned();
                        if let Some(formula) = cell["formula"].as_str() {
                            out.formulas.insert((row, column), formula.to_owned());
                        }
                        out.columns = out.columns.max(column + 1);
                    }
                    out.measure();
                    out
                })
                .collect(),
        )),
        _ => Err("invalid".to_owned()),
    }
}

/// The message for a host error (`code`), as Sirus's `documentError`.
pub fn error_message(error: &Value) -> &'static str {
    use crate::i18n::t;
    match error["code"].as_str().unwrap_or("") {
        "document_limit" => {
            t("Este arquivo excede os limites de conteúdo do leitor. Use um documento menor.")
        }
        "document_invalid" => {
            t("O arquivo está danificado, protegido por senha ou usa um formato não suportado.")
        }
        "document_unsupported" => {
            t("Use PDF, DOCX, XLSX, PPTX ou CSV. Converta arquivos antigos DOC/XLS/PPT primeiro.")
        }
        "document_busy" => t("Outro documento está carregando. Tente novamente em instantes."),
        "document_unavailable" => t("Este anexo não está mais disponível. Anexe novamente."),
        _ => t("Não foi possível abrir este documento."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sizes_read_like_sirus_code() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(6 * 1024), "6 KB");
        assert_eq!(format_size(6 * 1024 + 700), "7 KB");
        assert_eq!(format_size(3 * 1024 * 1024 / 2), "1.5 MB");
    }

    #[test]
    fn columns_are_lettered() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(701), "ZZ");
        assert_eq!(column_name(702), "AAA");
    }

    #[test]
    fn files_get_their_viewer() {
        let of = |name, mime| classify(name, mime, false, false);
        assert_eq!(of("a.PDF", ""), Viewer::Pdf);
        assert_eq!(of("a", "application/pdf"), Viewer::Pdf);
        assert_eq!(of("a.docx", ""), Viewer::Docx);
        assert_eq!(of("a.xlsx", ""), Viewer::Xlsx);
        assert_eq!(of("a.csv", "text/csv"), Viewer::Csv);
        assert_eq!(of("a.tsv", ""), Viewer::Tsv);
        assert_eq!(of("a.pptx", ""), Viewer::Pptx);
        assert_eq!(of("a.doc", ""), Viewer::Legacy);
        assert_eq!(of("shot.png", "image/png"), Viewer::Image);
        assert_eq!(of("page.html", "text/html"), Viewer::Html);
        assert_eq!(of("a.bin", "application/octet-stream"), Viewer::None);
        assert_eq!(classify("a.rs", "text/plain", false, true), Viewer::Text);
        assert_eq!(classify("src", "", true, false), Viewer::None);
    }

    #[test]
    fn delimited_text_keeps_quotes_and_line_breaks() {
        let sheet = parse_delimited(
            "a,b\n\"x, y\",\"he said \"\"hi\"\"\"\n\"two\nlines\",3\n",
            ',',
        );
        assert_eq!(sheet.rows.len(), 3);
        assert_eq!(sheet.rows[1], vec!["x, y", "he said \"hi\""]);
        assert_eq!(sheet.rows[2][0], "two\nlines");
        assert_eq!(sheet.columns, 2);
        let tabs = parse_delimited("\u{feff}a\tb\r\n1\t2", '\t');
        assert_eq!(tabs.rows, vec![vec!["a", "b"], vec!["1", "2"]]);
    }

    #[test]
    fn slides_give_their_paragraphs() {
        let xml = r#"<p:sld><a:p><a:r><a:t>Title &amp; more</a:t></a:r></a:p><a:p><a:tab/><a:r><a:t xml:space="preserve">Hello </a:t></a:r><a:r><a:t>world</a:t></a:r></a:p><a:p></a:p></p:sld>"#;
        assert_eq!(slide_text(xml), vec!["Title & more", "Hello world"]);
    }

    #[test]
    fn entities_unescape() {
        assert_eq!(
            unescape("a &lt; b &#65;&#x42; &unknown; &"),
            "a < b AB &unknown; &"
        );
    }

    #[test]
    fn a_deck_made_by_zip_is_read_in_order() {
        if !std::path::Path::new("/usr/bin/zip").exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("sirus-deck-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("ppt/slides")).unwrap();
        for (n, text) in [(2, "Second"), (10, "Tenth"), (1, "First")] {
            std::fs::write(
                dir.join(format!("ppt/slides/slide{n}.xml")),
                format!("<a:p><a:r><a:t>{text}</a:t></a:r></a:p>"),
            )
            .unwrap();
        }
        let out = dir.join("deck.pptx");
        let status = std::process::Command::new("/usr/bin/zip")
            .current_dir(&dir)
            .args(["-qr", "deck.pptx", "ppt"])
            .status()
            .unwrap();
        assert!(status.success());
        let slides = pptx_slides(&std::fs::read(&out).unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(slides, vec![vec!["First"], vec!["Second"], vec!["Tenth"]]);
        assert!(pptx_slides(b"not a zip").is_err());
    }

    #[test]
    fn the_hosts_documents_decode() {
        let pdf = parse_preview(&json!({ "type": "pdf", "data": STANDARD.encode(b"%PDF-1.4") }));
        assert!(matches!(pdf, Ok(Preview::Pdf(b)) if b == b"%PDF-1.4"));
        let word = parse_preview(&json!({ "type": "word", "blocks": [
            { "type": "paragraph", "text": "Title", "heading": 1, "list": false },
            { "type": "table", "rows": [["a", "b"]] }
        ] }));
        let Ok(Preview::Word(blocks)) = word else {
            panic!("word")
        };
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[1],
            WordBlock::Table {
                rows: vec![vec!["a".into(), "b".into()]]
            }
        );
        let sheets = parse_preview(&json!({ "type": "spreadsheet", "sheets": [{
            "name": "Plan1", "rows": 3, "columns": 2,
            "cells": [
                { "row": 0, "column": 0, "value": "x", "formula": null },
                { "row": 2, "column": 1, "value": "42", "formula": "A1+1" }
            ] }] }));
        let Ok(Preview::Sheets(sheets)) = sheets else {
            panic!("sheets")
        };
        assert_eq!(sheets[0].value(2, 1), "42");
        assert_eq!(sheets[0].value(1, 0), "");
        assert_eq!(sheets[0].formulas[&(2, 1)], "A1+1");
        assert_eq!(sheets[0].widths.len(), 2);
        assert!(parse_preview(&json!({ "type": "pdf", "data": "***" })).is_err());
        assert!(parse_preview(&json!({ "type": "other" })).is_err());
    }

    #[test]
    fn the_gallery_holds_only_pictures_in_order() {
        let file = |id: &str, name: &str| Info {
            id: id.into(),
            name: name.into(),
            kind: "file".into(),
            mime: String::new(),
            size: None,
            content: String::new(),
            truncated: false,
            thumb: None,
        };
        let all = vec![
            file("1", "a.png"),
            file("2", "notes.pdf"),
            file("3", "b.JPG"),
            file("4", "c.webp"),
        ];
        let pictures = images_of(&all);
        let ids: Vec<&str> = pictures.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["1", "3", "4"]);
        assert_eq!(gallery_step(&pictures, "3", 1), Some(2));
        assert_eq!(gallery_step(&pictures, "3", -1), Some(0));
        assert_eq!(gallery_step(&pictures, "1", -1), None);
        assert_eq!(gallery_step(&pictures, "4", 1), None);
        assert_eq!(gallery_step(&pictures, "2", 1), None);
    }

    #[test]
    fn replies_become_info() {
        let info = Info::from_json(&json!({ "id": "a1", "name": "r.pdf", "kind": "file",
            "content": "", "truncated": false, "mimeType": "application/pdf", "size": 6144 }));
        assert_eq!(info.viewer(), Viewer::Pdf);
        assert!(info.openable());
        assert_eq!(info.size, Some(6144));
        let folder = Info::from_json(&json!({ "id": "f", "name": "src", "kind": "folder" }));
        assert!(!folder.openable());
    }

    /// Against the running host (`cargo test live_host -- --ignored --nocapture`): attaches the
    /// files in $ATTACH_FILES (comma separated) to $ATTACH_SESSION, previews each, releases them.
    #[test]
    #[ignore]
    fn live_host() {
        use tungstenite::Message;
        let dir = format!(
            "{}/Library/Application Support/com.siruscode.native",
            std::env::var("HOME").unwrap()
        );
        let token = std::fs::read_to_string(format!("{dir}/native-token")).unwrap();
        let (mut socket, _) = tungstenite::connect("ws://127.0.0.1:7811/api/socket").unwrap();
        socket
            .send(Message::text(
                json!({ "type": "hello", "token": token.trim() }).to_string(),
            ))
            .unwrap();
        let mut next = 0;
        let mut call = |socket: &mut tungstenite::WebSocket<_>,
                        cmd: &str,
                        args: Value|
         -> Result<Value, Value> {
            next += 1;
            socket
                .send(Message::text(
                    json!({ "type": "invoke", "id": next, "cmd": cmd, "args": args }).to_string(),
                ))
                .unwrap();
            loop {
                let Message::Text(text) = socket.read().unwrap() else {
                    continue;
                };
                let mut message: Value = serde_json::from_str(&text).unwrap();
                if message["type"] == "result" && message["id"] == next {
                    return if message["ok"].as_bool().unwrap_or(false) {
                        Ok(message["value"].take())
                    } else {
                        Err(message["error"].take())
                    };
                }
            }
        };
        let owner = format!("session:{}", std::env::var("ATTACH_SESSION").unwrap());
        for path in std::env::var("ATTACH_FILES").unwrap().split(',') {
            let started = std::time::Instant::now();
            let data = std::fs::read(path).unwrap();
            let name = path.rsplit('/').next().unwrap();
            let reply = call(
                &mut socket,
                "paste_prompt_attachments",
                json!({ "owner": owner, "files": [{ "name": name, "data": STANDARD.encode(&data) }] }),
            );
            println!(
                "{name}: attach {:?} -> {}",
                started.elapsed(),
                reply
                    .as_ref()
                    .map(|v| v.to_string().chars().take(300).collect::<String>())
                    .unwrap_or_else(|e| e.to_string())
            );
            let Ok(reply) = reply else { continue };
            let info = Info::from_json(&reply[0]);
            println!("  viewer {:?} size {:?}", info.viewer(), info.size);
            let plain = matches!(info.viewer(), Viewer::Docx | Viewer::Xlsx | Viewer::Csv);
            let started = std::time::Instant::now();
            match info.viewer() {
                Viewer::Image => {
                    let image = call(
                        &mut socket,
                        "attachment_image",
                        json!({ "owner": owner, "id": info.id }),
                    );
                    println!(
                        "  image {:?}",
                        image.map(|v| v
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(40)
                            .collect::<String>())
                    );
                }
                _ => {
                    let reply = call(
                        &mut socket,
                        "attachment_preview",
                        json!({ "owner": owner, "id": info.id, "plain": plain }),
                    );
                    match reply {
                        Ok(value) => match parse_preview(&value) {
                            Ok(Preview::Pdf(b)) => {
                                println!("  pdf {} bytes in {:?}", b.len(), started.elapsed())
                            }
                            Ok(Preview::Word(b)) => println!(
                                "  word {} blocks: {:?}",
                                b.len(),
                                b.iter().take(3).collect::<Vec<_>>()
                            ),
                            Ok(Preview::Sheets(s)) => println!(
                                "  sheets {:?}",
                                s.iter()
                                    .map(|s| (
                                        s.name.clone(),
                                        s.rows.len(),
                                        s.columns,
                                        s.value(0, 0).to_owned()
                                    ))
                                    .collect::<Vec<_>>()
                            ),
                            Ok(Preview::Office { bytes }) => println!(
                                "  office {} bytes, slides {:?}",
                                bytes.len(),
                                pptx_slides(&bytes)
                            ),
                            Err(e) => println!(
                                "  parse error {e}: {}",
                                value.to_string().chars().take(200).collect::<String>()
                            ),
                        },
                        Err(error) => {
                            println!("  preview error {error} -> {}", error_message(&error))
                        }
                    }
                }
            }
            let released = call(
                &mut socket,
                "release_prompt_attachments",
                json!({ "owner": owner, "ids": [info.id] }),
            );
            println!("  released {released:?}");
        }
    }
}
