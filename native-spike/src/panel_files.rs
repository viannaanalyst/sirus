//! The Arquivos pane, after Sirus's FileTree.tsx and FileView: a 40 px toolbar (new file, new
//! folder, collapse all, search), a lazy tree of the session's folder with an inline row for
//! creating an entry, and files opened in this same pane as tabs (the Terminal's tab strip: file
//! icon, name, close, and a `+` that goes back to the tree to pick another), each with a
//! breadcrumb, Preview or Source (Markdown only has both) and its own scroll position.
//!
//! The search finds files in the whole worktree, not only the folders opened in the tree: the
//! list comes from `git ls-files` (or a bounded walk) on a background thread, and typing is
//! debounced and ranked off the UI thread (project_files.rs). Source view colors come from
//! tree-sitter, parsed once per file in the background (syntax.rs).
use crate::file_icons::{file_icon, folder_icon};
use crate::file_editor::{Editor, EditorEvent, MAX_EDITABLE_BYTES};
use crate::i18n::{t, tf};
use crate::markdown;
use crate::motion::{Curve, DISCLOSE, eased};
use crate::panels::PanelContext;
use crate::tool_images;
use crate::project_files::{self, Hit};
use crate::syntax::{self, Group, LineSpans};
use crate::tab_tile::{Face, TAB_TILE, TAB_TILE_GAP, mark_tile, tab_tile_inset};
use crate::theme::{self, accent, content, is_light};
use gpui::{
    Animation, AnimationExt, AnyElement, AnyView, App, Background, Context, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, FontWeight, HighlightStyle, Hsla,
    KeyDownEvent, ListAlignment, ListState, MouseButton, ObjectFit, Pixels, Point, Render,
    StyledText, Subscription, Transformation, UTF16Selection, Window, canvas, div, img,
    linear_color_stop, linear_gradient, list, prelude::*, px, radians, rgb, svg,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// Results the search lists (Monocode's Quick Open lists 80).
const RESULT_CAP: usize = 80;
/// Quiet time after the last keystroke before the search runs.
const SEARCH_DEBOUNCE_MS: u64 = 120;

/// Shows `path` in the Files pane behind `view` (a pane built by `panels::open`), in a tab.
pub fn show_file(view: &AnyView, path: String, cx: &mut App) {
    if let Ok(panel) = view.clone().downcast::<FilesPanel>() {
        panel.update(cx, |panel, cx| panel.open_file(path, cx));
    }
}

pub fn new(context: PanelContext, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let search = cx.new(|cx| TextField::new(t("Buscar arquivos"), cx));
        let search_events = cx.subscribe(
            &search,
            |this: &mut FilesPanel, _, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Changed => this.search_changed(cx),
                TextFieldEvent::Cancel => this.close_search(cx),
                TextFieldEvent::Submit => this.open_first_hit(cx),
            },
        );
        let mut panel = FilesPanel {
            backend: context.backend,
            session_id: context.session_id,
            root: context.worktree,
            folders: HashMap::new(),
            open: HashSet::new(),
            selected: None,
            root_open: true,
            tabs: Vec::new(),
            active: None,
            search,
            search_open: false,
            index: None,
            indexing: false,
            hits: Vec::new(),
            searching: false,
            search_seq: 0,
            creating: None,
            _search_events: search_events,
        };
        let root = panel.root.clone();
        panel.list(root, cx);
        panel
    })
    .into()
}

struct Node {
    path: String,
    name: String,
    is_dir: bool,
}

enum Listing {
    Loading,
    Ready(Vec<Node>),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Preview,
    Source,
}

enum Viewer {
    Loading,
    Text(Text),
    /// An image, read through the host's `reply_image`.
    Picture(Arc<gpui::Image>),
    Binary,
    Failed(String),
}

/// Where a file's syntax colors stand. They are parsed in the background the first time the Source
/// view is shown, and kept.
enum Colors {
    Idle,
    Pending,
    Ready(Rc<Vec<LineSpans>>),
    /// No grammar for the language, or too big to parse: the text stays plain.
    Plain,
}

/// A text file's two views. Each keeps its own scroll position; both notify the pane on scroll so
/// the scroll-to-bottom button follows.
struct Text {
    content: Arc<str>,
    lines: Rc<Vec<String>>,
    colors: Colors,
    preview: ListState,
    source: ListState,
    /// The Source view's editor. None for a file over MAX_EDITABLE_BYTES, which stays read-only.
    editor: Option<Entity<Editor>>,
    /// The editor's events: a save, or a change that the tab's dot must show.
    _events: Option<Subscription>,
}

/// One open file: a tab. It keeps its mode and scroll position while another tab is on screen.
struct Opened {
    path: String,
    name: String,
    mode: Mode,
    viewer: Viewer,
}

/// An entry being created: a row in the tree, under `parent`, with its name typed in `field`.
struct Creating {
    kind: &'static str,
    parent: String,
    field: Entity<TextField>,
    error: Option<String>,
    _events: Subscription,
}

struct FilesPanel {
    backend: crate::backend::Backend,
    session_id: String,
    /// The session's folder; the tree's root and the default for `list_dir`.
    root: String,
    /// Listings by folder path. A folder keeps its last listing while it reloads.
    folders: HashMap<String, Listing>,
    /// Expanded folders (the root is shown by its heading row).
    open: HashSet<String>,
    /// The selected row's path and whether it is a folder.
    selected: Option<(String, bool)>,
    root_open: bool,
    /// The open files, in tab order.
    tabs: Vec<Opened>,
    /// The tab on screen; None while the tree is showing (no tab open, or `+` was pressed).
    active: Option<usize>,
    search: Entity<TextField>,
    search_open: bool,
    /// Every file of the worktree (relative paths), as of the last listing.
    index: Option<Arc<Vec<String>>>,
    indexing: bool,
    /// The best matches for the typed text.
    hits: Vec<Hit>,
    /// A search is waiting for its debounce, the index, or its ranking.
    searching: bool,
    /// Counts keystrokes, so an older search's answer is dropped.
    search_seq: u64,
    creating: Option<Creating>,
    _search_events: Subscription,
}

impl FilesPanel {
    /// Lists `path` from the back end; the answer replaces the folder's listing when it arrives.
    fn list(&mut self, path: String, cx: &mut Context<Self>) {
        let args = if path == self.root {
            json!({ "sessionId": self.session_id })
        } else {
            json!({ "sessionId": self.session_id, "path": path.clone() })
        };
        self.folders.entry(path.clone()).or_insert(Listing::Loading);
        let rx = self.backend.request("list_dir", args);
        cx.spawn(async move |this, cx| {
            let listing = match settle(rx).await {
                Ok(value) => Listing::Ready(nodes(&value)),
                Err(error) => Listing::Failed(error),
            };
            let _ = this.update(cx, |this, cx| {
                this.folders.insert(path, listing);
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, path: String, is_dir: bool, cx: &mut Context<Self>) {
        self.selected = Some((path.clone(), is_dir));
        if is_dir {
            if !self.open.remove(&path) {
                self.open.insert(path.clone());
                self.list(path, cx);
            }
        } else {
            self.open_file(path, cx);
        }
        cx.notify();
    }

    fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.open.clear();
        cx.notify();
    }

    fn toggle_root(&mut self, cx: &mut Context<Self>) {
        self.root_open = !self.root_open;
        if self.root_open {
            let root = self.root.clone();
            self.list(root, cx);
        }
        cx.notify();
    }

    /// Shows `path` in a tab: the one already open for it, or a new one at the end.
    fn open_file(&mut self, path: String, cx: &mut Context<Self>) {
        self.selected = Some((path.clone(), false));
        if let Some(ix) = self.tabs.iter().position(|tab| tab.path == path) {
            self.active = Some(ix);
            // A file that failed to load gets another try.
            if matches!(self.tabs[ix].viewer, Viewer::Failed(_)) {
                self.tabs[ix].viewer = Viewer::Loading;
                self.load(path, cx);
            }
            cx.notify();
            return;
        }
        let name = if tool_images::is_capture(&path) {
            t("Captura da tela").to_owned()
        } else {
            path.rsplit('/').next().unwrap_or(&path).to_owned()
        };
        let mode = if is_markdown(&name) {
            Mode::Preview
        } else {
            Mode::Source
        };
        self.tabs.push(Opened {
            path: path.clone(),
            name,
            mode,
            viewer: Viewer::Loading,
        });
        self.active = Some(self.tabs.len() - 1);
        self.load(path, cx);
        cx.notify();
    }

    /// Reads `path` from the back end into its tab. An image comes through `reply_image`, which
    /// is the host's rule for which picture files may be shown.
    fn load(&mut self, path: String, cx: &mut Context<Self>) {
        if tool_images::is_image_path(&path) {
            return self.load_picture(path, cx);
        }
        let rx = self.backend.request(
            "read_text_file",
            json!({ "sessionId": self.session_id, "path": path.clone() }),
        );
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                let viewer = match reply {
                    Ok(value) if value["binary"].as_bool().unwrap_or(false) => Viewer::Binary,
                    Ok(value) => text_viewer(&path, value["content"].as_str().unwrap_or(""), cx),
                    Err(error) => Viewer::Failed(error),
                };
                if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.path == path) {
                    tab.viewer = viewer;
                }
                this.ensure_colors(&path, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn load_picture(&mut self, path: String, cx: &mut Context<Self>) {
        let rx = self.backend.request(
            "reply_image",
            json!({ "sessionId": self.session_id, "path": path.clone() }),
        );
        cx.spawn(async move |this, cx| {
            let viewer = match settle(rx).await {
                Ok(Value::String(url)) => match tool_images::picture(&url) {
                    Some(image) => Viewer::Picture(image),
                    None => Viewer::Failed(t("Não foi possível abrir esta imagem.").into()),
                },
                Ok(_) => Viewer::Failed(t("Não foi possível abrir esta imagem.").into()),
                Err(error) => Viewer::Failed(error),
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.path == path) {
                    tab.viewer = viewer;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Closes tab `ix`. The tab on screen moves to a neighbor; closing the last one shows the tree.
    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        self.tabs.remove(ix);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(active) if active == ix => Some(ix.min(self.tabs.len() - 1)),
            Some(active) if ix < active => Some(active - 1),
            other => other,
        };
        if let Some(path) = self
            .active
            .and_then(|active| self.tabs.get(active))
            .map(|tab| tab.path.clone())
        {
            self.selected = Some((path, false));
        }
        cx.notify();
    }

    fn select_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(ix) {
            self.selected = Some((tab.path.clone(), false));
            self.active = Some(ix);
            cx.notify();
        }
    }

    fn editor_of(&self, path: &str) -> Option<Entity<Editor>> {
        match &self.tabs.iter().find(|tab| tab.path == path)?.viewer {
            Viewer::Text(text) => text.editor.clone(),
            _ => None,
        }
    }

    fn tab_modified(&self, ix: usize, cx: &App) -> bool {
        match self.tabs.get(ix).map(|tab| &tab.viewer) {
            Some(Viewer::Text(text)) => text
                .editor
                .as_ref()
                .is_some_and(|editor| editor.read(cx).is_modified()),
            _ => false,
        }
    }

    /// The tab's ×. A file with unsaved changes asks first, in the shell's popup; the others close.
    fn request_close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tab_modified(ix, cx) {
            self.close_tab(ix, cx);
            return;
        }
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let path = tab.path.clone();
        let name = tab.name.clone();
        let panel = cx.entity();
        if let Some(Some(shell)) = window.root::<crate::shell::Shell>() {
            shell.update(cx, |shell, cx| {
                shell.confirm(
                    t("Descartar alterações?").to_owned(),
                    tf(
                        "\"{}\" tem alterações não salvas. Fechar a aba descarta essas alterações.",
                        &[&name],
                    ),
                    t("Descartar"),
                    move |cx: &mut App| {
                        let _ = panel.update(cx, |this, cx| this.close_path(&path, cx));
                    },
                    cx,
                )
            });
        }
    }

    fn close_path(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|tab| tab.path == path) {
            self.close_tab(ix, cx);
        }
    }

    /// ⌘S: writes the file's text through the host, then marks what was written as saved. The
    /// file is not checked against the disk first: a change made outside the editor is overwritten,
    /// as the desktop app's editor does.
    fn save(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(editor) = self.editor_of(&path) else {
            return;
        };
        if !editor.read(cx).is_modified() {
            return;
        }
        let (content, revision) = editor.read(cx).snapshot();
        let rx = self.backend.request(
            "write_text_file",
            json!({ "sessionId": self.session_id, "path": path, "content": content }),
        );
        cx.spawn(async move |_, cx| {
            let reply = settle(rx).await;
            let _ = editor.update(cx, |editor, cx| match reply {
                Ok(_) => editor.mark_saved(revision, cx),
                Err(error) => editor.set_error(Some(error), cx),
            });
        })
        .detach();
    }

    /// The `+` of the tab strip: back to the tree, with the search ready to pick another file.
    fn add_tab(&mut self, cx: &mut Context<Self>) {
        self.active = None;
        self.root_open = true;
        if !self.search_open {
            self.open_search(cx);
        } else {
            self.search.update(cx, |field, cx| field.focus(cx));
        }
        cx.notify();
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        let Some(tab) = self.active.and_then(|ix| self.tabs.get_mut(ix)) else {
            return;
        };
        tab.mode = mode;
        let path = tab.path.clone();
        if mode == Mode::Source {
            self.ensure_colors(&path, cx);
        }
        cx.notify();
    }

    /// Starts the background parse of a file's colors, once, when its Source view is the one shown.
    fn ensure_colors(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.path == path) else {
            return;
        };
        if tab.mode != Mode::Source {
            return;
        }
        let name = tab.name.clone();
        let Viewer::Text(text) = &mut tab.viewer else {
            return;
        };
        // The editor parses its own colors and keeps them up to date.
        if text.editor.is_some() || !matches!(text.colors, Colors::Idle) {
            return;
        }
        if !syntax::supports(&name) {
            text.colors = Colors::Plain;
            return;
        }
        text.colors = Colors::Pending;
        let content = text.content.clone();
        let rx = project_files::run_blocking(move || syntax::highlight(&name, &content));
        let path = path.to_owned();
        cx.spawn(async move |this, cx| {
            let spans = rx.recv().await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                if let Some(Viewer::Text(text)) = this
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.path == path)
                    .map(|tab| &mut tab.viewer)
                {
                    text.colors =
                        spans.map_or(Colors::Plain, |spans| Colors::Ready(Rc::new(spans)));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_search(&mut self, cx: &mut Context<Self>) {
        if self.search_open {
            self.close_search(cx);
        } else {
            self.open_search(cx);
        }
    }

    fn open_search(&mut self, cx: &mut Context<Self>) {
        self.search_open = true;
        self.search.update(cx, |field, cx| field.focus(cx));
        // The list is read again each time the search opens, so new files show; the last one is
        // searched meanwhile.
        self.reindex(cx);
        cx.notify();
    }

    fn close_search(&mut self, cx: &mut Context<Self>) {
        self.search_open = false;
        self.search.update(cx, |field, cx| field.set_text("", cx));
        self.search_seq += 1;
        self.hits.clear();
        self.searching = false;
        cx.notify();
    }

    /// The text typed in the search, when it is open and not empty.
    fn query(&self, cx: &App) -> Option<String> {
        let text = self.search.read(cx).text().trim().to_owned();
        (self.search_open && !text.is_empty()).then_some(text)
    }

    /// Lists every file of the worktree on a background thread, then ranks the typed text again.
    fn reindex(&mut self, cx: &mut Context<Self>) {
        if self.indexing {
            return;
        }
        self.indexing = true;
        let root = self.root.clone();
        let rx = project_files::run_blocking(move || project_files::list(&root));
        cx.spawn(async move |this, cx| {
            let files = rx.recv().await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.indexing = false;
                this.index = Some(Arc::new(files));
                this.run_search(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The typed text changed: rank after a short quiet time, so fast typing runs one search.
    fn search_changed(&mut self, cx: &mut Context<Self>) {
        self.search_seq += 1;
        let seq = self.search_seq;
        if self.query(cx).is_none() {
            self.hits.clear();
            self.searching = false;
            cx.notify();
            return;
        }
        self.searching = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SEARCH_DEBOUNCE_MS))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.search_seq == seq {
                    this.run_search(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Ranks the index against the typed text on the background executor. Without an index yet the
    /// search waits: the listing calls this again when it arrives.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self.query(cx) else {
            self.hits.clear();
            self.searching = false;
            return;
        };
        let Some(index) = self.index.clone() else {
            return;
        };
        let seq = self.search_seq;
        let ranked =
            cx.background_spawn(async move { project_files::search(&index, &query, RESULT_CAP) });
        cx.spawn(async move |this, cx| {
            let hits = ranked.await;
            let _ = this.update(cx, |this, cx| {
                if this.search_seq == seq {
                    this.hits = hits;
                    this.searching = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The absolute path of a file listed relative to the worktree.
    fn absolute(&self, relative: &str) -> String {
        format!("{}/{}", self.root.trim_end_matches('/'), relative)
    }

    /// Opens a search result in its tab and leaves the search.
    fn open_hit(&mut self, relative: &str, cx: &mut Context<Self>) {
        let path = self.absolute(relative);
        self.close_search(cx);
        self.open_file(path, cx);
    }

    fn open_first_hit(&mut self, cx: &mut Context<Self>) {
        if let Some(relative) = self.hits.first().map(|hit| hit.path.clone()) {
            self.open_hit(&relative, cx);
        }
    }

    /// The folder new entries go to: the selected folder, the selected file's folder, or the root.
    fn target_dir(&self) -> String {
        match &self.selected {
            Some((path, true)) => path.clone(),
            Some((path, false)) => parent_dir(path, &self.root),
            None => self.root.clone(),
        }
    }

    fn start_create(&mut self, kind: &'static str, cx: &mut Context<Self>) {
        let parent = self.target_dir();
        self.root_open = true;
        // Entries are made in the tree: leave the file on screen for it, and the search.
        self.active = None;
        if self.search_open {
            self.close_search(cx);
        }
        if parent != self.root {
            self.open.insert(parent.clone());
        }
        self.list(parent.clone(), cx);
        let placeholder = if kind == "directory" {
            t("Nome da pasta")
        } else {
            t("Nome do arquivo")
        };
        let field = cx.new(|cx| TextField::new(placeholder, cx));
        let events = cx.subscribe(
            &field,
            |this: &mut FilesPanel, _, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Submit => this.submit_create(cx),
                TextFieldEvent::Cancel => {
                    this.creating = None;
                    cx.notify();
                }
                TextFieldEvent::Changed => cx.notify(),
            },
        );
        field.update(cx, |field, cx| field.focus(cx));
        self.creating = Some(Creating {
            kind,
            parent,
            field,
            error: None,
            _events: events,
        });
        cx.notify();
    }

    /// Creates the typed entry through the back end; an empty name just cancels.
    fn submit_create(&mut self, cx: &mut Context<Self>) {
        let Some(creating) = self.creating.as_ref() else {
            return;
        };
        let name = creating.field.read(cx).text().trim().to_owned();
        if name.is_empty() {
            self.creating = None;
            cx.notify();
            return;
        }
        let (kind, parent) = (creating.kind, creating.parent.clone());
        let rx = self.backend.request(
            "create_workspace_entry",
            json!({ "sessionId": self.session_id, "kind": kind, "name": name, "parentPath": parent.clone() }),
        );
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                match reply {
                    Ok(value) => {
                        this.creating = None;
                        this.list(parent, cx);
                        let path = value["path"].as_str().unwrap_or_default().to_owned();
                        if kind == "directory" {
                            this.open.insert(path.clone());
                            this.list(path.clone(), cx);
                            this.selected = Some((path, true));
                        } else if !path.is_empty() {
                            this.open_file(path, cx);
                        }
                        // The new entry is a file of the worktree: the next search lists it.
                        this.index = None;
                    }
                    Err(error) => {
                        if let Some(creating) = this.creating.as_mut() {
                            creating.error = Some(error);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The tree below `dir`: its rows, and each open folder's rows fading in beneath it.
    fn tree_rows(&self, dir: &str, depth: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let nodes = match self.folders.get(dir) {
            Some(Listing::Ready(nodes)) => nodes,
            Some(Listing::Failed(error)) => return vec![note(error, depth, true)],
            Some(Listing::Loading) | None => return vec![note(t("Carregando…"), depth, false)],
        };
        let mut rows = Vec::new();
        if let Some(creating) = self
            .creating
            .as_ref()
            .filter(|creating| creating.parent == dir)
        {
            rows.push(self.create_row(creating, depth));
        }
        for node in nodes {
            let open = node.is_dir && self.open.contains(&node.path);
            rows.push(self.row(node, depth, open, cx));
            if open {
                let children = self.tree_rows(&node.path, depth + 1, cx);
                rows.push(
                    div()
                        .flex()
                        .flex_col()
                        .children(children)
                        .with_animation(
                            format!("children:{}", node.path),
                            Animation::new(Duration::from_secs_f32(DISCLOSE))
                                .with_easing(|t| eased(t, Curve::Expo)),
                            |el, t| el.opacity(t),
                        )
                        .into_any_element(),
                );
            }
        }
        rows
    }

    fn row(&self, node: &Node, depth: usize, open: bool, cx: &mut Context<Self>) -> AnyElement {
        let selected = self
            .selected
            .as_ref()
            .is_some_and(|(path, _)| path == &node.path);
        let (path, is_dir) = (node.path.clone(), node.is_dir);
        let row = div()
            .id(format!("row:{}", node.path))
            .h(px(30.))
            .w_full()
            .pl(px(8. + depth as f32 * 14.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.5))
            .text_color(content(if selected { 1. } else { 0.85 }))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.select(path.clone(), is_dir, cx)));
        let row = if selected {
            row.bg(content(0.08))
        } else {
            row.hover(|style| style.bg(content(0.05)))
        };
        let icon = if is_dir {
            folder_icon(&node.name, open, 14.)
        } else {
            file_icon(&node.name, false, 14.)
        };
        row.child(
            div()
                .w(px(12.))
                .flex_shrink_0()
                .when(is_dir, |column| column.child(chevron(open))),
        )
        .child(icon)
        .child(div().min_w_0().truncate().child(node.name.clone()))
        .into_any_element()
    }

    /// The inline row where a new entry's name is typed; a note under it when the back end refuses.
    fn create_row(&self, creating: &Creating, depth: usize) -> AnyElement {
        let icon = file_icon("", creating.kind == "directory", 14.);
        let row = div()
            .h(px(30.))
            .w_full()
            .pl(px(8. + depth as f32 * 14.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .child(div().w(px(12.)).flex_shrink_0())
            .child(icon)
            .child(div().flex_1().min_w_0().child(creating.field.clone()));
        let mut block = div().flex().flex_col().child(row);
        if let Some(error) = &creating.error {
            block = block.child(note(error, depth, true));
        }
        block.into_any_element()
    }

    fn tool(
        id: &'static str,
        icon: &'static str,
        active: bool,
        cx: &mut Context<Self>,
        on_click: fn(&mut Self, &mut Context<Self>),
    ) -> AnyElement {
        div()
            .id(id)
            .flex_1()
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child(
                div()
                    .size(px(28.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .when(active, |button| button.bg(content(0.1)))
                    .hover(|style| style.bg(content(0.07)))
                    .child(
                        svg()
                            .path(icon)
                            .size(px(15.))
                            .text_color(content(if active { 0.9 } else { 0.6 }))
                            .flex_shrink_0(),
                    ),
            )
            .into_any_element()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h(px(40.))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(content(0.07))
            .children([
                Self::tool("files-new-file", "plus", false, cx, |this, cx| {
                    this.start_create("file", cx)
                }),
                Self::tool("files-new-folder", "folder-plus", false, cx, |this, cx| {
                    this.start_create("directory", cx)
                }),
                Self::tool(
                    "files-collapse",
                    "chevrons-down-up",
                    false,
                    cx,
                    |this, cx| this.collapse_all(cx),
                ),
                Self::tool(
                    "files-search",
                    "search",
                    self.search_open,
                    cx,
                    |this, cx| this.toggle_search(cx),
                ),
            ])
            .into_any_element()
    }

    /// The project heading: the folder's name, a chevron, and a click that folds the tree.
    fn heading(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("files-root")
            .h(px(30.))
            .w_full()
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .hover(|style| style.bg(content(0.05)))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_root(cx)))
            .child(
                div()
                    .w(px(12.))
                    .flex_shrink_0()
                    .child(chevron(self.root_open)),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.5))
                    .text_color(content(0.5))
                    .child(root_name(&self.root)),
            )
            .into_any_element()
    }

    fn tree(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = if self.root_open {
            self.tree_rows(&self.root, 0, cx)
        } else {
            Vec::new()
        };
        div()
            .id("files-tree")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(4.))
            .flex()
            .flex_col()
            .children(rows)
            .into_any_element()
    }

    /// What the search found, in place of the tree.
    fn results(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = if self.hits.is_empty() {
            let text = if self.searching || self.indexing && self.index.is_none() {
                t("Procurando…")
            } else {
                t("Nenhum arquivo encontrado.")
            };
            vec![note(text, 0, false)]
        } else {
            self.hits.iter().map(|hit| self.hit_row(hit, cx)).collect()
        };
        div()
            .id("files-results")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(4.))
            .flex()
            .flex_col()
            .children(rows)
            .into_any_element()
    }

    /// A result: the file's icon, its name and its folder, the matched letters in the accent color.
    fn hit_row(&self, hit: &Hit, cx: &mut Context<Self>) -> AnyElement {
        let name = &hit.path[hit.name_start..];
        let folder = hit.path[..hit.name_start].trim_end_matches('/');
        let relative = hit.path.clone();
        div()
            .id(format!("hit:{}", hit.path))
            .h(px(30.))
            .w_full()
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .hover(|style| style.bg(content(0.05)))
            .on_click(cx.listener(move |this, _, _, cx| this.open_hit(&relative, cx)))
            .child(file_icon(name, false, 14.))
            .child(
                div()
                    .flex_shrink_0()
                    .max_w_full()
                    .truncate()
                    .text_size(px(12.5))
                    .text_color(content(0.9))
                    .child(StyledText::new(name.to_owned()).with_highlights(marks(
                        name,
                        &hit.positions,
                        hit.name_start,
                    ))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(content(0.4))
                    .child(StyledText::new(folder.to_owned()).with_highlights(marks(
                        folder,
                        &hit.positions,
                        0,
                    ))),
            )
            .into_any_element()
    }

    /// The Terminal's tab strip, for files: a tab per open file (its icon, name and a close that
    /// shows on hover and on the tab on screen), and a `+` after them.
    fn tab_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs: Vec<AnyElement> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(ix, tab)| {
                let active = self.active == Some(ix);
                let group = format!("file-tab-{ix}");
                let close = div()
                    .id(("file-tab-close", ix))
                    .absolute()
                    .right_1()
                    .top(px(3.5))
                    .size(px(20.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(if active { 1. } else { 0. })
                    .group_hover(group.clone(), |s| s.opacity(1.))
                    .hover(|s| s.bg(content(0.07)))
                    .child(theme::icon("close", 12.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.request_close(ix, window, cx);
                    }));
                let modified = self.tab_modified(ix, cx);
                let plate = div()
                    .id(("file-tab-button", ix))
                    .relative()
                    .h(px(theme::PLATE))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(TAB_TILE_GAP))
                    .pl(px(tab_tile_inset()))
                    .pr(px(28.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(active, |d| d.bg(content(0.1)).text_color(content(1.)))
                    .when(!active, |d| d.bg(content(0.04)).text_color(content(0.5)))
                    .child(mark_tile(
                        &TAB_TILE,
                        Face::Neutral,
                        file_icon(&tab.name, false, 12.),
                    ))
                    .child(div().flex_1().min_w_0().truncate().child(tab.name.clone()))
                    .when(modified, |d| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .size(px(6.))
                                .rounded_full()
                                .bg(content(0.6)),
                        )
                    })
                    .child(close)
                    .on_click(cx.listener(move |this, _, _, cx| this.select_tab(ix, cx)));
                div()
                    .id(("file-tab", ix))
                    .group(group)
                    .h_full()
                    .w(px(224.))
                    .min_w_0()
                    .flex_shrink(1.)
                    .flex()
                    .items_center()
                    .child(plate)
                    .into_any_element()
            })
            .collect();
        let add = div()
            .id("new-file-tab")
            .group("new-file-tab")
            .cursor_pointer()
            .size(px(theme::PLATE))
            .flex_shrink_0()
            .ml(px(2.))
            .rounded(px(crate::theme::RADIUS_MD))
            .bg(content(0.04))
            .opacity(0.65)
            .flex()
            .items_center()
            .justify_center()
            .hover(|s| s.opacity(1.).bg(content(0.07)))
            .child(
                theme::icon("plus", 14.).group_hover("new-file-tab", |s| s.text_color(content(1.))),
            )
            .on_click(cx.listener(|this, _, _, cx| this.add_tab(cx)));
        div()
            .h(px(36.))
            .w_full()
            .flex_shrink_0()
            .min_w_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(content(0.08))
            .child(
                div()
                    .id("file-tabs")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .pl(px(6.))
                    .pr(px(10.))
                    .overflow_hidden()
                    .children(tabs)
                    .child(add),
            )
            .into_any_element()
    }

    /// project > path parts, each a step; the file's icon leads. Markdown adds the Preview / Source
    /// control on the right.
    fn breadcrumb(&self, opened: &Opened, cx: &mut Context<Self>) -> AnyElement {
        let relative = opened
            .path
            .strip_prefix(&self.root)
            .unwrap_or(&opened.path)
            .trim_start_matches('/');
        let mut steps: Vec<String> = vec![root_name(&self.root)];
        steps.extend(
            relative
                .split('/')
                .filter(|part| !part.is_empty())
                .map(str::to_owned),
        );
        let last = steps.len() - 1;
        let mut trail = div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(px(11.5))
            .child(file_icon(&opened.name, false, 13.));
        for (index, step) in steps.into_iter().enumerate() {
            if index > 0 {
                trail = trail.child(
                    svg()
                        .path("chevron-right")
                        .size(px(10.))
                        .text_color(content(0.3))
                        .flex_shrink_0(),
                );
            }
            let color = if index == last {
                content(0.85)
            } else {
                content(0.45)
            };
            trail = trail.child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .text_color(color)
                    .child(step),
            );
        }
        let mut bar = div()
            .h(px(30.))
            .w_full()
            .flex_shrink_0()
            .px(px(10.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .border_b_1()
            .border_color(content(0.07))
            .child(div().min_w_0().overflow_hidden().child(trail));
        if is_markdown(&opened.name) {
            bar = bar.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .p(px(2.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .border_1()
                    .border_color(content(0.07))
                    .child(Self::segment(
                        "files-preview",
                        "Preview",
                        Mode::Preview,
                        opened.mode,
                        cx,
                    ))
                    .child(Self::segment(
                        "files-source",
                        "Source",
                        Mode::Source,
                        opened.mode,
                        cx,
                    )),
            );
        }
        bar.into_any_element()
    }

    fn segment(
        id: &'static str,
        label: &'static str,
        target: Mode,
        active: Mode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = target == active;
        div()
            .id(id)
            .h(px(22.))
            .px(px(9.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .text_size(px(11.5))
            .text_color(content(if on { 0.95 } else { 0.5 }))
            .when(on, |segment| segment.bg(content(0.12)))
            .when(!on, |segment| {
                segment.hover(|style| style.bg(content(0.06)))
            })
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.set_mode(target, cx)))
            .child(label)
            .into_any_element()
    }
}

impl Render for FilesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut column = div().size_full().flex().flex_col();
        if !self.tabs.is_empty() {
            column = column.child(self.tab_strip(cx));
        }
        match self.active.and_then(|ix| self.tabs.get(ix)) {
            Some(opened) => column
                // An agent's capture is opened from its row, not browsed to: no path bar above it.
                .when(!tool_images::is_capture(&opened.path), |column| {
                    column.child(self.breadcrumb(opened, cx))
                })
                .child(self.viewer(opened, cx))
                .into_any_element(),
            None => {
                column = column.child(self.toolbar(cx));
                let searching = self.query(cx).is_some();
                if !searching {
                    column = column.child(self.heading(cx));
                }
                if self.search_open {
                    column = column.child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .px(px(8.))
                            .py(px(6.))
                            .child(self.search.clone()),
                    );
                }
                column
                    .child(if searching {
                        self.results(cx)
                    } else {
                        self.tree(cx)
                    })
                    .into_any_element()
            }
        }
    }
}

/// Resolves a back end request: the value, or the error's message.
async fn settle(rx: async_channel::Receiver<Result<Value, Value>>) -> Result<Value, String> {
    match rx.recv().await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(message(&error)),
        Err(_) => Err(t("O motor do Sirus não respondeu.").into()),
    }
}

fn message(error: &Value) -> String {
    error
        .as_str()
        .or(error["message"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| error.to_string())
}

/// A folder's entries: folders first, then by name.
fn nodes(value: &Value) -> Vec<Node> {
    let mut nodes: Vec<Node> = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(Node {
                path: entry["path"].as_str()?.to_owned(),
                name: entry["name"].as_str()?.to_owned(),
                is_dir: entry["isDir"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    nodes.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    nodes
}

fn root_name(root: &str) -> String {
    root.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(root)
        .to_owned()
}

/// The folder holding `path`, kept inside the root.
fn parent_dir(path: &str, root: &str) -> String {
    path.rsplit_once('/')
        .map(|(dir, _)| dir.to_owned())
        .filter(|dir| dir.len() >= root.len() && dir.starts_with(root))
        .unwrap_or_else(|| root.to_owned())
}

fn is_markdown(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".mdx")
}

fn chevron(open: bool) -> AnyElement {
    svg()
        .path("chevron-right")
        .size(px(12.))
        .text_color(content(0.45))
        .flex_shrink_0()
        .when(open, |icon| {
            icon.with_transformation(Transformation::rotate(radians(std::f32::consts::FRAC_PI_2)))
        })
        .into_any_element()
}

/// A status line in the tree (loading, failed) or the file view.
fn note(text: &str, depth: usize, failed: bool) -> AnyElement {
    let color: Hsla = if failed {
        rgb(0xe08a8a).into()
    } else {
        content(0.5)
    };
    div()
        .pl(px(24. + depth as f32 * 14.))
        .py(px(4.))
        .text_size(px(12.))
        .text_color(color)
        .child(text.to_owned())
        .into_any_element()
}

/// The matched letters of a search result: `positions` are byte offsets in the whole path, `text`
/// is the part of it that starts at `offset`.
fn marks(text: &str, positions: &[usize], offset: usize) -> Vec<(Range<usize>, HighlightStyle)> {
    let style = HighlightStyle {
        color: Some(accent()),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    positions
        .iter()
        .filter_map(|&at| {
            let start = at.checked_sub(offset)?;
            let len = text.get(start..)?.chars().next()?.len_utf8();
            Some((start..start + len, style))
        })
        .collect()
}

// ---- The file view -------------------------------------------------------------------------

fn text_viewer(path: &str, text: &str, cx: &mut Context<FilesPanel>) -> Viewer {
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let preview = ListState::new(1, ListAlignment::Top, px(240.));
    let source = ListState::new(lines.len(), ListAlignment::Top, px(480.));
    for state in [&preview, &source] {
        let weak = cx.weak_entity();
        state.set_scroll_handler(move |_, _, cx| {
            let _ = weak.update(cx, |_, cx| cx.notify());
        });
    }
    let (editor, events) = if text.len() <= MAX_EDITABLE_BYTES {
        let name = path.rsplit('/').next().unwrap_or(path).to_owned();
        let editor = cx.new(|cx| Editor::new(&name, text.to_owned(), cx));
        let path = path.to_owned();
        let events = cx.subscribe(
            &editor,
            move |this: &mut FilesPanel, _, event: &EditorEvent, cx| match event {
                EditorEvent::Save => this.save(path.clone(), cx),
                EditorEvent::Changed => cx.notify(),
            },
        );
        (Some(editor), Some(events))
    } else {
        (None, None)
    };
    Viewer::Text(Text {
        content: Arc::from(text),
        lines: Rc::new(lines),
        colors: Colors::Idle,
        preview,
        source,
        editor,
        _events: events,
    })
}

impl FilesPanel {
    fn viewer(&self, opened: &Opened, cx: &mut Context<Self>) -> AnyElement {
        let text = match &opened.viewer {
            Viewer::Loading => return body_note(t("Carregando…"), false),
            Viewer::Binary => {
                return body_note(
                    t("Arquivo binário: não dá para mostrar o conteúdo aqui."),
                    false,
                );
            }
            Viewer::Failed(error) => return body_note(error, true),
            Viewer::Picture(image) => {
                // In normal flow an image takes its height from its aspect ratio, so a tall one keeps
                // the panel's width and runs past the bottom. Absolute, it gets exactly this box.
                return div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .absolute()
                            .top(px(16.))
                            .bottom(px(16.))
                            .left(px(16.))
                            .right(px(16.))
                            .child(
                                img(image.clone())
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .size_full()
                                    .object_fit(ObjectFit::Contain),
                            ),
                    )
                    .into_any_element();
            }
            Viewer::Text(text) if text.lines.is_empty() => {
                return body_note(t("Arquivo vazio."), false);
            }
            Viewer::Text(text) => text,
        };
        if opened.mode == Mode::Source {
            if let Some(editor) = &text.editor {
                return div()
                    .flex_1()
                    .min_h_0()
                    .child(editor.clone())
                    .into_any_element();
            }
        }
        let (state, view) = match opened.mode {
            Mode::Preview => {
                let markdown = text.content.clone();
                let view = list(text.preview.clone(), move |_, _, _| {
                    div()
                        .px(px(28.))
                        .pt(px(32.))
                        .pb(px(40.))
                        .child(markdown::render(&markdown))
                        .into_any_element()
                });
                (text.preview.clone(), view)
            }
            Mode::Source => {
                let lines = text.lines.clone();
                let colors = match &text.colors {
                    Colors::Ready(spans) => Some(spans.clone()),
                    _ => None,
                };
                let view = list(text.source.clone(), move |index, _, _| {
                    source_line(
                        index,
                        &lines[index],
                        colors.as_ref().and_then(|spans| spans.get(index)),
                    )
                    .into_any_element()
                });
                (text.source.clone(), view)
            }
        };
        let at_end = state.is_scrolled_to_end();
        let to_end = div()
            .id("files-to-end")
            .absolute()
            .right(px(16.))
            .bottom(px(16.))
            .size(px(28.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(theme::floating_fill())
            .border_1()
            .border_color(content(0.12))
            .hover(|style| style.bg(content(0.12)))
            .on_click(cx.listener(move |_, _, _, cx| {
                state.scroll_to_end();
                cx.notify();
            }))
            .child(
                svg()
                    .path("arrow-up")
                    .size(px(13.))
                    .text_color(content(0.75))
                    .flex_shrink_0()
                    .with_transformation(Transformation::rotate(radians(std::f32::consts::PI))),
            );
        let fade = div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(18.))
            .bg(pane_fade());
        let mut body = div().relative().flex_1().min_h_0().flex().flex_col();
        if opened.mode == Mode::Source && text.content.len() > MAX_EDITABLE_BYTES {
            body = body.child(note(
                t("Arquivo grande demais para editar: somente leitura."),
                0,
                false,
            ));
        }
        let mut body = body.child(view.flex_1().min_h_0()).child(fade);
        if at_end == Some(false) {
            body = body.child(to_end);
        }
        body.into_any_element()
    }
}

/// The text at the top of a file view when there is nothing to show.
fn body_note(text: &str, failed: bool) -> AnyElement {
    let color: Hsla = if failed {
        rgb(0xe08a8a).into()
    } else {
        content(0.5)
    };
    div()
        .flex_1()
        .min_h_0()
        .px(px(16.))
        .py(px(10.))
        .text_size(px(12.))
        .text_color(color)
        .child(text.to_owned())
        .into_any_element()
}

fn pane_fade() -> Background {
    let solid = theme::pane_fill();
    let clear = Hsla { a: 0., ..solid };
    linear_gradient(
        180.,
        linear_color_stop(solid, 0.),
        linear_color_stop(clear, 1.),
    )
}

/// One line of the source view: its number in a gutter, then the text, which wraps under the
/// gutter's top so the number stays on the line's first row. `spans` are its syntax colors, once
/// parsed.
pub(crate) fn source_line(index: usize, text: &str, spans: Option<&LineSpans>) -> gpui::Div {
    let shown = if text.is_empty() {
        " ".to_owned()
    } else {
        text.to_owned()
    };
    let highlights: Vec<(Range<usize>, HighlightStyle)> = spans
        .into_iter()
        .flatten()
        .filter(|(range, _)| range.end <= text.len())
        .map(|(range, group)| {
            (
                range.clone(),
                HighlightStyle {
                    color: Some(group_color(*group)),
                    ..Default::default()
                },
            )
        })
        .collect();
    div()
        .min_h(px(19.))
        .w_full()
        .flex()
        .items_start()
        .font_family("Menlo")
        .text_size(px(12.5))
        .line_height(px(19.))
        .child(
            div()
                .w(px(48.))
                .flex_shrink_0()
                .pr(px(14.))
                .text_right()
                .text_color(content(0.3))
                .child((index + 1).to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pr(px(16.))
                .text_color(content(0.88))
                .child(StyledText::new(shown).with_highlights(highlights)),
        )
}

/// Monocode's `--syntax-*` colors (src/styles/index.css), dark and light.
pub(crate) fn group_color(group: Group) -> Hsla {
    let (dark, light) = match group {
        Group::Keyword => (0x569cd6, 0x0000ff),
        Group::Callable => (0xdcdcaa, 0x795e26),
        Group::String => (0xce9178, 0xa31515),
        Group::Type => (0x4ec9b0, 0x267f99),
        Group::Tag => (0x569cd6, 0x800000),
        Group::Number => (0xb5cea8, 0x098658),
        Group::Comment => (0x6a9955, 0x008000),
        Group::Property => (0x9cdcfe, 0x001080),
        Group::Meta => (0xdcdcaa, 0x795e26),
    };
    rgb(if is_light() { light } else { dark }).into()
}

// ---- A one-line text field typed through GPUI's input handler ---------------------------------

/// The events a text field sends its owner: Enter submits, Escape cancels, typing changes.
#[derive(Clone, Copy)]
pub enum TextFieldEvent {
    Changed,
    Submit,
    Cancel,
}

/// A single-line field: the platform's input method types into it (insertions, marked text,
/// selection), and the keys below move the caret and edit. The browser's address bar works the same.
pub struct TextField {
    text: String,
    anchor: usize,
    cursor: usize,
    marked: Option<Range<usize>>,
    placeholder: &'static str,
    focus: FocusHandle,
    focus_pending: bool,
    /// Shows bullets instead of the text and never copies it (a password).
    masked: bool,
}

impl EventEmitter<TextFieldEvent> for TextField {}

impl TextField {
    pub fn new(placeholder: &'static str, cx: &mut Context<Self>) -> Self {
        Self {
            text: String::new(),
            anchor: 0,
            cursor: 0,
            marked: None,
            placeholder,
            focus: cx.focus_handle(),
            focus_pending: false,
            masked: false,
        }
    }

    /// The field's focus handle, to tell whether it is being typed in.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// A password field: bullets on screen, nothing copied out.
    pub fn set_masked(&mut self, masked: bool) {
        self.masked = masked;
    }

    /// What is drawn for a part of the text.
    fn shown(&self, range: Range<usize>) -> String {
        if self.masked {
            "•".repeat(self.text[range].chars().count())
        } else {
            self.text[range].to_owned()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.text = text.to_owned();
        self.anchor = self.text.len();
        self.cursor = self.anchor;
        self.marked = None;
        cx.notify();
    }

    /// Takes the keyboard on the next frame.
    pub fn focus(&mut self, cx: &mut Context<Self>) {
        self.focus_pending = true;
        cx.notify();
    }

    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    fn insert(&mut self, text: &str) {
        let r = self.selection();
        self.text.replace_range(r.clone(), text);
        self.cursor = r.start + text.len();
        self.anchor = self.cursor;
    }

    fn step(&self, from: usize, forward: bool) -> usize {
        if forward {
            self.text[from..]
                .chars()
                .next()
                .map_or(from, |c| from + c.len_utf8())
        } else {
            self.text[..from]
                .chars()
                .next_back()
                .map_or(from, |c| from - c.len_utf8())
        }
    }

    fn move_to(&mut self, to: usize, extend: bool) {
        self.cursor = to;
        if !extend {
            self.anchor = to;
        }
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let (shift, command) = (keys.modifiers.shift, keys.modifiers.platform);
        let sel = self.selection();
        let before = self.text.clone();
        match keys.key.as_str() {
            "enter" => {
                cx.emit(TextFieldEvent::Submit);
                cx.stop_propagation();
                return;
            }
            "escape" => {
                cx.emit(TextFieldEvent::Cancel);
                cx.stop_propagation();
                return;
            }
            "backspace" if !sel.is_empty() => self.insert(""),
            "backspace" => {
                self.anchor = self.step(self.cursor, false);
                self.insert("");
            }
            "delete" if !sel.is_empty() => self.insert(""),
            "delete" => {
                self.anchor = self.step(self.cursor, true);
                self.insert("");
            }
            "left" if command => self.move_to(0, shift),
            "right" if command => self.move_to(self.text.len(), shift),
            "left" if !sel.is_empty() && !shift => self.move_to(sel.start, false),
            "right" if !sel.is_empty() && !shift => self.move_to(sel.end, false),
            "left" => self.move_to(self.step(self.cursor, false), shift),
            "right" => self.move_to(self.step(self.cursor, true), shift),
            "home" => self.move_to(0, shift),
            "end" => self.move_to(self.text.len(), shift),
            "a" if command => {
                self.anchor = 0;
                self.cursor = self.text.len();
            }
            "c" if command => {
                if !self.masked {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                        self.text[sel.clone()].to_owned(),
                    ))
                }
            }
            "x" if command => {
                if !self.masked {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                        self.text[sel.clone()].to_owned(),
                    ));
                }
                self.insert("");
            }
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.insert(&text.replace(['\n', '\r'], ""));
                }
            }
            _ => return,
        }
        self.marked = None;
        if self.text != before {
            cx.emit(TextFieldEvent::Changed);
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_pending {
            self.focus_pending = false;
            window.focus(&self.focus, cx);
        }
        let focused = self.focus.is_focused(window);
        let sel = self.selection();
        let segment = |s: &str| {
            div()
                .flex_shrink_0()
                .whitespace_nowrap()
                .child(s.to_owned())
        };
        let body = if self.text.is_empty() && !focused {
            div()
                .text_color(content(0.35))
                .child(self.placeholder)
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .child(segment(&self.shown(0..sel.start)))
                .child(if sel.is_empty() {
                    div()
                        .w(px(1.))
                        .h(px(15.))
                        .flex_shrink_0()
                        .bg(if focused {
                            accent()
                        } else {
                            gpui::transparent_black()
                        })
                        .into_any_element()
                } else {
                    segment(&self.shown(sel.clone()))
                        .bg(theme::text_selection())
                        .into_any_element()
                })
                .child(segment(&self.shown(sel.end..self.text.len())))
                .into_any_element()
        };
        let entity = cx.entity();
        let focus = self.focus.clone();
        div()
            .id("text-field")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    this.move_to(this.text.len(), false);
                    cx.notify();
                }),
            )
            .relative()
            .w_full()
            .h(px(26.))
            .px(px(9.))
            .rounded(px(crate::theme::RADIUS_MD))
            .bg(theme::field_fill())
            .overflow_hidden()
            .flex()
            .items_center()
            .text_size(px(12.5))
            .text_color(content(0.9))
            .child(body)
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                    },
                )
                .absolute()
                .size_full(),
            )
    }
}

pub(crate) fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

pub(crate) fn byte_at(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (byte, c) in text.char_indices() {
        if units >= utf16 {
            return byte;
        }
        units += c.len_utf16();
    }
    text.len()
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let (start, end) = (
            byte_at(&self.text, range.start),
            byte_at(&self.text, range.end),
        );
        *adjusted = Some(range);
        Some(self.text[start..end].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let r = self.selection();
        Some(UTF16Selection {
            range: utf16_len(&self.text[..r.start])..utf16_len(&self.text[..r.end]),
            reversed: self.cursor < self.anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone()
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let marked = self.marked.take();
        if let Some(range) = range.or(marked) {
            self.anchor = byte_at(&self.text, range.start);
            self.cursor = byte_at(&self.text, range.end);
        }
        self.insert(&text.replace(['\n', '\r'], ""));
        cx.emit(TextFieldEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let marked = self.marked.take();
        if let Some(range) = range.or(marked) {
            self.anchor = byte_at(&self.text, range.start);
            self.cursor = byte_at(&self.text, range.end);
        }
        let start = utf16_len(&self.text[..self.selection().start]);
        self.insert(new_text);
        self.marked = (!new_text.is_empty()).then(|| start..start + utf16_len(new_text));
        cx.emit(TextFieldEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: gpui::Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<gpui::Bounds<Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
