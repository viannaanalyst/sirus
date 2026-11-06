//! Read-only interpretation of admitted private snapshots. No extraction, URLs or converters.
use crate::{
    attachments::PreparedAttachment,
    error::{Error, Result},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use roxmltree::{Document, Node};
use serde::Serialize;
use std::{
    io::{Cursor, Read},
    sync::Arc,
};

const MAX_PART: u64 = 8 * 1024 * 1024;
const MAX_INFLATED: u64 = 32 * 1024 * 1024;
const MAX_TEXT: usize = 2 * 1024 * 1024;
const MAX_ROWS: usize = 5000;
const MAX_COLS: usize = 128;
const MAX_CELLS: usize = 20_000;
static READER_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DocumentPreview {
    Pdf {
        data: String,
    },
    Word {
        blocks: Vec<WordBlock>,
    },
    Spreadsheet {
        sheets: Vec<Sheet>,
    },
    /// A DOCX, XLSX or PPTX that passed the archive preflight. The front end draws it with
    /// the lazily loaded Office engines (ADR-184); `plain` requests fall back to the typed
    /// content views above.
    Office {
        format: String,
        data: String,
    },
}
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
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
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sheet {
    pub name: String,
    pub rows: usize,
    pub columns: usize,
    pub cells: Vec<Cell>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    pub row: usize,
    pub column: usize,
    pub value: String,
    pub formula: Option<String>,
}

fn invalid() -> Error {
    Error::new(
        "document_invalid",
        "This document is damaged, encrypted or unsupported.",
    )
}
fn limit() -> Error {
    Error::new(
        "document_limit",
        "This document exceeds the reader's content limits.",
    )
}
fn xml(text: &str) -> Result<Document<'_>> {
    Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 150_000,
            ..Default::default()
        },
    )
    .map_err(|_| invalid())
}
fn named(node: Node<'_, '_>, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name
}
fn attr<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == name)
        .map(|a| a.value())
}
fn text_nodes(node: Node<'_, '_>, name: &str) -> String {
    node.descendants()
        .filter(|n| named(*n, name))
        .filter_map(|n| n.text())
        .collect()
}
fn paragraph(node: Node<'_, '_>) -> String {
    let mut text = String::new();
    for child in node.descendants() {
        if named(child, "t") {
            text.push_str(child.text().unwrap_or_default());
        } else if named(child, "br") || named(child, "cr") {
            text.push('\n');
        } else if named(child, "tab") {
            text.push('\t');
        }
    }
    text
}

struct Office {
    archive: zip::ZipArchive<Cursor<Vec<u8>>>,
}
impl Office {
    fn new(bytes: Vec<u8>) -> Result<Self> {
        // Bound the central directory before ZipArchive allocates its entry map.
        // ZIP64 and multi-disk archives are unnecessary for <=10 MiB attachments.
        let tail = bytes.len().saturating_sub(65_557);
        let eocd = bytes[tail..]
            .windows(4)
            .rposition(|w| w == b"PK\x05\x06")
            .map(|p| tail + p)
            .ok_or_else(invalid)?;
        let end = bytes.get(eocd..eocd + 22).ok_or_else(invalid)?;
        let comment = u16::from_le_bytes([end[20], end[21]]) as usize;
        let directory_size = u32::from_le_bytes(end[12..16].try_into().unwrap()) as usize;
        let directory_offset = u32::from_le_bytes(end[16..20].try_into().unwrap()) as usize;
        if eocd + 22 + comment != bytes.len()
            || directory_size > 512 * 1024
            || directory_offset.checked_add(directory_size) != Some(eocd)
            || (eocd >= 20 && bytes.get(eocd - 20..eocd - 16) == Some(b"PK\x06\x07"))
        {
            return Err(invalid());
        }
        let count = u16::from_le_bytes([end[10], end[11]]) as usize;
        if count > 512 {
            return Err(limit());
        }
        if end[4..8] != [0, 0, 0, 0] || end[8..10] != end[10..12] {
            return Err(invalid());
        }
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid())?;
        if archive.len() > 512 {
            return Err(limit());
        }
        let mut inflated = 0u64;
        let mut names = std::collections::HashSet::new();
        for i in 0..archive.len() {
            let entry = archive.by_index(i).map_err(|_| invalid())?;
            inflated = inflated.checked_add(entry.size()).ok_or_else(limit)?;
            if entry.size() > MAX_PART || inflated > MAX_INFLATED || entry.name().len() > 512 {
                return Err(limit());
            }
            if !names.insert(entry.name().to_owned()) || entry.enclosed_name().is_none() {
                return Err(invalid());
            }
        }
        Ok(Self { archive })
    }
    fn part(&mut self, path: &str) -> Result<String> {
        let entry = self.archive.by_name(path).map_err(|_| invalid())?;
        let mut bytes = Vec::new();
        entry
            .take(MAX_PART + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid())?;
        if bytes.len() as u64 > MAX_PART {
            return Err(limit());
        }
        String::from_utf8(bytes).map_err(|_| invalid())
    }
    fn has(&self, path: &str) -> bool {
        self.archive.index_for_name(path).is_some()
    }
}

fn word(mut office: Office) -> Result<DocumentPreview> {
    let part = office.part("word/document.xml")?;
    let doc = xml(&part)?;
    let body = doc
        .descendants()
        .find(|n| named(*n, "body"))
        .ok_or_else(invalid)?;
    let mut blocks = Vec::new();
    let mut text_size = 0;
    let mut cell_count = 0;
    for node in body.children().filter(Node::is_element) {
        if named(node, "p") {
            let text = paragraph(node);
            text_size += text.len();
            let style = node
                .descendants()
                .find(|n| named(*n, "pStyle"))
                .and_then(|n| attr(n, "val"))
                .unwrap_or_default()
                .to_ascii_lowercase();
            let heading = style
                .strip_prefix("heading")
                .and_then(|s| s.parse::<u8>().ok())
                .filter(|n| (1..=6).contains(n))
                .unwrap_or(0);
            let list = node.descendants().any(|n| named(n, "numPr"));
            blocks.push(WordBlock::Paragraph {
                text,
                heading,
                list,
            });
        } else if named(node, "tbl") {
            let mut rows = Vec::new();
            for row in node.children().filter(|n| named(*n, "tr")) {
                let mut cells = Vec::new();
                for cell in row.children().filter(|n| named(*n, "tc")) {
                    let text = cell
                        .children()
                        .filter(|n| named(*n, "p"))
                        .map(paragraph)
                        .collect::<Vec<_>>()
                        .join("\n");
                    text_size += text.len();
                    cells.push(text);
                    cell_count += 1;
                    if cell_count > MAX_CELLS || text_size > MAX_TEXT {
                        return Err(limit());
                    }
                    if cells.len() > MAX_COLS {
                        return Err(limit());
                    }
                }
                rows.push(cells);
                if rows.len() > MAX_ROWS {
                    return Err(limit());
                }
            }
            blocks.push(WordBlock::Table { rows });
        }
        if text_size > MAX_TEXT || blocks.len() > 4000 {
            return Err(limit());
        }
    }
    Ok(DocumentPreview::Word { blocks })
}

fn coordinate(reference: &str) -> Result<(usize, usize)> {
    let split = reference
        .find(|c: char| c.is_ascii_digit())
        .ok_or_else(invalid)?;
    let (letters, digits) = reference.split_at(split);
    if letters.is_empty() || !letters.bytes().all(|c| c.is_ascii_uppercase()) {
        return Err(invalid());
    }
    let column = letters
        .bytes()
        .try_fold(0usize, |v, c| {
            v.checked_mul(26)?.checked_add((c - b'A' + 1) as usize)
        })
        .ok_or_else(limit)?;
    let row = digits.parse::<usize>().map_err(|_| invalid())?;
    if row == 0 || column == 0 {
        return Err(invalid());
    }
    if row > MAX_ROWS || column > MAX_COLS {
        return Err(limit());
    }
    Ok((row - 1, column - 1))
}
fn sheet_path(target: &str) -> Result<String> {
    let path = if let Some(path) = target.strip_prefix("/xl/") {
        format!("xl/{path}")
    } else {
        format!("xl/{target}")
    };
    if !path.starts_with("xl/worksheets/")
        || !path.ends_with(".xml")
        || path.contains('\\')
        || path
            .split('/')
            .any(|p| p == ".." || p == "." || p.is_empty())
    {
        return Err(invalid());
    }
    Ok(path)
}
fn spreadsheet(mut office: Office) -> Result<DocumentPreview> {
    let mut strings = Vec::new();
    let mut text_size = 0;
    if office.has("xl/sharedStrings.xml") {
        let part = office.part("xl/sharedStrings.xml")?;
        for node in xml(&part)?.descendants().filter(|n| named(*n, "si")) {
            let text = text_nodes(node, "t");
            text_size += text.len();
            strings.push(text);
            if strings.len() > MAX_CELLS || text_size > MAX_TEXT {
                return Err(limit());
            }
        }
    }
    let workbook = office.part("xl/workbook.xml")?;
    let relationships = office.part("xl/_rels/workbook.xml.rels")?;
    let rels = xml(&relationships)?;
    let doc = xml(&workbook)?;
    let mut sheets = Vec::new();
    let mut count = 0;
    for item in doc.descendants().filter(|n| named(*n, "sheet")) {
        if sheets.len() >= 32 {
            return Err(limit());
        }
        let id = attr(item, "id").ok_or_else(invalid)?;
        let rel = rels
            .descendants()
            .find(|n| named(*n, "Relationship") && attr(*n, "Id") == Some(id))
            .ok_or_else(invalid)?;
        if attr(rel, "TargetMode") == Some("External")
            || !attr(rel, "Type").is_some_and(|t| t.ends_with("/worksheet"))
        {
            return Err(invalid());
        }
        let path = sheet_path(attr(rel, "Target").ok_or_else(invalid)?)?;
        let part = office.part(&path)?;
        let sheet_doc = xml(&part)?;
        let mut sheet = Sheet {
            name: attr(item, "name").unwrap_or("Sheet").to_owned(),
            rows: 0,
            columns: 0,
            cells: Vec::new(),
        };
        if sheet.name.len() > 256 {
            return Err(limit());
        }
        let mut seen = std::collections::HashSet::new();
        for cell in sheet_doc.descendants().filter(|n| named(*n, "c")) {
            let (row, column) = coordinate(attr(cell, "r").ok_or_else(invalid)?)?;
            if !seen.insert((row, column)) {
                return Err(invalid());
            }
            let raw = cell
                .children()
                .find(|n| named(*n, "v"))
                .and_then(|n| n.text())
                .unwrap_or_default();
            let value = match attr(cell, "t").unwrap_or_default() {
                "s" => strings
                    .get(raw.parse::<usize>().map_err(|_| invalid())?)
                    .ok_or_else(invalid)?
                    .clone(),
                "inlineStr" => text_nodes(cell, "t"),
                "b" => {
                    if raw == "1" {
                        "TRUE".into()
                    } else {
                        "FALSE".into()
                    }
                }
                _ => raw.to_owned(),
            };
            let formula = cell
                .children()
                .find(|n| named(*n, "f"))
                .and_then(|n| n.text())
                .map(str::to_owned);
            text_size += value.len() + formula.as_ref().map_or(0, String::len);
            count += 1;
            if text_size > MAX_TEXT || count > MAX_CELLS {
                return Err(limit());
            }
            sheet.rows = sheet.rows.max(row + 1);
            sheet.columns = sheet.columns.max(column + 1);
            sheet.cells.push(Cell {
                row,
                column,
                value,
                formula,
            });
        }
        sheets.push(sheet);
    }
    if sheets.is_empty() {
        return Err(invalid());
    }
    Ok(DocumentPreview::Spreadsheet { sheets })
}
/// Windows-1252's characters for 0x80..=0x9F (the rest of the byte range is Latin-1); unused
/// codes keep the C1 control.
const CP1252_HIGH: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
    '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
];
/// CSV text as UTF-8, or else as Windows-1252: spreadsheets and Brazilian bank and ERP exports
/// often save Latin-1.
fn csv_text(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.into(),
        Err(_) => bytes
            .iter()
            .map(|&byte| match byte {
                0x80..=0x9f => CP1252_HIGH[(byte - 0x80) as usize],
                _ => byte as char,
            })
            .collect::<String>()
            .into(),
    }
}
fn csv_preview(bytes: &[u8]) -> Result<DocumentPreview> {
    if bytes.len() > MAX_TEXT {
        return Err(limit());
    }
    let decoded = csv_text(bytes);
    let text = decoded.trim_start_matches('\u{feff}');
    if text.len() > MAX_TEXT {
        return Err(limit());
    }
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    let delimiter = if first.contains(';') && !first.contains(',') {
        b';'
    } else {
        b','
    };
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes());
    let mut sheet = Sheet {
        name: "CSV".into(),
        rows: 0,
        columns: 0,
        cells: Vec::new(),
    };
    for (row, record) in reader.records().enumerate() {
        let record = record.map_err(|_| invalid())?;
        if row >= MAX_ROWS
            || record.len() > MAX_COLS
            || sheet.cells.len() + record.len() > MAX_CELLS
        {
            return Err(limit());
        }
        sheet.rows = row + 1;
        sheet.columns = sheet.columns.max(record.len());
        for (column, value) in record.iter().enumerate() {
            sheet.cells.push(Cell {
                row,
                column,
                value: value.into(),
                formula: None,
            });
        }
    }
    Ok(DocumentPreview::Spreadsheet {
        sheets: vec![sheet],
    })
}
fn parse(file: &PreparedAttachment, plain: bool) -> Result<DocumentPreview> {
    parse_bytes(file.bytes()?, &file.view.name, plain)
}

fn parse_bytes(bytes: Vec<u8>, name: &str, plain: bool) -> Result<DocumentPreview> {
    let extension = name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if bytes.starts_with(b"%PDF-") {
        return Ok(DocumentPreview::Pdf {
            data: STANDARD.encode(bytes),
        });
    }
    match extension.as_str() {
        "docx" | "xlsx" | "pptx" if !plain => {
            // The archive bounds apply before the bytes reach the renderer.
            let office = Office::new(bytes.clone())?;
            drop(office);
            Ok(DocumentPreview::Office {
                format: extension,
                data: STANDARD.encode(bytes),
            })
        }
        "docx" => word(Office::new(bytes)?),
        "xlsx" => spreadsheet(Office::new(bytes)?),
        "csv" => csv_preview(&bytes),
        _ => Err(Error::new(
            "document_unsupported",
            "Use PDF, DOCX, XLSX, PPTX or CSV. Convert legacy DOC/XLS/PPT files first.",
        )),
    }
}

pub async fn attachment_preview(
    state: crate::runtime::State<'_, Arc<crate::commands::AppState>>,
    owner: String,
    id: String,
    plain: Option<bool>,
) -> Result<DocumentPreview> {
    let permit = READER_GATE
        .try_acquire()
        .map_err(|_| Error::new("document_busy", "Another document is loading. Try again."))?;
    state.ensure_running()?;
    let snapshot = {
        let data = state.data.lock();
        if !crate::attachments::owner_exists(&data, &owner) {
            return Err(Error::new(
                "document_unavailable",
                "Attachment owner no longer exists.",
            ));
        }
        state.attachments.preview_snapshot(&owner, &id)?
    };
    let reading = snapshot.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        parse(&reading, plain.unwrap_or(false))
    })
    .await
    .map_err(|_| invalid())??;
    state.ensure_running()?;
    let data = state.data.lock();
    if !crate::attachments::owner_exists(&data, &owner)
        || !Arc::ptr_eq(&snapshot, &state.attachments.preview_snapshot(&owner, &id)?)
    {
        return Err(Error::new(
            "document_unavailable",
            "Attachment is no longer available.",
        ));
    }
    Ok(result)
}

/// A document inside a session's workspace (a project file or one an agent wrote), read with
/// the same 10 MiB bound, regular-file rule and archive preflight as an attachment snapshot.
pub async fn workspace_document(
    state: crate::runtime::State<'_, Arc<crate::commands::AppState>>,
    session_id: String,
    path: String,
    plain: Option<bool>,
) -> Result<DocumentPreview> {
    let permit = READER_GATE
        .try_acquire()
        .map_err(|_| Error::new("document_busy", "Another document is loading. Try again."))?;
    state.ensure_running()?;
    let state = state.inner().clone();
    let plain = plain.unwrap_or(false);
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let cwd = crate::commands::session_path(&state, &session_id)?;
        if crate::editor::is_reserved_path(std::path::Path::new(&path)) {
            return Err(Error::invalid_path("cannot open reserved paths"));
        }
        let file = crate::paths::open_regular_within(&cwd, std::path::Path::new(&path))?;
        let limit_bytes = crate::attachments::MAX_FILE_BYTES as u64;
        if file.metadata()?.len() > limit_bytes {
            return Err(limit());
        }
        let mut bytes = Vec::new();
        file.take(limit_bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid())?;
        if bytes.len() as u64 > limit_bytes {
            return Err(limit());
        }
        parse_bytes(bytes, &path, plain)
    })
    .await
    .map_err(|_| invalid())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn archive(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in parts {
            writer
                .start_file(
                    *name,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
    fn excel(sheet: &str) -> Office {
        Office::new(archive(&[
            ("xl/workbook.xml", r#"<workbook xmlns:r="urn:rel"><sheets><sheet name="Resumo" r:id="r1"/><sheet name="Vazio" r:id="r2"/></sheets></workbook>"#),
            ("xl/_rels/workbook.xml.rels", r#"<Relationships><Relationship Id="r1" Type="urn:office/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="r2" Type="urn:office/worksheet" Target="/xl/worksheets/sheet2.xml"/></Relationships>"#),
            ("xl/sharedStrings.xml", "<sst><si><r><t>Olá </t></r><r><t>mundo</t></r></si></sst>"),
            ("xl/worksheets/sheet1.xml",sheet),
            ("xl/worksheets/sheet2.xml","<worksheet><sheetData/></worksheet>"),
        ])).unwrap()
    }
    #[test]
    fn word_preserves_heading_paragraphs_lists_and_table_text_without_active_content() {
        let bytes = archive(&[(
            "word/document.xml",
            r#"<w:document xmlns:w="urn:word"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title &amp; text</w:t></w:r></w:p><w:p><w:pPr><w:numPr/></w:pPr><w:hyperlink><w:r><w:t>&lt;script&gt;data&lt;/script&gt;</w:t><w:br/><w:t>second line</w:t></w:r></w:hyperlink></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>Cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#,
        )]);
        let result = serde_json::to_value(word(Office::new(bytes).unwrap()).unwrap()).unwrap();
        assert_eq!(result["type"], "word");
        assert_eq!(result["blocks"][0]["heading"], 1);
        assert_eq!(result["blocks"][0]["text"], "Title & text");
        assert_eq!(
            result["blocks"][1]["text"],
            "<script>data</script>\nsecond line"
        );
        assert_eq!(result["blocks"][1]["list"], true);
        assert_eq!(result["blocks"][2]["rows"][0][0], "Cell");
    }
    #[test]
    fn sheets_keep_sparse_coordinates_shared_strings_inline_text_and_cached_formulas() {
        let result = serde_json::to_value(spreadsheet(excel(r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1"><f>SUM(C2:C3)</f><v>42</v></c></row><row r="3"><c r="B3" t="inlineStr"><is><t>hello</t></is></c><c r="D3" t="b"><v>1</v></c></row></sheetData></worksheet>"#)).unwrap()).unwrap();
        assert_eq!(result["sheets"][0]["rows"], 3);
        assert_eq!(result["sheets"][0]["columns"], 4);
        assert_eq!(result["sheets"][0]["cells"][0]["value"], "Olá mundo");
        assert_eq!(result["sheets"][0]["cells"][1]["formula"], "SUM(C2:C3)");
        assert_eq!(result["sheets"][0]["cells"][1]["value"], "42");
        assert_eq!(result["sheets"][0]["cells"][2]["row"], 2);
        assert_eq!(result["sheets"][0]["cells"][3]["value"], "TRUE");
        assert_eq!(result["sheets"][1]["name"], "Vazio");
        assert_eq!(result["sheets"][1]["rows"], 0);
    }
    #[test]
    fn sheet_coordinates_and_relationships_are_bounded() {
        assert_eq!(coordinate("DX5000").unwrap(), (4999, 127));
        for bad in [
            "A0",
            "A5001",
            "DY1",
            "A1048576",
            "A99999999999999999999999",
            "a1",
            "1",
            "A-1",
        ] {
            assert!(coordinate(bad).is_err(), "{bad}");
        }
        for bad in [
            "../outside.xml",
            "https://host/file.xml",
            "worksheets/../../outside.xml",
            "worksheets\\x.xml",
            "/outside.xml",
        ] {
            assert!(sheet_path(bad).is_err(), "{bad}");
        }
        assert_eq!(
            sheet_path("/xl/worksheets/sheet1.xml").unwrap(),
            "xl/worksheets/sheet1.xml"
        );
        assert!(spreadsheet(excel(r#"<worksheet><c r="A1"/><c r="A1"/></worksheet>"#)).is_err());
        assert!(spreadsheet(excel(
            r#"<worksheet><c r="A1" t="s"><v>99</v></c></worksheet>"#
        ))
        .is_err());
    }
    #[test]
    fn csv_preserves_quoted_commas_newlines_and_bom_and_never_evaluates_formulas() {
        let result = serde_json::to_value(
            csv_preview(
                "\u{feff}Name,Value\n\"with, comma\",\"line1\nline2\"\nformula,=RUN()".as_bytes(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(result["sheets"][0]["rows"], 3);
        assert_eq!(result["sheets"][0]["cells"][2]["value"], "with, comma");
        assert_eq!(result["sheets"][0]["cells"][3]["value"], "line1\nline2");
        assert_eq!(result["sheets"][0]["cells"][5]["value"], "=RUN()");
        assert!(result["sheets"][0]["cells"][5]["formula"].is_null());
        let latin =
            serde_json::to_value(csv_preview(b"\r\nEmiss\xe3o;Valor\r\nCaf\xe9;\x80 5").unwrap())
                .unwrap();
        assert_eq!(latin["sheets"][0]["cells"][0]["value"], "Emissão");
        assert_eq!(latin["sheets"][0]["cells"][2]["value"], "Café");
        assert_eq!(latin["sheets"][0]["cells"][3]["value"], "€ 5");
        assert!(csv_preview(&vec![b'a'; MAX_TEXT + 1]).is_err());
    }
    #[test]
    fn rejects_bad_archives_dtd_oversized_parts_and_entry_counts() {
        assert!(Office::new(b"not a zip".to_vec()).is_err());
        assert!(xml("<!DOCTYPE x [<!ENTITY y 'hi'>]><x>&y;</x>").is_err());
        assert!(xml("<a><b></a>").is_err());
        let huge = "a".repeat(MAX_PART as usize + 1);
        assert!(Office::new(archive(&[("word/document.xml", &huge)])).is_err());
        let names: Vec<_> = (0..513).map(|i| format!("file{i}")).collect();
        let parts: Vec<_> = names.iter().map(|n| (n.as_str(), "x")).collect();
        assert!(Office::new(archive(&parts)).is_err());
        assert!(Office::new(archive(&[("../escape", "x")])).is_err());
    }
    #[test]
    fn wide_and_long_csv_are_refused_before_returning_large_grids() {
        assert!(csv_preview("a,".repeat(128).as_bytes()).is_err());
        assert!(csv_preview("a\n".repeat(5001).as_bytes()).is_err());
    }
    #[test]
    fn office_files_pass_the_archive_preflight_and_plain_keeps_the_typed_views() {
        let deck = archive(&[("ppt/presentation.xml", "<p/>")]);
        let rich =
            serde_json::to_value(parse_bytes(deck.clone(), "Deck.PPTX", false).unwrap()).unwrap();
        assert_eq!(rich["type"], "office");
        assert_eq!(rich["format"], "pptx");
        assert!(
            parse_bytes(deck, "deck.pptx", true).is_err(),
            "no plain view for slides"
        );
        let sheet = archive(&[
            (
                "xl/workbook.xml",
                r#"<workbook xmlns:r="urn:rel"><sheets><sheet name="A" r:id="r1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships><Relationship Id="r1" Type="urn:office/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                "<worksheet><sheetData/></worksheet>",
            ),
        ]);
        assert_eq!(
            serde_json::to_value(parse_bytes(sheet.clone(), "a.xlsx", false).unwrap()).unwrap()
                ["type"],
            "office"
        );
        assert_eq!(
            serde_json::to_value(parse_bytes(sheet, "a.xlsx", true).unwrap()).unwrap()["type"],
            "spreadsheet"
        );
        assert!(parse_bytes(b"not a zip".to_vec(), "a.docx", false).is_err());
        assert!(parse_bytes(b"x".to_vec(), "old.ppt", false).is_err());
        let names: Vec<_> = (0..513).map(|i| format!("file{i}")).collect();
        let parts: Vec<_> = names.iter().map(|n| (n.as_str(), "x")).collect();
        assert!(parse_bytes(archive(&parts), "big.pptx", false).is_err());
    }
}
