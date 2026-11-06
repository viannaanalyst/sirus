//! The container of Word and PowerPoint files, for the attachment viewer: a read-only zip reader
//! (stored and deflated entries, with bounds against zip bombs) and the helpers every OOXML part
//! needs (relationship files, part paths, namespace-free XML lookups). Pure and tested; no UI.
use miniz_oxide::inflate::decompress_to_vec_with_limit;
use roxmltree::{Document, Node};
use std::cell::Cell;
use std::collections::HashMap;

/// At most this many entries in an archive.
const MAX_ENTRIES: usize = 20_000;
/// The largest part that is inflated.
const MAX_PART: usize = 64 * 1024 * 1024;
/// Everything one archive may inflate, in total, over all reads.
const MAX_TOTAL: u64 = 256 * 1024 * 1024;

struct Entry {
    name: String,
    method: u16,
    compressed: usize,
    size: usize,
    offset: usize,
}

pub struct Zip<'a> {
    data: &'a [u8],
    entries: Vec<Entry>,
    spent: Cell<u64>,
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

impl<'a> Zip<'a> {
    pub fn open(data: &'a [u8]) -> Result<Zip<'a>, String> {
        let bad = || "not a zip archive".to_owned();
        // The end-of-central-directory record is within the last 64 KB.
        let from = data.len().saturating_sub(22 + 65_535);
        let end = (from..=data.len().saturating_sub(22))
            .rev()
            .find(|&i| u32_at(data, i) == Some(0x0605_4b50))
            .ok_or_else(bad)?;
        let count = u16_at(data, end + 10).ok_or_else(bad)? as usize;
        let mut at = u32_at(data, end + 16).ok_or_else(bad)? as usize;
        if count > MAX_ENTRIES {
            return Err("too many entries".to_owned());
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if u32_at(data, at) != Some(0x0201_4b50) {
                return Err(bad());
            }
            let name_len = u16_at(data, at + 28).ok_or_else(bad)? as usize;
            let extra = u16_at(data, at + 30).ok_or_else(bad)? as usize;
            let comment = u16_at(data, at + 32).ok_or_else(bad)? as usize;
            let name = data.get(at + 46..at + 46 + name_len).ok_or_else(bad)?;
            entries.push(Entry {
                name: String::from_utf8_lossy(name)
                    .trim_start_matches('/')
                    .to_owned(),
                method: u16_at(data, at + 10).ok_or_else(bad)?,
                compressed: u32_at(data, at + 20).ok_or_else(bad)? as usize,
                size: u32_at(data, at + 24).ok_or_else(bad)? as usize,
                offset: u32_at(data, at + 42).ok_or_else(bad)? as usize,
            });
            at += 46 + name_len + extra + comment;
        }
        Ok(Zip {
            data,
            entries,
            spent: Cell::new(0),
        })
    }

    /// The part's bytes, or None when it is missing, damaged or past the bounds.
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let entry = self.entries.iter().find(|e| e.name == name)?;
        if entry.size > MAX_PART || self.spent.get() + entry.size as u64 > MAX_TOTAL {
            return None;
        }
        let local = entry.offset;
        if u32_at(self.data, local)? != 0x0403_4b50 {
            return None;
        }
        let name_len = u16_at(self.data, local + 26)? as usize;
        let extra = u16_at(self.data, local + 28)? as usize;
        let start = local + 30 + name_len + extra;
        let raw = self.data.get(start..start.checked_add(entry.compressed)?)?;
        let out = match entry.method {
            0 => raw.to_vec(),
            8 => decompress_to_vec_with_limit(raw, entry.size.max(1)).ok()?,
            _ => return None,
        };
        self.spent.set(self.spent.get() + out.len() as u64);
        Some(out)
    }

    pub fn read_text(&self, name: &str) -> Option<String> {
        String::from_utf8(self.read(name)?).ok()
    }
}

/// `target` (as written in a relationship of the part in `base_dir`) as an archive path.
pub fn resolve(base_dir: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_owned();
    }
    let mut parts: Vec<&str> = base_dir.split('/').filter(|p| !p.is_empty()).collect();
    for piece in target.split('/') {
        match piece {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// One relationship of a part.
#[derive(Clone, Debug)]
pub struct Rel {
    /// The archive path, or the address when the target is external.
    pub target: String,
    pub external: bool,
    /// The last path segment of the relationship's type (`image`, `slide`, `hyperlink`...).
    pub kind: String,
}

/// The relationships of `part`, by id.
pub fn rels(zip: &Zip, part: &str) -> HashMap<String, Rel> {
    let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
    let path = if dir.is_empty() {
        format!("_rels/{file}.rels")
    } else {
        format!("{dir}/_rels/{file}.rels")
    };
    let mut out = HashMap::new();
    let Some(text) = zip.read_text(&path) else {
        return out;
    };
    let Ok(doc) = parse(&text) else {
        return out;
    };
    for node in doc.descendants().filter(|n| n.has_tag_name("Relationship")) {
        let (Some(id), Some(target)) = (node.attribute("Id"), node.attribute("Target")) else {
            continue;
        };
        let external = node.attribute("TargetMode") == Some("External");
        out.insert(
            id.to_owned(),
            Rel {
                target: if external {
                    target.to_owned()
                } else {
                    resolve(dir, target)
                },
                external,
                kind: node
                    .attribute("Type")
                    .and_then(|t| t.rsplit('/').next())
                    .unwrap_or("")
                    .to_owned(),
            },
        );
    }
    out
}

/// Parses XML with bounds on size and depth (no DTD entities are expanded by roxmltree).
pub fn parse(text: &str) -> Result<Document<'_>, String> {
    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: 4_000_000,
        ..Default::default()
    };
    Document::parse_with_options(text, options).map_err(|e| e.to_string())
}

// ---- Namespace-free lookups ---------------------------------------------------------------

/// The first child element called `name` (local name).
pub fn child<'a, 'b>(node: Node<'a, 'b>, name: &str) -> Option<Node<'a, 'b>> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

pub fn children<'a, 'b>(
    node: Node<'a, 'b>,
    name: &'static str,
) -> impl Iterator<Item = Node<'a, 'b>> {
    node.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

/// The first descendant element called `name`.
pub fn find<'a, 'b>(node: Node<'a, 'b>, name: &str) -> Option<Node<'a, 'b>> {
    node.descendants()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

/// An attribute by local name (`val` finds `w:val`).
pub fn attr<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == name)
        .map(|a| a.value())
}

/// The relationship id of an element (`r:id`), which shares its local name with a plain `id`.
pub fn rel_id<'a>(node: Node<'a, '_>) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == "id" && a.namespace().is_some_and(|n| n.contains("relationships")))
        .map(|a| a.value())
}

/// The `val` attribute of the child called `name`.
pub fn val<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(node, name).and_then(|c| attr(c, "val"))
}

/// An on/off property (`<w:b/>`, `<w:b w:val="0"/>`) of a properties node.
pub fn flag(props: Node, name: &str) -> Option<bool> {
    child(props, name).map(|c| !matches!(attr(c, "val"), Some("0" | "false" | "off" | "none")))
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// The MIME type of an image part, or None for formats a web view cannot draw (EMF, WMF).
pub fn image_mime(part: &str) -> Option<&'static str> {
    Some(
        match part.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" | "jpe" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            "tif" | "tiff" => "image/tiff",
            "svg" => "image/svg+xml",
            _ => return None,
        },
    )
}

/// The part as a `data:` URL, when it is a drawable image of a sensible size.
pub fn image_data_url(zip: &Zip, part: &str) -> Option<String> {
    use base64::Engine as _;
    let mime = image_mime(part)?;
    let bytes = zip.read(part)?;
    if bytes.len() > 12 * 1024 * 1024 {
        return None;
    }
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A zip with stored entries, enough for the tests (no checksums are verified by the reader).
    pub fn zip_store(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let offset = out.len() as u32;
            let mut local = Vec::new();
            local.extend(0x0403_4b50u32.to_le_bytes());
            local.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            local.extend(0u32.to_le_bytes());
            local.extend((data.len() as u32).to_le_bytes());
            local.extend((data.len() as u32).to_le_bytes());
            local.extend((name.len() as u16).to_le_bytes());
            local.extend(0u16.to_le_bytes());
            out.extend(&local);
            out.extend(name.as_bytes());
            out.extend(*data);
            central.extend(0x0201_4b50u32.to_le_bytes());
            central.extend([20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend(0u32.to_le_bytes());
            central.extend((data.len() as u32).to_le_bytes());
            central.extend((data.len() as u32).to_le_bytes());
            central.extend((name.len() as u16).to_le_bytes());
            central.extend([0u8; 12]);
            central.extend(offset.to_le_bytes());
            central.extend(name.as_bytes());
        }
        let start = out.len() as u32;
        out.extend(&central);
        out.extend(0x0605_4b50u32.to_le_bytes());
        out.extend([0, 0, 0, 0]);
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((central.len() as u32).to_le_bytes());
        out.extend(start.to_le_bytes());
        out.extend([0, 0]);
        out
    }

    #[test]
    fn a_stored_archive_reads_back() {
        let zip = zip_store(&[("a/b.xml", b"<x/>"), ("c.txt", b"hello")]);
        let z = Zip::open(&zip).unwrap();
        assert_eq!(z.read_text("c.txt").as_deref(), Some("hello"));
        assert_eq!(z.read("a/b.xml").unwrap(), b"<x/>");
        assert!(z.read("missing").is_none());
        assert!(Zip::open(b"not a zip at all, really not").is_err());
    }

    #[test]
    fn part_paths_resolve_like_relationship_targets() {
        assert_eq!(resolve("ppt/slides", "../media/a.png"), "ppt/media/a.png");
        assert_eq!(resolve("word", "media/a.png"), "word/media/a.png");
        assert_eq!(resolve("word", "/word/x.xml"), "word/x.xml");
    }

    #[test]
    fn xml_lookups_ignore_prefixes() {
        let doc =
            parse("<w:p xmlns:w='u'><w:pPr><w:jc w:val='center'/><w:b/></w:pPr></w:p>").unwrap();
        let props = child(doc.root_element(), "pPr").unwrap();
        assert_eq!(val(props, "jc"), Some("center"));
        assert_eq!(flag(props, "b"), Some(true));
        assert_eq!(flag(props, "i"), None);
    }

    #[test]
    fn markup_is_escaped() {
        assert_eq!(
            escape("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }
}
