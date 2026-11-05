//! The composer's attachment chips and the attachment viewer, after Sirus Code's
//! `ComposerContextChips` and `AttachmentModal`. This is a child module of `shell` (declared there
//! with `#[path]`) so it can reach the window's state; the parsing lives in `attachment_doc`, the
//! PDF reading in `attachment_pdf`, and everything slow runs on a thread.
use super::*;
use crate::attachment_doc::{self as doc, Info, Preview, Sheet, Viewer, WordBlock, format_size};
use crate::attachment_html as html;
use crate::attachment_pdf;
use crate::attachment_web as web;
use crate::file_icons::file_icon;
use crate::reply_rich::{self, Act, Asset, Want};
use crate::panel_files::source_line;
use crate::syntax::LineSpans;
use gpui::{
    ObjectFit, ScrollHandle, StyledImage, UniformListScrollHandle, img, uniform_list,
};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

const MAX_FILES: usize = 20;
const MAX_FILE: u64 = 10 * 1024 * 1024;
const MAX_BATCH: u64 = 40 * 1024 * 1024;
const HEADER: f32 = 46.;
const BAR: f32 = 40.;

// ---- State -------------------------------------------------------------------------------

/// What the window keeps for attachments: what the host said of each draft file, and the viewer.
#[derive(Default)]
pub(super) struct Files {
    drafts: HashMap<String, Info>,
    open: Option<Open>,
    generation: u64,
    /// Small copies of sent pictures (a draft's come with the host's reply), for the bubbles and
    /// the gallery's strip; loaded a few at a time by `pump_thumbs`.
    thumbs: HashMap<String, Arc<gpui::Image>>,
    thumbs_pending: HashSet<String>,
    thumbs_failed: HashSet<String>,
    /// The native view may show over the body (the viewer has finished fading in, no dialog is up).
    native_visible: bool,
    /// Host requests for a reply's images and videos in flight, and whether the page engine is
    /// drawing a diagram or a formula (one at a time).
    assets_busy: usize,
    drawing: bool,
}

impl Files {
    pub(super) fn note(&mut self, reply: &Value) {
        for item in reply.as_array().into_iter().flatten() {
            let info = Info::from_json(item);
            self.drafts.insert(info.id.clone(), info);
        }
    }

    /// True while the viewer is on screen (or fading out).
    pub(super) fn visible(&self) -> bool {
        self.open.is_some()
    }

    /// Small tiles (34 by 26) for the images among a queued request's files, up to four.
    pub(super) fn queue_tiles(&self, files: &[(String, String)], row: usize) -> Vec<Stateful<Div>> {
        files
            .iter()
            .filter_map(|(id, _)| self.drafts.get(id))
            .filter(|info| info.is_image())
            .take(4)
            .enumerate()
            .map(|(n, info)| {
                div()
                    .id(("queue-thumb", row * 64 + n))
                    .w(px(34.))
                    .h(px(26.))
                    .flex_shrink_0()
                    .rounded(px(5.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(content(0.1))
                    .bg(content(0.05))
                    .when_some(self.thumb_of(info), |d, thumb| {
                        d.child(img(thumb).size_full().object_fit(ObjectFit::Cover))
                    })
            })
            .collect()
    }

    /// The small picture of an image attachment, when there is one yet.
    pub(super) fn thumb_of(&self, info: &Info) -> Option<Arc<gpui::Image>> {
        info.thumb
            .clone()
            .or_else(|| self.thumbs.get(&info.id).cloned())
    }
}

/// The lines of a text file, with their syntax colors once parsed.
#[derive(Clone)]
struct TextBody {
    lines: Arc<Vec<String>>,
    spans: Option<Arc<Vec<LineSpans>>>,
}

struct PdfData {
    bytes: Arc<Vec<u8>>,
    pages: usize,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum WebKind {
    Word,
    Slides,
    #[default]
    Page,
}

/// A document converted to a page for the web view (`attachment_html`), or a video file.
#[derive(Default)]
struct WebDoc {
    html: String,
    /// A video of a reply, played by the web view itself.
    video: Option<std::path::PathBuf>,
    kind: WebKind,
    headings: Vec<html::Heading>,
    slides: usize,
}

/// What an attachment turned into once the host answered. Sendable: it is made on a thread.
enum Content {
    Loading,
    Failed(String),
    Pdf(Arc<PdfData>),
    Web(Arc<WebDoc>),
    Word(Arc<Vec<WordBlock>>),
    Sheets(Arc<Vec<Sheet>>),
    Slides(Arc<Vec<Vec<String>>>),
    Text(TextBody),
    Image(Arc<gpui::Image>),
}

struct Open {
    info: Info,
    owner: String,
    sent: bool,
    /// The pictures of the composer or message the file was opened from, in order (empty when the
    /// file is not a picture), and the one shown among them.
    gallery: Vec<Info>,
    generation: u64,
    t: Tween,
    closing: bool,
    content: Content,
    /// The file's own text, for the page/source toggle of CSV and HTML.
    source: Option<TextBody>,
    show_source: bool,
    scroll: ScrollHandle,
    // PDF
    page: usize,
    editing: Option<String>,
    /// The native view over the body (a web view or PDFKit's), made when first painted.
    overlay: Rc<RefCell<Option<web::Overlay>>>,
    /// The slide a presentation shows, and whether Word's outline list is open.
    slide: usize,
    outline: bool,
    // Spreadsheets
    tab: usize,
    selected: Option<(usize, usize)>,
    rows: UniformListScrollHandle,
}

// ---- Pure layout -------------------------------------------------------------------------

fn text_body(name: &str, text: &str) -> TextBody {
    TextBody {
        lines: Arc::new(text.lines().map(str::to_owned).collect()),
        spans: crate::syntax::highlight(name, text).map(Arc::new),
    }
}

/// What the files to attach add up to, before any byte is read: the message to show when it is
/// too much. `existing` counts the attachments already in the draft.
fn check_limits(sizes: &[u64], existing: usize) -> Result<(), &'static str> {
    if existing + sizes.len() > MAX_FILES {
        return Err(t(
            "Mantenha no máximo 20 anexos. Remova um antes de adicionar mais.",
        ));
    }
    if sizes.iter().any(|size| *size > MAX_FILE) {
        return Err(t("Cada arquivo deve ter até 10 MiB."));
    }
    if sizes.iter().sum::<u64>() > MAX_BATCH {
        return Err(t("Os anexos devem somar até 40 MiB."));
    }
    Ok(())
}

/// Reads the files the person chose into the host's `{name, data}` form. Meant for a thread; sizes
/// are checked from the file system first, so a huge file is refused unread.
fn prepare_files(paths: &[std::path::PathBuf], existing: usize) -> Result<Vec<Value>, String> {
    let sizes: Vec<u64> = paths
        .iter()
        .map(|path| {
            std::fs::metadata(path)
                .map(|m| if m.is_dir() { 0 } else { m.len() })
                .unwrap_or(0)
        })
        .collect();
    check_limits(&sizes, existing).map_err(str::to_owned)?;
    let mut files = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // A folder goes as the names inside it, one level, as Sirus Code's attachments.
        if path.is_dir() {
            let mut names: Vec<String> = std::fs::read_dir(path)
                .map_err(|_| tf("Não foi possível ler {}.", &[&name]))?
                .flatten()
                .map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if entry.path().is_dir() {
                        format!("{name}/")
                    } else {
                        name
                    }
                })
                .collect();
            names.sort();
            names.truncate(500);
            let text = format!(
                "Selected folder names (one level; file contents are not included):\n{}",
                names.join("\n")
            );
            files.push(json!({ "name": format!("{name}.txt"), "data": base64(text.as_bytes()) }));
            continue;
        }
        let data = std::fs::read(path).map_err(|_| tf("Não foi possível ler {}.", &[&name]))?;
        // The file may have grown since it was measured.
        if data.len() as u64 > MAX_FILE {
            return Err(t("Cada arquivo deve ter até 10 MiB.").to_owned());
        }
        files.push(json!({ "name": name, "data": base64(&data) }));
    }
    Ok(files)
}

// ---- Loading -----------------------------------------------------------------------------

/// Asks the host for an attachment's document. Blocks.
fn fetch(
    backend: &Backend,
    command: &str,
    owner: &str,
    id: &str,
    plain: bool,
) -> Result<Value, String> {
    let reply = backend
        .request(command, json!({ "owner": owner, "id": id, "plain": plain }))
        .recv_blocking();
    match reply {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(doc::error_message(&error).to_owned()),
        Err(_) => Err(t("Não foi possível abrir este documento.").to_owned()),
    }
}

fn failed(message: impl Into<String>) -> Content {
    Content::Failed(message.into())
}

fn pdf_message(error: attachment_pdf::Error) -> &'static str {
    match error {
        attachment_pdf::Error::Limit => {
            t("Este PDF excede os limites de páginas ou imagens do leitor.")
        }
        _ => t("Não foi possível ler este PDF. Ele pode estar protegido por senha ou danificado."),
    }
}

/// Everything slow about opening `info`: the host's reply, then decoding it.
fn load(backend: &Backend, info: &Info, owner: &str) -> Content {
    let viewer = info.viewer();
    match viewer {
        Viewer::Image => match fetch(backend, "attachment_image", owner, &info.id, false) {
            Ok(Value::String(url)) => decode_image(&url).map_or_else(
                || failed(t("Não foi possível abrir este documento.")),
                Content::Image,
            ),
            Ok(_) => failed(t("Não foi possível abrir este documento.")),
            Err(message) => failed(message),
        },
        Viewer::Pdf => {
            match fetch(backend, "attachment_preview", owner, &info.id, false).and_then(|v| {
                doc::parse_preview(&v)
                    .map_err(|_| t("Não foi possível abrir este documento.").to_owned())
            }) {
                Ok(Preview::Pdf(bytes)) => match attachment_pdf::open(&bytes) {
                    Ok(pdf) => Content::Pdf(Arc::new(PdfData {
                        bytes: Arc::new(bytes),
                        pages: pdf.sizes.len(),
                    })),
                    Err(error) => failed(pdf_message(error)),
                },
                Ok(_) => failed(t("Não foi possível abrir este documento.")),
                Err(message) => failed(message),
            }
        }
        Viewer::Docx => {
            // The document as a page (pictures, layout); the host's text reading when it cannot.
            let converted = fetch(backend, "attachment_preview", owner, &info.id, false)
                .ok()
                .and_then(|v| doc::parse_preview(&v).ok())
                .and_then(|preview| match preview {
                    Preview::Office { bytes, .. } => html::docx_to_html(&bytes).ok(),
                    _ => None,
                });
            match converted {
                Some(page) => Content::Web(Arc::new(WebDoc {
                    html: page.html,
                    kind: WebKind::Word,
                    headings: page.headings,
                    slides: 0,
                    ..Default::default()
                })),
                None => load_plain(backend, info, owner),
            }
        }
        Viewer::Xlsx | Viewer::Csv => load_plain(backend, info, owner),
        Viewer::Pptx => {
            match fetch(backend, "attachment_preview", owner, &info.id, false).and_then(|v| {
                doc::parse_preview(&v)
                    .map_err(|_| t("Não foi possível abrir este documento.").to_owned())
            }) {
                Ok(Preview::Office { bytes, .. }) => match html::pptx_to_html(&bytes) {
                    Ok(deck) => Content::Web(Arc::new(WebDoc {
                        html: deck.html,
                        kind: WebKind::Slides,
                        headings: Vec::new(),
                        slides: deck.slides,
                        ..Default::default()
                    })),
                    Err(_) => match doc::pptx_slides(&bytes) {
                        Ok(slides) => Content::Slides(Arc::new(slides)),
                        Err(_) => failed(t("Não foi possível abrir este documento.")),
                    },
                },
                Ok(_) => failed(t("Não foi possível abrir este documento.")),
                Err(message) => failed(message),
            }
        }
        Viewer::Tsv => {
            let mut sheet = doc::parse_delimited(&info.content, '\t');
            sheet.name = "TSV".to_owned();
            Content::Sheets(Arc::new(vec![sheet]))
        }
        // The legacy formats get the host's own refusal, which says to convert them.
        Viewer::Legacy => match fetch(backend, "attachment_preview", owner, &info.id, false) {
            Err(message) => failed(message),
            Ok(_) => failed(t("Não foi possível abrir este documento.")),
        },
        // A web page is drawn (no scripts, no network); its source is one toggle away.
        Viewer::Html => Content::Web(Arc::new(WebDoc {
            html: html::html_attachment(&info.content),
            kind: WebKind::Page,
            headings: Vec::new(),
            slides: 0,
            ..Default::default()
        })),
        Viewer::Text => Content::Text(text_body(&info.name, &info.content)),
        Viewer::None => failed(t("Não foi possível abrir este documento.")),
    }
}

/// The host's own reading of a Word or spreadsheet file: text and tables, or cells.
fn load_plain(backend: &Backend, info: &Info, owner: &str) -> Content {
    match fetch(backend, "attachment_preview", owner, &info.id, true).and_then(|v| {
        doc::parse_preview(&v).map_err(|_| t("Não foi possível abrir este documento.").to_owned())
    }) {
        Ok(Preview::Word(blocks)) => Content::Word(Arc::new(blocks)),
        Ok(Preview::Sheets(sheets)) => Content::Sheets(Arc::new(sheets)),
        Ok(_) => failed(t("Não foi possível abrir este documento.")),
        Err(message) => failed(message),
    }
}

/// The picture inside a `data:image/...;base64,` URL.
fn decode_image(url: &str) -> Option<Arc<gpui::Image>> {
    use base64::Engine as _;
    let (head, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
    let format = doc::image_format("", head)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    Some(Arc::new(gpui::Image::from_bytes(format, bytes)))
}

/// The ink as a CSS color, for the hidden page that draws formulas and diagrams.
fn ink_css() -> String {
    let rgb = content(1.).to_rgb();
    let channel = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("rgb({},{},{})", channel(rgb.r), channel(rgb.g), channel(rgb.b))
}

/// Gets one of a reply's pictures or videos from the host, which decides whether the path may be
/// shown at all (`reply_image`, `reply_media`). Blocks.
fn fetch_asset(backend: &Backend, want: &Want) -> Asset {
    match want {
        Want::Image { session, path } => {
            let reply = backend
                .request("reply_image", json!({ "sessionId": session, "path": path }))
                .recv_blocking();
            let Ok(Ok(Value::String(url))) = reply else {
                return Asset::Failed;
            };
            decode_sized(&url).unwrap_or(Asset::Failed)
        }
        Want::Video { session, path, cwd } => {
            let reply = backend
                .request("reply_media", json!({ "sessionId": session, "path": path }))
                .recv_blocking();
            let Ok(Ok(grant)) = reply else {
                return Asset::Failed;
            };
            // The host's grant is all that is needed; the player reads the file itself.
            if let Some(token) = grant["token"].as_str() {
                backend.invoke("media_release", json!({ "token": token }));
            }
            let Some(file) = reply_rich::video_file(cwd, path) else {
                return Asset::Failed;
            };
            let poster = poster_of(&file);
            Asset::Video { path: file, poster }
        }
        _ => Asset::Failed,
    }
}

/// The picture inside a `data:image/...;base64,` URL, with its size in points.
fn decode_sized(url: &str) -> Option<Asset> {
    use base64::Engine as _;
    let (head, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
    let format = doc::image_format("", head)?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
    let (w, h) = reply_rich::image_size(&bytes).unwrap_or((320, 180));
    Some(Asset::Picture {
        image: Arc::new(gpui::Image::from_bytes(format, bytes)),
        size: (w as f32, h as f32),
    })
}

/// A still of a video, from the system's Quick Look; `None` when it makes none.
fn poster_of(file: &std::path::Path) -> Option<(Arc<gpui::Image>, (f32, f32))> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("sirus-poster-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let done = std::process::Command::new("/usr/bin/qlmanage")
        .args(["-t", "-s", "640", "-o"])
        .arg(&dir)
        .arg(file)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    let png = done
        .then(|| std::fs::read(dir.join(format!("{}.png", file.file_name()?.to_str()?))).ok())
        .flatten();
    let _ = std::fs::remove_dir_all(&dir);
    let png = png?;
    let (w, h) = reply_rich::image_size(&png)?;
    Some((Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png)), (w as f32, h as f32)))
}

// ---- The window's side -------------------------------------------------------------------

impl Shell {
    /// Opens a draft attachment from the composer.
    fn open_draft(&mut self, id: &str, cx: &mut Context<Self>) {
        let ix = self.active_conversation();
        let Some(info) = self.viewer.drafts.get(id).cloned() else {
            return;
        };
        let owner = format!("session:{}", self.conversations[ix].session_id);
        // The gallery is the draft's pictures, in the order of the chips.
        let files: Vec<Info> = self
            .attachments
            .get(&ix)
            .into_iter()
            .flatten()
            .filter_map(|(id, _)| self.viewer.drafts.get(id).cloned())
            .collect();
        let gallery = if info.is_image() {
            doc::images_of(&files)
        } else {
            Vec::new()
        };
        self.open_attachment(info, owner, false, gallery, cx);
    }

    /// Opens an attachment of a message already sent.
    pub(super) fn open_sent(
        &mut self,
        conversation: usize,
        info: Info,
        files: &[Info],
        cx: &mut Context<Self>,
    ) {
        let owner = format!("session:{}", self.conversations[conversation].session_id);
        let gallery = if info.is_image() {
            doc::images_of(files)
        } else {
            Vec::new()
        };
        self.open_attachment(info, owner, true, gallery, cx);
    }

    fn open_attachment(
        &mut self,
        info: Info,
        owner: String,
        sent: bool,
        gallery: Vec<Info>,
        cx: &mut Context<Self>,
    ) {
        self.open_attachment_with(info, owner, sent, gallery, None, cx);
    }

    /// Opens the viewer. With `ready`, the content is already in hand (a reply's picture, video
    /// or page) and the host is not asked for the file.
    fn open_attachment_with(
        &mut self,
        info: Info,
        owner: String,
        sent: bool,
        gallery: Vec<Info>,
        ready: Option<(Content, Option<TextBody>)>,
        cx: &mut Context<Self>,
    ) {
        if ready.is_none() && !info.openable() {
            return;
        }
        let loaded = ready.is_some();
        let (content, ready_source) = match ready {
            Some((content, source)) => (content, source),
            None => (Content::Loading, None),
        };
        self.viewer.generation += 1;
        let generation = self.viewer.generation;
        // A table or a web page can also be read as the text it came from.
        let source = ready_source.or_else(|| {
            (!info.content.is_empty()
                && matches!(info.viewer(), Viewer::Csv | Viewer::Tsv | Viewer::Html))
            .then(|| text_body(&info.name, &info.content))
        });
        let show_source = false;
        self.viewer.open = Some(Open {
            info: info.clone(),
            owner: owner.clone(),
            sent,
            gallery,
            generation,
            t: Tween::from_to(
                0.,
                1.,
                Instant::now(),
                if cx.reduce_motion() { 0. } else { 0.2 },
                Curve::Expo,
            ),
            closing: false,
            content,
            source,
            show_source,
            scroll: ScrollHandle::new(),
            page: 0,
            editing: None,
            overlay: Rc::new(RefCell::new(None)),
            slide: 0,
            outline: false,
            tab: 0,
            selected: None,
            rows: UniformListScrollHandle::new(),
        });
        if !loaded {
            self.load_attachment(cx);
        }
        cx.notify();
    }

    fn load_attachment(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.viewer.open.as_mut() else {
            return;
        };
        open.content = Content::Loading;
        *open.overlay.borrow_mut() = None;
        open.slide = 0;
        open.outline = false;
        let (generation, info, owner) = (open.generation, open.info.clone(), open.owner.clone());
        let backend = self.backend.clone();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(load(&backend, &info, &owner));
        });
        cx.spawn(async move |this, cx| {
            let Ok(content) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| {
                if let Some(open) = this
                    .viewer
                    .open
                    .as_mut()
                    .filter(|o| o.generation == generation)
                {
                    open.content = content;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn close_attachment(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.viewer.open.as_mut().filter(|o| !o.closing) {
            open.closing = true;
            open.t.set(
                0.,
                Instant::now(),
                if cx.reduce_motion() { 0. } else { 0.14 },
                Curve::Expo,
            );
            cx.notify();
        }
    }

    /// Takes a draft attachment out of the composer and gives it back to the host.
    fn remove_draft(&mut self, id: &str) {
        let Some(ix) = self
            .attachments
            .iter()
            .find(|(_, l)| l.iter().any(|(i, _)| i == id))
            .map(|(ix, _)| *ix)
        else {
            return;
        };
        let owner = format!("session:{}", self.conversations[ix].session_id);
        self.backend.invoke(
            "release_prompt_attachments",
            json!({ "owner": owner, "ids": [id] }),
        );
        if let Some(list) = self.attachments.get_mut(&ix) {
            list.retain(|(kept, _)| kept != id);
        }
        self.viewer.drafts.remove(id);
    }

    fn ask_remove_attachment(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.viewer.open.as_ref().filter(|o| !o.sent) else {
            return;
        };
        let (id, name) = (open.info.id.clone(), open.info.name.clone());
        self.ask(
            t("Remover anexo?").to_owned(),
            tf("\"{}\" sai do rascunho.", &[&name]),
            t("Remover"),
            move |this, cx| {
                this.remove_draft(&id);
                this.gallery_after_removal(&id, cx);
            },
            cx,
        );
    }

    /// The keys while the viewer is open; true when it took the key. `command` is ⌘.
    pub(super) fn attachment_key(
        &mut self,
        key: &str,
        command: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(open) = self.viewer.open.as_mut() else {
            return false;
        };
        if open.closing {
            return true;
        }
        let pdf = match &open.content {
            Content::Pdf(pdf) => Some(pdf.pages),
            _ => None,
        };
        let slides = match &open.content {
            Content::Web(web) if web.kind == WebKind::Slides => Some(web.slides),
            _ => None,
        };
        if let (Some(editing), Some(pages)) = (open.editing.as_mut(), pdf) {
            match key {
                "enter" => {
                    if let Ok(n) = editing.parse::<usize>() {
                        let target = n.clamp(1, pages.max(1)) - 1;
                        open.page = target;
                        if let Some(o) = open.overlay.borrow().as_ref() {
                            o.go_to_page(target);
                        }
                    }
                    open.editing = None;
                }
                "escape" => open.editing = None,
                "backspace" => {
                    editing.pop();
                }
                digit
                    if digit.len() == 1
                        && digit.as_bytes()[0].is_ascii_digit()
                        && editing.len() < 5 =>
                {
                    editing.push_str(digit)
                }
                _ => {}
            }
            cx.notify();
            return true;
        }
        if matches!(key, "left" | "right") && !open.gallery.is_empty() {
            self.gallery_step(if key == "left" { -1 } else { 1 }, cx);
            cx.notify();
            return true;
        }
        // ⌘+ ⌘- ⌘0 zoom a page or a PDF (the same keys reach the native view through its monitor).
        if command && matches!(key, "=" | "+" | "-" | "0") {
            let by = match key {
                "-" => -1,
                "0" => 0,
                _ => 1,
            };
            if let Some(o) = open.overlay.borrow().as_ref() {
                o.zoom(by);
            }
            cx.notify();
            return true;
        }
        match (key, pdf, slides) {
            ("escape", _, _) => self.close_attachment(cx),
            ("left" | "pageup", Some(_), _) => self.pdf_step(-1),
            ("right" | "pagedown", Some(pages), _) if pages > 0 => self.pdf_step(1),
            ("left" | "pageup" | "up", _, Some(_)) => self.slide_step(-1),
            ("right" | "pagedown" | "down", _, Some(_)) => self.slide_step(1),
            _ => {}
        }
        cx.notify();
        true
    }

    /// Shows the picture `by` places away in the gallery (nothing at the ends). True when it moved.
    fn gallery_step(&mut self, by: isize, cx: &mut Context<Self>) -> bool {
        let Some(open) = self.viewer.open.as_mut() else {
            return false;
        };
        match doc::gallery_step(&open.gallery, &open.info.id, by) {
            Some(target) => {
                let next = open.gallery[target].clone();
                self.show_gallery(next, cx);
                true
            }
            None => false,
        }
    }

    /// Swaps the viewer's file for another picture of its gallery, keeping the window open.
    fn show_gallery(&mut self, info: Info, cx: &mut Context<Self>) {
        self.viewer.generation += 1;
        let generation = self.viewer.generation;
        if let Some(open) = self.viewer.open.as_mut() {
            open.info = info;
            open.generation = generation;
        }
        self.load_attachment(cx);
        cx.notify();
    }

    /// After a draft picture is removed: its neighbour shows, or the viewer closes when none is left.
    fn gallery_after_removal(&mut self, removed: &str, cx: &mut Context<Self>) {
        let next = self.viewer.open.as_ref().and_then(|open| {
            let at = open.gallery.iter().position(|f| f.id == removed)?;
            open.gallery
                .get(at + 1)
                .or_else(|| at.checked_sub(1).and_then(|i| open.gallery.get(i)))
                .cloned()
        });
        match next {
            Some(info) => {
                if let Some(open) = self.viewer.open.as_mut() {
                    open.gallery.retain(|f| f.id != removed);
                }
                self.show_gallery(info, cx);
            }
            None => self.close_attachment(cx),
        }
    }

    fn pdf_step(&mut self, by: isize) {
        let Some(open) = self.viewer.open.as_mut() else {
            return;
        };
        let Content::Pdf(pdf) = &open.content else {
            return;
        };
        let target = (open.page as isize + by).clamp(0, pdf.pages as isize - 1) as usize;
        open.page = target;
        if let Some(o) = open.overlay.borrow().as_ref() {
            o.go_to_page(target);
        }
    }

    fn slide_step(&mut self, by: isize) {
        let Some(open) = self.viewer.open.as_mut() else {
            return;
        };
        let Content::Web(web) = &open.content else {
            return;
        };
        let target = (open.slide as isize + by).clamp(0, web.slides as isize - 1) as usize;
        self.slide_to(target);
    }

    /// Shows slide `target` (0-based): the page does it and reports back.
    fn slide_to(&mut self, target: usize) {
        if let Some(open) = self.viewer.open.as_mut() {
            open.slide = target;
            if let Some(o) = open.overlay.borrow().as_ref() {
                o.eval(&format!("sirusShow({target})"));
            }
        }
    }

    /// What the native view reported (see `attachment_web::Event`).
    fn overlay_event(&mut self, generation: u64, event: web::Event, cx: &mut Context<Self>) {
        let Some(open) = self.viewer.open.as_mut().filter(|o| o.generation == generation) else {
            return;
        };
        match event {
            web::Event::Slide(i) => open.slide = i,
            web::Event::Page(i) => open.page = i,
            web::Event::Scale => {}
            web::Event::Escape => self.close_attachment(cx),
            web::Event::Zoom(by) => {
                if let Some(o) = open.overlay.borrow().as_ref() {
                    o.zoom(by);
                }
            }
            web::Event::Link(url) => {
                // Only web and mail links leave the viewer, and the system's default app takes them.
                if url.starts_with("https://")
                    || url.starts_with("http://")
                    || url.starts_with("mailto:")
                {
                    let _ = std::process::Command::new("/usr/bin/open").arg(url).spawn();
                }
            }
        }
        cx.notify();
    }

    /// The native view's slot in the layout: it is put over this element's bounds each frame.
    /// `visible` is false while something of GPUI's must show above it.
    fn native_body(&self, open: &Open, visible: bool, cx: &mut Context<Self>) -> AnyElement {
        let slot = open.overlay.clone();
        let generation = open.generation;
        let view = cx.entity().downgrade();
        let (html, pdf, video): (Option<String>, Option<Arc<Vec<u8>>>, Option<std::path::PathBuf>) =
            match &open.content {
                Content::Web(web) if web.video.is_some() => (None, None, web.video.clone()),
                Content::Web(web) => (Some(web.html.clone()), None, None),
                Content::Pdf(pdf) => (None, Some(pdf.bytes.clone()), None),
                _ => (None, None, None),
            };
        gpui::canvas(
            |bounds, _, _| bounds,
            move |_, bounds, window, cx| {
                if slot.borrow().is_none() {
                    let made = match (&html, &pdf, &video) {
                        (_, _, Some(file)) => web::Overlay::file(window, file),
                        (Some(html), _, _) => web::Overlay::web(window, html),
                        (_, Some(bytes), _) => web::Overlay::pdf(window, bytes),
                        _ => None,
                    };
                    let Some(overlay) = made else { return };
                    let events = overlay.events.clone();
                    let view = view.clone();
                    *slot.borrow_mut() = Some(overlay);
                    cx.spawn(async move |cx| {
                        while let Ok(event) = events.recv().await {
                            let done = view
                                .update(cx, |this, cx| this.overlay_event(generation, event, cx))
                                .is_err();
                            if done {
                                break;
                            }
                        }
                    })
                    .detach();
                }
                if let Some(overlay) = slot.borrow().as_ref() {
                    overlay.place(bounds, window, visible);
                }
            },
        )
        .size_full()
        .into_any_element()
    }

    // ---- A reply's pictures, videos, diagrams, formulas and pages ------------------------------

    /// Schedules `pump_reply_assets` after this frame when a reply asked for something new. Cheap
    /// and idempotent; nothing is redrawn unless a result arrives.
    pub(super) fn wake_reply_assets(&self, cx: &mut Context<Self>) {
        if reply_rich::has_wanted() {
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |this, cx| this.pump_reply_assets(cx));
            })
            .detach();
        }
    }

    /// Starts the queued requests: a few host fetches at once, and one drawing at a time in the
    /// hidden page. Each result is stored by its content key and the window redraws once.
    fn pump_reply_assets(&mut self, cx: &mut Context<Self>) {
        let host_room = 3usize.saturating_sub(self.viewer.assets_busy);
        let draw_room = usize::from(!self.viewer.drawing);
        for (key, want) in reply_rich::take_wanted(host_room, draw_room) {
            if want.is_drawn() {
                self.viewer.drawing = true;
                let ink = ink_css();
                cx.spawn(async move |this, cx| {
                    let asset = match crate::reply_render::draw(&want, &ink, cx).await {
                        Ok((png, size)) => reply_rich::picture(png, size),
                        Err(_) => Asset::Failed,
                    };
                    let _ = this.update(cx, |this, cx| {
                        reply_rich::finish(&key, asset);
                        this.viewer.drawing = false;
                        cx.notify();
                    });
                })
                .detach();
            } else {
                self.viewer.assets_busy += 1;
                let backend = self.backend.clone();
                let (tx, rx) = async_channel::bounded(1);
                std::thread::spawn(move || {
                    let _ = tx.send_blocking(fetch_asset(&backend, &want));
                });
                cx.spawn(async move |this, cx| {
                    let asset = rx.recv().await.unwrap_or(Asset::Failed);
                    let _ = this.update(cx, |this, cx| {
                        reply_rich::finish(&key, asset);
                        this.viewer.assets_busy = this.viewer.assets_busy.saturating_sub(1);
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    /// A click on a reply's picture, video or page: it opens in the viewer.
    pub(super) fn reply_act(&mut self, act: Act, cx: &mut Context<Self>) {
        let named = |name: &str| Info {
            id: "reply".to_owned(),
            name: name.to_owned(),
            kind: "file".to_owned(),
            mime: String::new(),
            size: None,
            content: String::new(),
            truncated: false,
            thumb: None,
        };
        match act {
            Act::Image { name, want } => {
                if let Some(Asset::Picture { image, .. }) = reply_rich::peek(&want) {
                    let info = named(&name);
                    self.open_attachment_with(info, String::new(), true, Vec::new(), Some((Content::Image(image), None)), cx);
                }
            }
            Act::Video { name, path } => {
                let video = Content::Web(Arc::new(WebDoc { video: Some(path), ..Default::default() }));
                self.open_attachment_with(named(&name), String::new(), true, Vec::new(), Some((video, None)), cx);
            }
            Act::Html { source } => self.open_preview(source, cx),
            Act::HtmlFile { session, link } => {
                let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
                let path = reply_rich::resolve_link(std::path::Path::new(""), home.as_deref(), &link);
                let rx = self.backend.request(
                    "read_text_file",
                    json!({ "sessionId": session, "path": path.to_string_lossy() }),
                );
                cx.spawn(async move |this, cx| {
                    let Ok(Ok(reply)) = rx.recv().await else { return };
                    if reply["binary"].as_bool().unwrap_or(false) {
                        return;
                    }
                    let source = reply["content"].as_str().unwrap_or("").to_owned();
                    let _ = this.update(cx, |this, cx| this.open_preview(source, cx));
                })
                .detach();
            }
        }
    }

    /// HTML a reply wrote, in the web view under Sirus Code's rules for it (`reply_rich::PREVIEW_POLICY`):
    /// its own scripts run, nothing is fetched. One toggle away is the source.
    fn open_preview(&mut self, source: String, cx: &mut Context<Self>) {
        let Some(page) = reply_rich::preview_page(&source) else {
            return;
        };
        let info = Info {
            id: "reply".to_owned(),
            name: format!("{}.html", t("Prévia em HTML")),
            kind: "file".to_owned(),
            mime: "text/html".to_owned(),
            size: Some(source.len() as u64),
            content: String::new(),
            truncated: false,
            thumb: None,
        };
        let text = text_body(&info.name, &source);
        let content = Content::Web(Arc::new(WebDoc { html: page, ..Default::default() }));
        self.open_attachment_with(info, String::new(), true, Vec::new(), Some((content, Some(text))), cx);
    }

    /// Loads, a few at a time, the small pictures the bubbles and the gallery's strip show for
    /// sent images: those of the open gallery first, then the active conversation's messages.
    fn pump_thumbs(&mut self, cx: &mut Context<Self>) {
        const AT_ONCE: usize = 3;
        if self.viewer.thumbs_pending.len() >= AT_ONCE {
            return;
        }
        let ix = self.active_conversation();
        let Some(conversation) = self.conversations.get(ix) else {
            return;
        };
        let mut wanted: Vec<(String, String)> = Vec::new();
        let mut want = |viewer: &Files, owner: &str, info: &Info| {
            if info.is_image()
                && info.thumb.is_none()
                && !viewer.thumbs.contains_key(&info.id)
                && !viewer.thumbs_pending.contains(&info.id)
                && !viewer.thumbs_failed.contains(&info.id)
                && !viewer.drafts.contains_key(&info.id)
            {
                wanted.push((owner.to_owned(), info.id.clone()));
            }
        };
        if let Some(open) = self.viewer.open.as_ref().filter(|o| o.sent) {
            for info in &open.gallery {
                want(&self.viewer, &open.owner, info);
            }
        }
        let owner = format!("session:{}", conversation.session_id);
        for turn in conversation.turns.iter().rev().take(40) {
            for info in &turn.attachments {
                want(&self.viewer, &owner, info);
            }
        }
        for (owner, id) in wanted
            .into_iter()
            .take(AT_ONCE - self.viewer.thumbs_pending.len())
        {
            self.viewer.thumbs_pending.insert(id.clone());
            let backend = self.backend.clone();
            let (tx, rx) = async_channel::bounded(1);
            let asked = id.clone();
            std::thread::spawn(move || {
                let image = match fetch(&backend, "attachment_image", &owner, &asked, false) {
                    Ok(Value::String(url)) => decode_image(&url),
                    _ => None,
                };
                let _ = tx.send_blocking(image);
            });
            cx.spawn(async move |this, cx| {
                let image = rx.recv().await.ok().flatten();
                let _ = this.update(cx, |this, cx| {
                    this.viewer.thumbs_pending.remove(&id);
                    match image {
                        Some(image) => {
                            this.viewer.thumbs.insert(id, image);
                        }
                        None => {
                            this.viewer.thumbs_failed.insert(id);
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    // ---- Chips ----------------------------------------------------------------------------

    /// The draft's files above the text, inside the composer card.
    pub(super) fn attachment_chips(
        &self,
        attached: &[(String, String)],
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if attached.is_empty() {
            return None;
        }
        Some(
            div()
                .px(px(8.))
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .children(attached.iter().enumerate().map(|(i, (id, name))| {
                    let info = self.viewer.drafts.get(id).cloned().unwrap_or_else(|| Info {
                        id: id.clone(),
                        name: name.clone(),
                        kind: "file".into(),
                        mime: String::new(),
                        size: None,
                        content: String::new(),
                        truncated: false,
                        thumb: None,
                    });
                    let (open_id, remove_id) = (id.clone(), id.clone());
                    chip(("attached", i), &info, true)
                        .when(info.openable(), |d| {
                            d.cursor_pointer().on_click(
                                cx.listener(move |this, _, _, cx| this.open_draft(&open_id, cx)),
                            )
                        })
                        .child(
                            div()
                                .id(("attached-remove", i))
                                .size(px(20.))
                                .rounded(px(6.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(content(0.1)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    // A click on the cross is not a click on the chip.
                                    cx.stop_propagation();
                                    this.remove_draft(&remove_id);
                                    cx.notify();
                                }))
                                .child(icon("close", 11.).text_color(content(0.6))),
                        )
                }))
                .into_any_element(),
        )
    }

    /// The files of a sent message, over its bubble.
    pub(super) fn sent_chips(
        &self,
        conversation: usize,
        turn: usize,
        files: &[Info],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .justify_end()
            .gap(px(6.))
            .children(files.iter().enumerate().map(|(i, info)| {
                let opened = info.clone();
                let all = files.to_vec();
                let click = cx.listener(move |this, _, _, cx| {
                    this.open_sent(conversation, opened.clone(), &all, cx);
                });
                if info.is_image() {
                    // A picture shows as itself, so there is something to click.
                    return picture_tile(
                        ("sent-image", turn * 64 + i),
                        self.viewer.thumb_of(info),
                        click,
                    )
                    .into_any_element();
                }
                chip(("sent-file", turn * 64 + i), info, false)
                    .when(info.openable(), |d| d.cursor_pointer().on_click(click))
                    .into_any_element()
            }))
            .into_any_element()
    }

    /// Reads files for the host off the UI thread, refusing too many or too large ones.
    pub(super) fn attach_files(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        let existing = self
            .attachments
            .get(&self.active_conversation())
            .map_or(0, Vec::len);
        self.attach_with(move || prepare_files(&paths, existing), cx);
    }

    /// An image pasted from the clipboard.
    pub(super) fn attach_pasted_image(&mut self, bytes: Vec<u8>, kind: gpui::ImageFormat, cx: &mut Context<Self>) {
        let existing = self
            .attachments
            .get(&self.active_conversation())
            .map_or(0, Vec::len);
        self.attach_with(
            move || {
                // The host goes by the name, so it has to say what the bytes are. Screenshot
                // tools often copy TIFF, which models don't read: it becomes a PNG.
                let (name, bytes) = match kind {
                    gpui::ImageFormat::Png => ("imagem.png", bytes),
                    gpui::ImageFormat::Jpeg => ("imagem.jpg", bytes),
                    gpui::ImageFormat::Webp => ("imagem.webp", bytes),
                    gpui::ImageFormat::Gif => ("imagem.gif", bytes),
                    _ => ("imagem.png", to_png(&bytes).ok_or_else(|| t("Não foi possível anexar.").to_owned())?),
                };
                check_limits(&[bytes.len() as u64], existing).map_err(str::to_owned)?;
                Ok(vec![json!({ "name": name, "data": base64(&bytes) })])
            },
            cx,
        );
    }

    /// Runs `prepare` on a thread, then hands its files to the host for the active conversation.
    fn attach_with(
        &mut self,
        prepare: impl FnOnce() -> Result<Vec<Value>, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let ix = self.active_conversation();
        let backend = self.backend.clone();
        let owner = format!("session:{}", self.conversations[ix].session_id);
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(prepare());
        });
        cx.spawn(async move |this, cx| {
            let Ok(prepared) = rx.recv().await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                match prepared {
                    Ok(files) if !files.is_empty() => {
                        let id = backend.invoke(
                            "paste_prompt_attachments",
                            json!({ "owner": owner, "files": files }),
                        );
                        this.waiting.insert(id, Waiting::Attached(ix));
                    }
                    Ok(_) => {}
                    Err(message) => this.problem = Some(message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- The viewer -----------------------------------------------------------------------

    pub(super) fn attachment_view(
        &mut self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.pump_thumbs(cx);
        let open = self.viewer.open.as_ref()?;
        if open.closing && open.t.done(now) {
            self.viewer.open = None;
            return None;
        }
        if !open.t.done(now) {
            *moving = true;
        }
        let shown = open.t.at(now).clamp(0., 1.);
        // A picture opens large; documents keep the reader's size.
        let picture = open.info.viewer() == Viewer::Image;
        let width = (self.viewport_width - 80.).clamp(320., if picture { 1500. } else { 1100. });
        let height = (self.viewport_height - 80.).clamp(240., if picture { 1000. } else { 860. });
        let _ = height;
        let native = !open.closing && open.t.done(now) && self.confirm.is_none() && !open.show_source;
        if !native && let Some(overlay) = open.overlay.borrow().as_ref() {
            overlay.hide();
        }
        self.viewer.native_visible = native;
        let open = self.viewer.open.as_ref()?;
        let info = &open.info;
        let button = |id: &'static str| {
            div()
                .id(id)
                .size(px(30.))
                .rounded(px(8.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(content(0.08)))
        };
        let can_toggle = open.source.is_some();
        let header = div()
            .h(px(HEADER))
            .flex_shrink_0()
            .pl(px(16.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(content(0.08))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(content(0.55))
                    .child(t(if open.sent { "Enviado" } else { "Rascunho" })),
            )
            .child(icon("chevron-right", 12.).text_color(content(0.4)))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(0.95))
                    .child(info.name.clone()),
            )
            .when_some(
                open.gallery
                    .iter()
                    .position(|f| f.id == info.id)
                    .filter(|_| open.gallery.len() > 1),
                |d, at| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.5))
                            .text_color(content(0.5))
                            .child(format!(
                                "· {}",
                                tf("{} de {}", &[&(at + 1), &open.gallery.len()])
                            )),
                    )
                },
            )
            .when_some(info.size, |d, size| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.5))
                        .text_color(content(0.5))
                        .child(format_size(size)),
                )
            })
            .child(div().flex_1())
            .when(can_toggle, |d| {
                d.child(
                    button("attachment-source")
                        .px(px(8.))
                        .w_auto()
                        .text_size(px(12.))
                        .text_color(content(0.7))
                        .when(open.show_source, |s| s.bg(content(0.08)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(open) = this.viewer.open.as_mut() {
                                open.show_source = !open.show_source;
                            }
                            cx.notify();
                        }))
                        .child(t(if open.show_source {
                            "Ver página"
                        } else {
                            "Ver código-fonte"
                        })),
                )
            })
            .when(!info.content.is_empty(), |d| {
                let text = info.content.clone();
                d.child(
                    button("attachment-copy")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()))
                        }))
                        .child(icon("copy", 15.).text_color(content(0.6))),
                )
            })
            .when(!open.sent, |d| {
                d.child(
                    button("attachment-trash")
                        .on_click(cx.listener(|this, _, _, cx| this.ask_remove_attachment(cx)))
                        .child(icon("trash", 15.).text_color(content(0.6))),
                )
            })
            .child(
                button("attachment-close")
                    .on_click(cx.listener(|this, _, _, cx| this.close_attachment(cx)))
                    .child(icon("close", 15.).text_color(content(0.6))),
            );
        let body = self.attachment_body(width, cx);
        Some(
            div()
                .id("attachment-backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::Hsla {
                    a: 0.55 * shown,
                    ..gpui::black()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_attachment(cx);
                    }),
                )
                .child(
                    div()
                        .id("attachment-card")
                        .relative()
                        .top(px((1. - shown) * 8.))
                        .opacity(shown)
                        .w(px(width))
                        .h(px(height))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .rounded(px(16.))
                        .border_1()
                        .border_color(content(0.12))
                        .child(crate::theme::floating_surface(16.))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(header)
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .flex()
                                .flex_col()
                                .bg(content(0.03))
                                .child(body),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The file's view; a picture with neighbours gets the gallery's arrows and strip around it.
    fn attachment_body(&self, modal_width: f32, cx: &mut Context<Self>) -> AnyElement {
        let inner = self.attachment_content(modal_width, cx);
        match self.viewer.open.as_ref() {
            Some(open) if !open.gallery.is_empty() => self.gallery_frame(inner, open, cx),
            _ => inner,
        }
    }

    /// The arrows on the sides of the picture and the strip of thumbnails under it.
    fn gallery_frame(&self, inner: AnyElement, open: &Open, cx: &mut Context<Self>) -> AnyElement {
        let many = open.gallery.len() > 1;
        let at = open.gallery.iter().position(|f| f.id == open.info.id);
        let arrow = |id: &'static str, name: &'static str, left: bool, enabled: bool| {
            div()
                .id(id)
                .absolute()
                .top_0()
                .bottom_0()
                .when(left, |d| d.left(px(14.)))
                .when(!left, |d| d.right(px(14.)))
                .flex()
                .items_center()
                .child(
                    div()
                        .id((id, 1usize))
                        .size(px(38.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::Hsla { a: 0.62, ..gpui::black() })
                        .border_1()
                        .border_color(content(0.16))
                        .opacity(if enabled { 1. } else { 0.3 })
                        .when(enabled, |d| {
                            d.cursor_pointer().hover(|s| s.bg(content(0.2)))
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.gallery_step(if left { -1 } else { 1 }, cx);
                            cx.notify();
                        }))
                        .child(icon(name, 18.).text_color(content(0.9))),
                )
        };
        let area = div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(inner)
            .when(many, |d| {
                d.child(arrow(
                    "gallery-prev",
                    "chevron-left",
                    true,
                    at.is_some_and(|i| i > 0),
                ))
                .child(arrow(
                    "gallery-next",
                    "chevron-right",
                    false,
                    at.is_some_and(|i| i + 1 < open.gallery.len()),
                ))
            });
        let strip = div()
            .id("gallery-strip")
            .h(px(72.))
            .flex_shrink_0()
            .border_t_1()
            .border_color(content(0.08))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .px(px(12.))
            .overflow_x_scroll()
            .children(open.gallery.iter().enumerate().map(|(i, info)| {
                let current = info.id == open.info.id;
                let target = info.clone();
                div()
                    .id(("gallery-thumb", i))
                    .size(px(48.))
                    .flex_shrink_0()
                    .rounded(px(8.))
                    .overflow_hidden()
                    .border_2()
                    .border_color(if current {
                        accent()
                    } else {
                        gpui::transparent_black()
                    })
                    .bg(content(0.06))
                    .cursor_pointer()
                    .when(!current, |d| d.opacity(0.75).hover(|s| s.opacity(1.)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this
                            .viewer
                            .open
                            .as_ref()
                            .is_some_and(|o| o.info.id != target.id)
                        {
                            this.show_gallery(target.clone(), cx);
                        }
                    }))
                    .child(match self.viewer.thumb_of(info) {
                        Some(thumb) => img(thumb)
                            .size_full()
                            .object_fit(ObjectFit::Cover)
                            .into_any_element(),
                        None => div().size_full().into_any_element(),
                    })
            }));
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(area)
            .when(many, |d| d.child(strip))
            .into_any_element()
    }

    fn attachment_content(&self, _modal_width: f32, cx: &mut Context<Self>) -> AnyElement {
        let Some(open) = self.viewer.open.as_ref() else {
            return div().into_any_element();
        };
        let note = |text: String| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p(px(24.))
                .text_size(px(13.))
                .text_color(content(0.5))
                .text_center()
                .child(text)
        };
        if open.show_source
            && let Some(source) = &open.source
        {
            return text_view(source.clone());
        }
        match &open.content {
            Content::Loading => note(t("Abrindo documento…").to_owned()).into_any_element(),
            Content::Failed(message) => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .p(px(24.))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(content(0.55))
                        .text_center()
                        .child(message.clone()),
                )
                .child(
                    div()
                        .id("attachment-retry")
                        .h(px(28.))
                        .px(px(12.))
                        .rounded(px(8.))
                        .flex()
                        .items_center()
                        .bg(content(0.08))
                        .text_size(px(12.5))
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.12)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.load_attachment(cx);
                            cx.notify();
                        }))
                        .child(t("Tentar novamente")),
                )
                .into_any_element(),
            Content::Text(text) => text_view(text.clone()),
            Content::Image(image) => div()
                .flex_1()
                .min_h_0()
                .p(px(16.))
                .child(
                    img(image.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            Content::Word(blocks) => word_view(blocks, open),
            Content::Slides(slides) => slides_view(slides, open),
            Content::Sheets(sheets) => self.sheets_view(sheets.clone(), open, cx),
            Content::Pdf(pdf) => self.pdf_view(pdf, open, cx),
            Content::Web(web) => self.web_view(web, open, cx),
        }
    }

    /// A PDF in PDFKit's own view (selection, copy, pinch and wheel zoom), under the page bar.
    fn pdf_view(&self, pdf: &Arc<PdfData>, open: &Open, cx: &mut Context<Self>) -> AnyElement {
        let pages = pdf.pages;
        let percent = open.overlay.borrow().as_ref().map_or(100, |o| o.percent());
        let tool = |id: &'static str| {
            div()
                .id(id)
                .size(px(28.))
                .rounded(px(7.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(content(0.08)))
        };
        let box_text = match &open.editing {
            Some(typed) => format!("{typed}|"),
            None => (open.page + 1).to_string(),
        };
        let toolbar = div()
            .h(px(BAR))
            .flex_shrink_0()
            .px(px(12.))
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(content(0.08))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        tool("pdf-prev")
                            .opacity(if open.page > 0 { 1. } else { 0.35 })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pdf_step(-1);
                                cx.notify();
                            }))
                            .child(icon("chevron-left", 14.)),
                    )
                    .child(
                        div()
                            .id("pdf-page-box")
                            .w(px(48.))
                            .h(px(26.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(if open.editing.is_some() {
                                accent()
                            } else {
                                content(0.14)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.5))
                            .cursor_text()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(open) = this.viewer.open.as_mut() {
                                    open.editing = Some(String::new());
                                }
                                cx.notify();
                            }))
                            .child(box_text),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .child(format!("/ {pages}")),
                    )
                    .child(
                        tool("pdf-next")
                            .opacity(if open.page + 1 < pages { 1. } else { 0.35 })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pdf_step(1);
                                cx.notify();
                            }))
                            .child(icon("chevron-right", 14.)),
                    ),
            )
            .child(self.zoom_controls(open, percent, cx));
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.native_body(open, self.viewer.native_visible, cx)),
            )
            .into_any_element()
    }

    /// − 100% + for a page or a PDF: ⌘- ⌘0 ⌘+ do the same.
    fn zoom_controls(&self, open: &Open, percent: u32, cx: &mut Context<Self>) -> Div {
        let _ = open;
        let tool = |id: &'static str| {
            div()
                .id(id)
                .size(px(28.))
                .rounded(px(7.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(content(0.08)))
        };
        let zoom = |by: i8| {
            cx.listener(move |this, _, _, cx| {
                if let Some(open) = this.viewer.open.as_ref()
                    && let Some(o) = open.overlay.borrow().as_ref()
                {
                    o.zoom(by);
                }
                cx.notify();
            })
        };
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                tool("zoom-out")
                    .on_click(zoom(-1))
                    .child(div().w(px(11.)).h(px(1.5)).rounded_full().bg(content(0.7))),
            )
            .child(
                div()
                    .id("zoom-fit")
                    .min_w(px(44.))
                    .h(px(26.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.))
                    .text_color(content(0.7))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.08)))
                    .on_click(zoom(0))
                    .child(format!("{percent}%")),
            )
            .child(
                tool("zoom-in")
                    .on_click(zoom(1))
                    .child(icon("plus", 13.)),
            )
    }

    /// A converted Word page or a presentation in the web view, under its bar: Word's outline and
    /// zoom, or the slide counter and arrows.
    fn web_view(&self, web: &Arc<WebDoc>, open: &Open, cx: &mut Context<Self>) -> AnyElement {
        let tool = |id: &'static str| {
            div()
                .id(id)
                .size(px(28.))
                .rounded(px(7.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(content(0.08)))
        };
        let bar = div()
            .h(px(BAR))
            .flex_shrink_0()
            .px(px(12.))
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(content(0.08));
        let mut column = div().flex_1().min_h_0().flex().flex_col();
        match web.kind {
            WebKind::Page => {}
            WebKind::Slides => {
                column = column.child(
                    bar.justify_center()
                        .gap(px(10.))
                        .child(
                            tool("slide-prev")
                                .opacity(if open.slide > 0 { 1. } else { 0.35 })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.slide_step(-1);
                                    cx.notify();
                                }))
                                .child(icon("chevron-left", 14.)),
                        )
                        .child(
                            div()
                                .min_w(px(110.))
                                .text_center()
                                .text_size(px(12.5))
                                .text_color(content(0.7))
                                .child(tf("Slide {} de {}", &[&(open.slide + 1), &web.slides])),
                        )
                        .child(
                            tool("slide-next")
                                .opacity(if open.slide + 1 < web.slides { 1. } else { 0.35 })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.slide_step(1);
                                    cx.notify();
                                }))
                                .child(icon("chevron-right", 14.)),
                        ),
                );
            }
            WebKind::Word => {
                let percent = open.overlay.borrow().as_ref().map_or(100, |o| o.percent());
                let outline = (!web.headings.is_empty()).then(|| {
                    div()
                        .id("outline-toggle")
                        .h(px(26.))
                        .px(px(10.))
                        .rounded(px(7.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(12.5))
                        .text_color(content(0.75))
                        .cursor_pointer()
                        .when(open.outline, |d| d.bg(content(0.1)))
                        .hover(|s| s.bg(content(0.08)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(open) = this.viewer.open.as_mut() {
                                open.outline = !open.outline;
                            }
                            cx.notify();
                        }))
                        .child(t("Estrutura"))
                        .child(icon(if open.outline { "chevron-up" } else { "chevron-down" }, 12.))
                });
                column = column.child(
                    bar.child(div().flex().items_center().children(outline))
                        .child(self.zoom_controls(open, percent, cx)),
                );
                // The outline opens as a list between the bar and the page, so the native page
                // view (which draws above GPUI) is never covered by it.
                if open.outline {
                    column = column.child(
                        div()
                            .id("outline-list")
                            .flex_shrink_0()
                            .max_h(px(190.))
                            .overflow_y_scroll()
                            .py(px(4.))
                            .border_b_1()
                            .border_color(content(0.08))
                            .children(web.headings.iter().enumerate().map(|(i, heading)| {
                                let target = heading.id.clone();
                                div()
                                    .id(("outline-item", i))
                                    .h(px(28.))
                                    .pl(px(16. + 14. * f32::from(heading.level.saturating_sub(1))))
                                    .pr(px(12.))
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.5))
                                    .text_color(content(0.85))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(content(0.08)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(open) = this.viewer.open.as_mut() {
                                            if let Some(o) = open.overlay.borrow().as_ref() {
                                                o.eval(&format!(
                                                    "document.getElementById('{target}').scrollIntoView({{block:'start'}})"
                                                ));
                                            }
                                            open.outline = false;
                                        }
                                        cx.notify();
                                    }))
                                    .child(div().truncate().child(heading.text.clone()))
                            })),
                    );
                }
            }
        }
        column
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.native_body(open, self.viewer.native_visible, cx)),
            )
            .into_any_element()
    }

    fn sheets_view(
        &self,
        sheets: Arc<Vec<Sheet>>,
        open: &Open,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab = open.tab.min(sheets.len().saturating_sub(1));
        let Some(sheet) = sheets.get(tab) else {
            return div()
                .flex_1()
                .p(px(24.))
                .text_color(content(0.5))
                .child(t("Este documento não tem conteúdo legível."))
                .into_any_element();
        };
        let selected = open.selected;
        let current = selected.map(|(r, c)| {
            let label = format!("{}{}", doc::column_name(c), r + 1);
            let value = sheet
                .formulas
                .get(&(r, c))
                .map_or_else(|| sheet.value(r, c).to_owned(), |f| format!("={f}"));
            (label, value)
        });
        let gutter = 48.;
        let total: f32 = gutter + sheet.widths.iter().sum::<f32>();
        let rows = sheet.rows.len().min(5000);
        let header = div()
            .h(px(26.))
            .w(px(total))
            .flex_shrink_0()
            .flex()
            .bg(content(0.06))
            .border_b_1()
            .border_color(content(0.1))
            .text_size(px(11.5))
            .text_color(content(0.5))
            .child(
                div()
                    .w(px(gutter))
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(content(0.1)),
            )
            .children(sheet.widths.iter().enumerate().map(|(c, w)| {
                div()
                    .w(px(*w))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_r_1()
                    .border_color(content(0.1))
                    .child(doc::column_name(c))
            }));
        let list = uniform_list(
            "sheet-rows",
            rows,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                let Some(open) = this.viewer.open.as_ref() else {
                    return Vec::new();
                };
                let Content::Sheets(sheets) = &open.content else {
                    return Vec::new();
                };
                let Some(sheet) = sheets.get(open.tab.min(sheets.len().saturating_sub(1))) else {
                    return Vec::new();
                };
                let selected = open.selected;
                range
                    .map(|row| {
                        div()
                            .h(px(26.))
                            .w(px(48. + sheet.widths.iter().sum::<f32>()))
                            .flex()
                            .text_size(px(12.))
                            .child(
                                div()
                                    .w(px(48.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(content(0.06))
                                    .border_r_1()
                                    .border_b_1()
                                    .border_color(content(0.1))
                                    .text_color(content(0.45))
                                    .child((row + 1).to_string()),
                            )
                            .children(sheet.widths.iter().enumerate().map(|(column, w)| {
                                let chosen = selected == Some((row, column));
                                div()
                                    .id(("sheet-cell", row * 256 + column))
                                    .w(px(*w))
                                    .h_full()
                                    .flex_shrink_0()
                                    .px(px(6.))
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .border_r_1()
                                    .border_b_1()
                                    .border_color(content(0.1))
                                    .when(chosen, |d| {
                                        d.bg(accent_wash(0.16)).border_color(accent())
                                    })
                                    .text_color(content(0.9))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(open) = this.viewer.open.as_mut() {
                                            open.selected = Some((row, column));
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        div().truncate().child(sheet.value(row, column).to_owned()),
                                    )
                            }))
                            .into_any_element()
                    })
                    .collect()
            }),
        )
        .track_scroll(&open.rows)
        .w(px(total))
        .flex_1()
        .min_h_0();
        let formula_bar = div()
            .h(px(BAR))
            .flex_shrink_0()
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(content(0.08))
            .child(
                div()
                    .w(px(48.))
                    .flex_shrink_0()
                    .font_family("Menlo")
                    .text_size(px(12.))
                    .text_color(content(0.7))
                    .child(current.as_ref().map_or(String::new(), |c| c.0.clone())),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.4))
                    .child("ƒx"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(26.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .bg(content(0.05))
                    .flex()
                    .items_center()
                    .font_family("Menlo")
                    .text_size(px(12.))
                    .text_color(content(0.9))
                    .truncate()
                    .child(current.map_or(String::new(), |c| c.1)),
            );
        let tabs = div()
            .h(px(BAR))
            .flex_shrink_0()
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(4.))
            .border_t_1()
            .border_color(content(0.08))
            .children(sheets.iter().enumerate().map(|(i, s)| {
                div()
                    .id(("sheet-tab", i))
                    .h(px(26.))
                    .px(px(12.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .text_color(if i == tab {
                        content(0.95)
                    } else {
                        content(0.5)
                    })
                    .when(i == tab, |d| d.bg(content(0.1)))
                    .cursor_pointer()
                    .hover(|st| st.bg(content(0.07)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(open) = this.viewer.open.as_mut() {
                            open.tab = i;
                            open.selected = None;
                        }
                        cx.notify();
                    }))
                    .child(s.name.clone())
            }));
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(formula_bar)
            .child(
                div()
                    .id("sheet-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scroll()
                    .child(div().h_full().w(px(total)).flex().flex_col().child(header).child(list)),
            )
            .child(tabs)
            .child(
                div()
                    .flex_shrink_0()
                    .px(px(12.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(content(0.08))
                    .text_size(px(11.5))
                    .text_color(content(0.45))
                    .child(t("Valores salvos das células. Fórmulas não são recalculadas; gráficos e formatação não são exibidos.")),
            )
            .into_any_element()
    }
}

// ---- Static views ------------------------------------------------------------------------

/// A sent picture in its bubble: its small copy (or an empty tile while it loads), clickable.
fn picture_tile(
    id: (&'static str, usize),
    thumb: Option<Arc<gpui::Image>>,
    click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(96.))
        .flex_shrink_0()
        .rounded(px(12.))
        .overflow_hidden()
        .border_1()
        .border_color(content(0.1))
        .bg(content(0.05))
        .cursor_pointer()
        .hover(|s| s.border_color(content(0.28)))
        .on_click(click)
        .when_some(thumb, |d, thumb| {
            d.child(img(thumb).size_full().object_fit(ObjectFit::Cover))
        })
}

/// A file chip: the type icon (or the image), the name, the size.
fn chip(id: (&'static str, usize), info: &Info, draft: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(34.))
        .max_w_full()
        .pl(px(8.))
        .pr(if draft { px(6.) } else { px(10.) })
        .rounded(px(10.))
        .border_1()
        .border_color(content(0.1))
        .bg(content(0.05))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.))
        .text_color(content(0.9))
        .child(match &info.thumb {
            Some(thumb) => img(thumb.clone())
                .size(px(22.))
                .flex_shrink_0()
                .rounded(px(5.))
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => file_icon(&info.name, info.kind == "folder", 18.),
        })
        .child(
            div()
                .min_w_0()
                .max_w(px(220.))
                .truncate()
                .child(info.name.clone()),
        )
        .when(info.kind == "file", |d| {
            d.when_some(info.size, |d, size| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(content(0.5))
                        .child(format_size(size)),
                )
            })
        })
        .when(info.truncated, |d| {
            d.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(content(0.5))
                    .child(format!("· {}", t("resumido"))),
            )
        })
}

fn text_view(text: TextBody) -> AnyElement {
    // The host sends at most a few KB of a text file, so the lines are laid out directly: each
    // wraps at the view's width and grows its row (a fixed-height list would cut long ones).
    const SHOWN: usize = 4000;
    div()
        .id("attachment-lines")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .py(px(8.))
        .children(text.lines.iter().take(SHOWN).enumerate().map(|(i, line)| {
            source_line(i, line, text.spans.as_ref().and_then(|s| s.get(i)))
        }))
        .into_any_element()
}

/// The Word text and tables on a white page, as Sirus Code's `WordReader`.
fn word_view(blocks: &Arc<Vec<WordBlock>>, open: &Open) -> AnyElement {
    let ink: gpui::Hsla = rgb(0x242424).into();
    let rule: gpui::Hsla = rgb(0xd4d4d4).into();
    const SHOWN: usize = 2000;
    let page = div()
        .w_full()
        .max_w(px(800.))
        .min_h_full()
        .rounded(px(3.))
        .bg(gpui::white())
        .p(px(24.))
        .text_color(ink)
        .text_size(px(14.))
        .line_height(px(24.))
        .flex()
        .flex_col()
        .child(div().flex_col().children(if blocks.is_empty() {
            vec![
                div()
                    .child(t("Este documento não tem conteúdo legível."))
                    .into_any_element(),
            ]
        } else {
            blocks
                .iter()
                .take(SHOWN)
                .map(|block| match block {
                    WordBlock::Table { rows } => div()
                        .my(px(14.))
                        .flex()
                        .flex_col()
                        .border_t_1()
                        .border_l_1()
                        .border_color(rule)
                        .children(rows.iter().map(|row| {
                            div().flex().children(row.iter().map(|cell| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .px(px(10.))
                                    .py(px(6.))
                                    .border_r_1()
                                    .border_b_1()
                                    .border_color(rule)
                                    .child(cell.clone())
                            }))
                        }))
                        .into_any_element(),
                    WordBlock::Paragraph { text, heading, .. } if *heading > 0 => {
                        let size = 14. * (1.8 - f32::from(*heading) * 0.12).max(1.1);
                        div()
                            .mt(px(14.))
                            .mb(px(8.))
                            .text_size(px(size))
                            .line_height(px(size * 1.4))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(text.clone())
                            .into_any_element()
                    }
                    WordBlock::Paragraph { text, list, .. } => div()
                        .mb(px(8.))
                        .min_h(px(24.))
                        .child(if *list {
                            format!("• {text}")
                        } else {
                            text.clone()
                        })
                        .into_any_element(),
                })
                .collect()
        }));
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            div()
                .id("word-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&open.scroll)
                .p(px(16.))
                .flex()
                .justify_center()
                .child(page),
        )
        .child(
            div()
                .flex_shrink_0()
                .px(px(12.))
                .py(px(8.))
                .border_t_1()
                .border_color(content(0.08))
                .text_size(px(11.5))
                .text_color(content(0.45))
                .child(t(
                    "Texto e tabelas. Imagens e diagramação original não são exibidas.",
                )),
        )
        .into_any_element()
}

fn slides_view(slides: &Arc<Vec<Vec<String>>>, open: &Open) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            div()
                .id("slides-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&open.scroll)
                .p(px(16.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(14.))
                .children(slides.iter().enumerate().map(|(i, lines)| {
                    div()
                        .w_full()
                        .max_w(px(760.))
                        .flex_shrink_0()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(content(0.1))
                        .bg(content(0.05))
                        .p(px(18.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(content(0.45))
                                .child(tf("Slide {} de {}", &[&(i + 1), &slides.len()])),
                        )
                        .children(lines.iter().enumerate().map(|(n, line)| {
                            div()
                                .text_size(px(if n == 0 { 17. } else { 13.5 }))
                                .line_height(px(if n == 0 { 24. } else { 20. }))
                                .font_weight(if n == 0 {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(content(if n == 0 { 0.95 } else { 0.8 }))
                                .child(line.clone())
                        }))
                        .when(lines.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(content(0.4))
                                    .child("—"),
                            )
                        })
                })),
        )
        .child(
            div()
                .flex_shrink_0()
                .px(px(12.))
                .py(px(8.))
                .border_t_1()
                .border_color(content(0.08))
                .text_size(px(11.5))
                .text_color(content(0.45))
                .child(t("Texto dos slides. Imagens e layout não são exibidos.")),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_checked_before_anything_is_read() {
        let mb = 1024 * 1024;
        assert!(check_limits(&[mb, 2 * mb], 3).is_ok());
        assert!(check_limits(&[mb], 20).is_err());
        assert!(check_limits(&vec![mb; 21], 0).is_err());
        assert!(check_limits(&[10 * mb + 1], 0).is_err());
        assert!(check_limits(&[10 * mb; 5], 0).is_err());
        assert!(check_limits(&[10 * mb; 4], 0).is_ok());
    }

    #[test]
    fn files_are_read_for_the_host_and_big_ones_refused_unread() {
        let dir = std::env::temp_dir().join(format!("sirus-prepare-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        std::fs::write(dir.join("folder/inside.rs"), "x").unwrap();
        let ok = prepare_files(&[dir.join("a.txt"), dir.join("folder")], 0).unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[0]["name"], "a.txt");
        assert_eq!(ok[0]["data"], "aGVsbG8=");
        assert_eq!(ok[1]["name"], "folder.txt");
        let big = dir.join("big.bin");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(MAX_FILE + 1).unwrap();
        assert!(prepare_files(&[big], 0).is_err());
        assert!(prepare_files(&[dir.join("missing")], 0).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Any image macOS reads, as a PNG, through the system's `sips`.
fn to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("sirus-paste-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let (from, to) = (dir.join("in.img"), dir.join("out.png"));
    std::fs::write(&from, bytes).ok()?;
    let done = std::process::Command::new("sips")
        .args(["-s", "format", "png"])
        .arg(&from)
        .arg("--out")
        .arg(&to)
        .output()
        .is_ok_and(|out| out.status.success());
    let png = done.then(|| std::fs::read(&to).ok()).flatten();
    let _ = std::fs::remove_dir_all(&dir);
    png
}
