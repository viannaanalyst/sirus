//! Conversations side by side (split-layout.ts, SplitWorkspace.tsx, split-drag.ts of Sirus Code).
//!
//! The layout is a binary tree of panes; every pane shows one session. The active pane always
//! shows the selected tab, so the composer, the dock, the Environment card and the shortcuts keep
//! following one conversation. Each pane draws its own transcript and composer by naming its
//! conversation while it is built (`Shell::active_conversation` honours `State::building`), and a
//! press inside a pane makes it the active one before the press is handled.
//!
//! Opening a pane slides it in from its edge while the other panes shrink, 320 ms with
//! cubic-bezier(.22, 1, .36, 1); closing mirrors it; reduced motion cuts it. Frames are only
//! requested while that runs.
use super::*;
use crate::motion::{Curve, Tween, eased};
use crate::theme::{accent_wash, floating_fill, pane_fill};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

/// Sirus Code's limit of panes.
pub const MAX_PANES: usize = 4;
/// A divider never leaves less than this share to either side.
pub const MIN_RATIO: f32 = 0.2;
/// Seconds a pane takes to slide in or out.
pub const SLIDE: f32 = 0.32;
/// Width of the strip along the area that splits the whole area once there are several panes.
pub const AREA_EDGE: f32 = 28.;
/// Height of a pane's header and the gap around a pane, as in Sirus.
const HEADER: f32 = 34.;
const GAP: f32 = 4.;
const DEPTH_LIMIT: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

impl Edge {
    fn opposite(self) -> Edge {
        match self {
            Edge::Left => Edge::Right,
            Edge::Right => Edge::Left,
            Edge::Top => Edge::Bottom,
            Edge::Bottom => Edge::Top,
            Edge::Center => Edge::Center,
        }
    }
}

/// Where a dropped conversation lands: a pane, or the whole area.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Root,
    Pane(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropState {
    Ok,
    /// The pane limit refuses it.
    Full,
    /// Nothing would change.
    Nothing,
}

/// A box in fractions (0 to 1) of the panes area.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub const WHOLE: Rect = Rect {
    x: 0.,
    y: 0.,
    w: 1.,
    h: 1.,
};

impl Rect {
    /// The box shrunk to nothing against one of its edges.
    fn collapsed(self, edge: Edge) -> Rect {
        match edge {
            Edge::Left => Rect { w: 0., ..self },
            Edge::Right => Rect {
                x: self.x + self.w,
                w: 0.,
                ..self
            },
            Edge::Top => Rect { h: 0., ..self },
            Edge::Bottom => Rect {
                y: self.y + self.h,
                h: 0.,
                ..self
            },
            Edge::Center => self,
        }
    }

    fn lerp(self, to: Rect, t: f32) -> Rect {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Rect {
            x: mix(self.x, to.x),
            y: mix(self.y, to.y),
            w: mix(self.w, to.w),
            h: mix(self.h, to.h),
        }
    }

    fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }

    /// The part of the box a drop on `edge` would take.
    pub fn half(self, edge: Edge) -> Rect {
        match edge {
            Edge::Left => Rect {
                w: self.w / 2.,
                ..self
            },
            Edge::Right => Rect {
                x: self.x + self.w / 2.,
                w: self.w / 2.,
                ..self
            },
            Edge::Top => Rect {
                h: self.h / 2.,
                ..self
            },
            Edge::Bottom => Rect {
                y: self.y + self.h / 2.,
                h: self.h / 2.,
                ..self
            },
            Edge::Center => self,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Node {
    Leaf {
        id: u32,
        session: String,
    },
    Split {
        id: u32,
        row: bool,
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

#[derive(Clone, PartialEq, Debug)]
pub struct Layout {
    root: Node,
    active: u32,
    next: u32,
}

/// A divider: the branch, its direction, its share and the box it splits.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Divider {
    pub id: u32,
    pub row: bool,
    pub ratio: f32,
    pub rect: Rect,
}

fn collect<'a>(node: &'a Node, out: &mut Vec<(u32, &'a str)>) {
    match node {
        Node::Leaf { id, session } => out.push((*id, session)),
        Node::Split { a, b, .. } => {
            collect(a, out);
            collect(b, out);
        }
    }
}

fn remove_node(node: &Node, leaf: u32) -> Option<Node> {
    match node {
        Node::Leaf { id, .. } => (*id != leaf).then(|| node.clone()),
        Node::Split {
            id,
            row,
            ratio,
            a,
            b,
        } => match (remove_node(a, leaf), remove_node(b, leaf)) {
            (None, other) | (other, None) => other,
            (Some(a), Some(b)) => Some(Node::Split {
                id: *id,
                row: *row,
                ratio: *ratio,
                a: Box::new(a),
                b: Box::new(b),
            }),
        },
    }
}

fn insert_node(node: &Node, target: Target, edge: Edge, leaf: &Node, split_id: u32) -> Node {
    let wrap = |node: &Node| {
        let first = matches!(edge, Edge::Left | Edge::Top);
        let (a, b) = if first {
            (leaf.clone(), node.clone())
        } else {
            (node.clone(), leaf.clone())
        };
        Node::Split {
            id: split_id,
            row: matches!(edge, Edge::Left | Edge::Right),
            ratio: 0.5,
            a: Box::new(a),
            b: Box::new(b),
        }
    };
    if target == Target::Root {
        return wrap(node);
    }
    match node {
        Node::Leaf { id, .. } if target == Target::Pane(*id) => wrap(node),
        Node::Leaf { .. } => node.clone(),
        Node::Split {
            id,
            row,
            ratio,
            a,
            b,
        } => Node::Split {
            id: *id,
            row: *row,
            ratio: *ratio,
            a: Box::new(insert_node(a, target, edge, leaf, split_id)),
            b: Box::new(insert_node(b, target, edge, leaf, split_id)),
        },
    }
}

fn map_sessions(node: &Node, map: &dyn Fn(u32, &str) -> String) -> Node {
    match node {
        Node::Leaf { id, session } => Node::Leaf {
            id: *id,
            session: map(*id, session),
        },
        Node::Split {
            id,
            row,
            ratio,
            a,
            b,
        } => Node::Split {
            id: *id,
            row: *row,
            ratio: *ratio,
            a: Box::new(map_sessions(a, map)),
            b: Box::new(map_sessions(b, map)),
        },
    }
}

fn sibling_of<'a>(node: &'a Node, leaf: u32) -> Option<&'a Node> {
    let Node::Split { a, b, .. } = node else {
        return None;
    };
    if matches!(**a, Node::Leaf { id, .. } if id == leaf) {
        return Some(b);
    }
    if matches!(**b, Node::Leaf { id, .. } if id == leaf) {
        return Some(a);
    }
    sibling_of(a, leaf).or_else(|| sibling_of(b, leaf))
}

fn hug_of(node: &Node, leaf: u32) -> Option<Edge> {
    let Node::Split { row, a, b, .. } = node else {
        return None;
    };
    if matches!(**a, Node::Leaf { id, .. } if id == leaf) {
        return Some(if *row { Edge::Left } else { Edge::Top });
    }
    if matches!(**b, Node::Leaf { id, .. } if id == leaf) {
        return Some(if *row { Edge::Right } else { Edge::Bottom });
    }
    hug_of(a, leaf).or_else(|| hug_of(b, leaf))
}

fn first_leaf(node: &Node) -> u32 {
    match node {
        Node::Leaf { id, .. } => *id,
        Node::Split { a, .. } => first_leaf(a),
    }
}

fn lay_out(node: &Node, rect: Rect, panes: &mut Vec<(u32, Rect)>, dividers: &mut Vec<Divider>) {
    match node {
        Node::Leaf { id, .. } => panes.push((*id, rect)),
        Node::Split {
            id,
            row,
            ratio,
            a,
            b,
        } => {
            let (first, second) = if *row {
                let w = rect.w * ratio;
                (
                    Rect { w, ..rect },
                    Rect {
                        x: rect.x + w,
                        w: rect.w - w,
                        ..rect
                    },
                )
            } else {
                let h = rect.h * ratio;
                (
                    Rect { h, ..rect },
                    Rect {
                        y: rect.y + h,
                        h: rect.h - h,
                        ..rect
                    },
                )
            };
            dividers.push(Divider {
                id: *id,
                row: *row,
                ratio: *ratio,
                rect,
            });
            lay_out(a, first, panes, dividers);
            lay_out(b, second, panes, dividers);
        }
    }
}

fn resize_node(node: &Node, branch: u32, ratio: f32) -> Node {
    match node {
        Node::Leaf { .. } => node.clone(),
        Node::Split {
            id,
            row,
            ratio: held,
            a,
            b,
        } => Node::Split {
            id: *id,
            row: *row,
            ratio: if *id == branch { ratio } else { *held },
            a: Box::new(resize_node(a, branch, ratio)),
            b: Box::new(resize_node(b, branch, ratio)),
        },
    }
}

impl Layout {
    pub fn single(session: &str) -> Layout {
        Layout {
            root: Node::Leaf {
                id: 1,
                session: session.to_owned(),
            },
            active: 1,
            next: 2,
        }
    }

    pub fn leaves(&self) -> Vec<(u32, &str)> {
        let mut out = Vec::new();
        collect(&self.root, &mut out);
        out
    }

    pub fn count(&self) -> usize {
        self.leaves().len()
    }

    pub fn active(&self) -> u32 {
        self.active
    }

    pub fn session_of(&self, leaf: u32) -> Option<&str> {
        self.leaves()
            .into_iter()
            .find(|(id, _)| *id == leaf)
            .map(|(_, s)| s)
    }

    pub fn active_session(&self) -> &str {
        self.session_of(self.active).unwrap_or("")
    }

    /// The pane already showing this session.
    pub fn showing(&self, session: &str) -> Option<u32> {
        self.leaves()
            .into_iter()
            .find(|(_, s)| *s == session)
            .map(|(id, _)| id)
    }

    /// Whether dropping `session` at `target` changes anything, and whether the pane limit refuses it.
    pub fn drop_state(&self, session: &str, target: Target, edge: Edge) -> DropState {
        let leaves = self.leaves();
        let from = self.showing(session);
        if from.is_some() && from.map(Target::Pane) == Some(target) {
            return DropState::Nothing;
        }
        if let Target::Pane(id) = target {
            if !leaves.iter().any(|(leaf, _)| *leaf == id) {
                return DropState::Nothing;
            }
        }
        if edge == Edge::Center {
            return if target == Target::Root {
                DropState::Nothing
            } else {
                DropState::Ok
            };
        }
        if from.is_some() {
            return if leaves.len() < 2 {
                DropState::Nothing
            } else {
                DropState::Ok
            };
        }
        if leaves.len() >= MAX_PANES {
            DropState::Full
        } else {
            DropState::Ok
        }
    }

    /// Drops a conversation on a pane edge (a new pane) or its center (replaces it; a conversation
    /// already open swaps places). The dropped pane becomes the active one.
    pub fn dropped(&self, session: &str, target: Target, edge: Edge) -> Option<Layout> {
        if self.drop_state(session, target, edge) != DropState::Ok {
            return None;
        }
        let from = self.showing(session);
        if edge == Edge::Center {
            let Target::Pane(into) = target else {
                return None;
            };
            let replaced = self.session_of(into)?.to_owned();
            let session = session.to_owned();
            let root = map_sessions(&self.root, &|id, held| {
                if id == into {
                    session.clone()
                } else if Some(id) == from {
                    replaced.clone()
                } else {
                    held.to_owned()
                }
            });
            return Some(Layout {
                root,
                active: into,
                next: self.next,
            });
        }
        let mut next = self.next;
        let leaf_id = from.unwrap_or_else(|| {
            next += 1;
            next - 1
        });
        let base = match from {
            Some(id) => remove_node(&self.root, id)?,
            None => self.root.clone(),
        };
        let leaf = Node::Leaf {
            id: leaf_id,
            session: session.to_owned(),
        };
        let split_id = next;
        next += 1;
        Some(Layout {
            root: insert_node(&base, target, edge, &leaf, split_id),
            active: leaf_id,
            next,
        })
    }

    /// Sirus' `openInSplit`: beside the active pane, or beside the other pane when the session is
    /// the active one already.
    pub fn opened_beside(&self, session: &str, edge: Edge) -> Option<Layout> {
        let from = self.showing(session);
        let target = if from == Some(self.active) {
            self.leaves()
                .into_iter()
                .map(|(id, _)| id)
                .find(|id| Some(*id) != from)?
        } else {
            self.active
        };
        self.dropped(session, Target::Pane(target), edge)
    }

    /// Removes a pane; when it was active, the pane that takes its space becomes active.
    pub fn removed(&self, leaf: u32) -> Option<Layout> {
        if self.count() < 2 || self.session_of(leaf).is_none() {
            return None;
        }
        let sibling = sibling_of(&self.root, leaf).map(first_leaf);
        let root = remove_node(&self.root, leaf)?;
        let active = if self.active == leaf {
            sibling.unwrap_or_else(|| first_leaf(&root))
        } else {
            self.active
        };
        Some(Layout {
            root,
            active,
            next: self.next,
        })
    }

    /// Closes the panes whose session `live` refuses (the last one stays).
    pub fn pruned(&self, live: &dyn Fn(&str) -> bool) -> Layout {
        let mut layout = self.clone();
        for (id, session) in self.leaves() {
            if !live(session) {
                if let Some(next) = layout.removed(id) {
                    layout = next;
                }
            }
        }
        layout
    }

    /// Moves a divider; the share stays between `MIN_RATIO` and its complement.
    pub fn resized(&self, branch: u32, ratio: f32) -> Option<Layout> {
        let ratio = ratio.clamp(MIN_RATIO, 1. - MIN_RATIO);
        let (_, dividers) = self.rects();
        let held = dividers.iter().find(|d| d.id == branch)?.ratio;
        if (held - ratio).abs() < 1e-6 {
            return None;
        }
        Some(Layout {
            root: resize_node(&self.root, branch, ratio),
            ..self.clone()
        })
    }

    /// Keeps the layout in step with the selected tab: a session already shown activates its pane,
    /// otherwise the active pane switches to it. True when something changed.
    pub fn select(&mut self, session: &str) -> bool {
        if self.active_session() == session {
            return false;
        }
        if let Some(shown) = self.showing(session) {
            self.active = shown;
            return true;
        }
        let active = self.active;
        self.root = map_sessions(&self.root, &|id, held| {
            if id == active {
                session.to_owned()
            } else {
                held.to_owned()
            }
        });
        true
    }

    pub fn rects(&self) -> (Vec<(u32, Rect)>, Vec<Divider>) {
        let (mut panes, mut dividers) = (Vec::new(), Vec::new());
        lay_out(&self.root, WHOLE, &mut panes, &mut dividers);
        (panes, dividers)
    }

    /// The edge a pane hugs when it closes: the side it sits on within its parent.
    pub fn hug_edge(&self, leaf: u32) -> Edge {
        hug_of(&self.root, leaf).unwrap_or(Edge::Left)
    }

    pub fn to_json(&self) -> Value {
        fn node(n: &Node) -> Value {
            match n {
                Node::Leaf { id, session } => json!({ "leaf": id, "session": session }),
                Node::Split {
                    id,
                    row,
                    ratio,
                    a,
                    b,
                } => json!({
                    "split": id,
                    "dir": if *row { "row" } else { "column" },
                    "ratio": ratio,
                    "a": node(a),
                    "b": node(b),
                }),
            }
        }
        json!({ "version": 1, "active": self.active, "next": self.next, "root": node(&self.root) })
    }

    /// Reads a saved layout; anything malformed, over the limit or with a repeated id is refused.
    pub fn from_json(value: &Value) -> Option<Layout> {
        fn node(v: &Value, depth: usize, ids: &mut Vec<u32>, leaves: &mut usize) -> Option<Node> {
            if depth > DEPTH_LIMIT {
                return None;
            }
            if let Some(id) = v["leaf"].as_u64() {
                let id = u32::try_from(id).ok()?;
                let session = v["session"].as_str().filter(|s| !s.is_empty())?.to_owned();
                if ids.contains(&id) {
                    return None;
                }
                ids.push(id);
                *leaves += 1;
                return Some(Node::Leaf { id, session });
            }
            let id = u32::try_from(v["split"].as_u64()?).ok()?;
            if ids.contains(&id) {
                return None;
            }
            ids.push(id);
            let row = match v["dir"].as_str()? {
                "row" => true,
                "column" => false,
                _ => return None,
            };
            let ratio = v["ratio"].as_f64()? as f32;
            if !ratio.is_finite() {
                return None;
            }
            Some(Node::Split {
                id,
                row,
                ratio: ratio.clamp(MIN_RATIO, 1. - MIN_RATIO),
                a: Box::new(node(&v["a"], depth + 1, ids, leaves)?),
                b: Box::new(node(&v["b"], depth + 1, ids, leaves)?),
            })
        }
        if value["version"].as_u64()? != 1 {
            return None;
        }
        let (mut ids, mut leaves) = (Vec::new(), 0);
        let root = node(&value["root"], 0, &mut ids, &mut leaves)?;
        let active = u32::try_from(value["active"].as_u64()?).ok()?;
        let next = u32::try_from(value["next"].as_u64()?)
            .ok()?
            .max(ids.iter().max()? + 1);
        let layout = Layout { root, active, next };
        let mut sessions: Vec<&str> = layout.leaves().into_iter().map(|(_, s)| s).collect();
        sessions.sort_unstable();
        sessions.dedup();
        (leaves <= MAX_PANES && sessions.len() == leaves && layout.session_of(active).is_some())
            .then_some(layout)
    }
}

/// Which part of a pane a point is over: the middle replaces, otherwise the nearest edge.
pub fn edge_at(nx: f32, ny: f32) -> Edge {
    if (nx - 0.5).abs().max((ny - 0.5).abs()) < 0.2 {
        return Edge::Center;
    }
    [
        (Edge::Left, nx),
        (Edge::Right, 1. - nx),
        (Edge::Top, ny),
        (Edge::Bottom, 1. - ny),
    ]
    .into_iter()
    .fold((Edge::Left, f32::MAX), |best, item| {
        if item.1 < best.1 { item } else { best }
    })
    .0
}

/// What a point of the area (in fractions, with the area's size in pixels) would drop onto.
pub fn target_at(
    panes: &[(u32, Rect)],
    nx: f32,
    ny: f32,
    width: f32,
    height: f32,
) -> Option<(Target, Edge)> {
    if panes.len() > 1 {
        let edge = if nx * width < AREA_EDGE {
            Some(Edge::Left)
        } else if (1. - nx) * width < AREA_EDGE {
            Some(Edge::Right)
        } else if ny * height < AREA_EDGE {
            Some(Edge::Top)
        } else if (1. - ny) * height < AREA_EDGE {
            Some(Edge::Bottom)
        } else {
            None
        };
        if let Some(edge) = edge {
            return Some((Target::Root, edge));
        }
    }
    let (id, rect) = panes.iter().find(|(_, r)| r.contains(nx, ny))?;
    Some((
        Target::Pane(*id),
        edge_at((nx - rect.x) / rect.w, (ny - rect.y) / rect.h),
    ))
}

fn file() -> PathBuf {
    crate::backend::data_dir().join("split-layout.json")
}

pub fn write(path: &std::path::Path, layout: Option<&Layout>) {
    match layout {
        Some(layout) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, layout.to_json().to_string());
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn read(path: &std::path::Path) -> Option<Layout> {
    let text = std::fs::read_to_string(path).ok()?;
    Layout::from_json(&serde_json::from_str(&text).ok()?)
}

/// A conversation dragged from the sidebar, a tab or a pane header.
#[derive(Clone)]
pub struct ConvDrag {
    pub session: String,
    pub title: String,
}

pub struct DragChip(String);

impl DragChip {
    pub fn new(title: String) -> Self {
        DragChip(title)
    }
}

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(28.))
            .px(px(12.))
            .rounded(px(crate::theme::RADIUS_MD))
            .flex()
            .items_center()
            .bg(floating_fill())
            .border_1()
            .border_color(content(0.14))
            .text_size(px(13.))
            .text_color(content(0.9))
            .child(self.0.clone())
    }
}

/// A pane that is leaving: its box when it started, the edge it shrinks toward.
struct Ghost {
    session: String,
    rect: Rect,
    hug: Edge,
}

struct Anim {
    start: Instant,
    /// Where the surviving panes were.
    from: Vec<(u32, Rect)>,
    /// New panes and the edge they slide in from.
    enter: Vec<(u32, Edge)>,
    leave: Vec<Ghost>,
    /// How much of the frame and header show, at the start and at the end.
    framed: (f32, f32),
}

impl Anim {
    fn progress(&self, now: Instant) -> f32 {
        (now.duration_since(self.start).as_secs_f32() / SLIDE).clamp(0., 1.)
    }
}

#[derive(Default)]
pub struct State {
    layout: Option<Layout>,
    /// What the dragged conversation would land on, while one is dragged over the area.
    hover: Option<(Target, Edge, DropState)>,
    anim: Option<Anim>,
    resizing: Option<u32>,
    area: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The "latest" button's tween, per conversation, so panes do not fight over one.
    jumps: HashMap<usize, Tween>,
    /// The conversation a pane is being built for.
    building: Cell<Option<usize>>,
    restored: bool,
}

impl State {
    pub fn building(&self) -> Option<usize> {
        self.building.get()
    }

    /// The conversations in the panes now, none without a split.
    pub fn sessions(&self) -> Vec<String> {
        self.layout
            .as_ref()
            .map(|layout| {
                layout
                    .leaves()
                    .into_iter()
                    .map(|(_, s)| s.to_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// One pane as drawn this frame.
struct PaneBox {
    id: u32,
    session: String,
    rect: Rect,
    /// A sliding pane keeps its final size and lines up with this edge of its box.
    slide: Option<(Edge, Rect)>,
    ghost: bool,
}

fn geometry(layout: &Layout, anim: Option<&Anim>, e: f32) -> Vec<PaneBox> {
    let (rects, _) = layout.rects();
    let mut boxes: Vec<PaneBox> = Vec::new();
    for (id, to) in rects {
        let session = layout.session_of(id).unwrap_or("").to_owned();
        let (from, slide) = match anim {
            Some(a) => match (
                a.from.iter().find(|(f, _)| *f == id),
                a.enter.iter().find(|(f, _)| *f == id),
            ) {
                (Some((_, rect)), _) => (*rect, None),
                (None, Some((_, edge))) => (to.collapsed(*edge), Some((edge.opposite(), to))),
                (None, None) => (to, None),
            },
            None => (to, None),
        };
        boxes.push(PaneBox {
            id,
            session,
            rect: from.lerp(to, e),
            slide,
            ghost: false,
        });
    }
    if let Some(a) = anim {
        for (n, ghost) in a.leave.iter().enumerate() {
            boxes.push(PaneBox {
                id: u32::MAX - n as u32,
                session: ghost.session.clone(),
                rect: ghost.rect.lerp(ghost.rect.collapsed(ghost.hug), e),
                slide: Some((ghost.hug.opposite(), ghost.rect)),
                ghost: true,
            });
        }
    }
    boxes
}

impl Shell {
    fn session_index(&self, session: &str) -> Option<usize> {
        self.conversations
            .iter()
            .position(|c| !session.is_empty() && c.session_id == session)
    }

    fn session_alive(&self, session: &str) -> bool {
        self.session_index(session)
            .is_some_and(|ix| self.project_of(ix).is_some())
    }

    fn split_save(&self) {
        write(&file(), self.split.layout.as_ref());
    }

    /// The layout as the next change starts from: the open one, or the active conversation alone.
    fn split_base(&self) -> Option<Layout> {
        match &self.split.layout {
            Some(layout) => Some(layout.clone()),
            None => {
                let session = self.active_session_id();
                (!session.is_empty()).then(|| Layout::single(&session))
            }
        }
    }

    /// How much of the frame and header show right now.
    fn split_framed(&self, now: Instant) -> f32 {
        if self.split.layout.is_none() {
            return 0.;
        }
        match &self.split.anim {
            Some(a) => a.framed.0 + (a.framed.1 - a.framed.0) * eased(a.progress(now), Curve::Expo),
            None => 1.,
        }
    }

    /// Where every pane is this frame, sliding ones included.
    fn split_current(&self, now: Instant) -> Vec<(u32, Rect)> {
        let Some(layout) = &self.split.layout else {
            return vec![];
        };
        let e = self
            .split
            .anim
            .as_ref()
            .map_or(1., |a| eased(a.progress(now), Curve::Expo));
        geometry(layout, self.split.anim.as_ref(), e)
            .into_iter()
            .filter(|p| !p.ghost)
            .map(|p| (p.id, p.rect))
            .collect()
    }

    /// Gives the conversation a tab of its project, so closing its pane leaves it open.
    fn ensure_tab(&mut self, ix: usize) {
        let Some(project) = self.project_of(ix) else {
            return;
        };
        let has = |tabs: &Vec<Tab>| tabs.iter().any(|t| t.conversation == ix && !t.closing);
        let present = if Some(project) == self.current_project {
            has(&self.tabs)
        } else {
            self.parked.get(&project).is_some_and(|(tabs, _)| has(tabs))
        };
        if present {
            return;
        }
        let now = Instant::now();
        let id = self.next_id;
        self.next_id += 1;
        let tab = Tab {
            id,
            conversation: ix,
            pinned: false,
            closing: false,
            x: Tween::still(-1., now),
            w: Tween::still(-1., now),
        };
        if Some(project) == self.current_project {
            self.tabs.push(tab);
        } else {
            self.parked
                .entry(project)
                .or_insert((Vec::new(), id))
                .0
                .push(tab);
        }
    }

    /// Starts a change of layout: remembers where the panes were so they glide to where they go.
    fn split_commit(
        &mut self,
        base: &Layout,
        new: Layout,
        enter: Option<(u32, Edge)>,
        leave: Vec<Ghost>,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        let from_framed = self.split_framed(now);
        let from = if self.split.layout.is_some() {
            self.split_current(now)
        } else {
            base.rects().0
        };
        let single = new.count() < 2;
        self.split.layout = Some(new);
        self.split.hover = None;
        self.split.anim = if cx.reduce_motion() {
            None
        } else {
            Some(Anim {
                start: now,
                from,
                enter: enter.into_iter().collect(),
                leave,
                framed: (from_framed, if single { 0. } else { 1. }),
            })
        };
        if single && self.split.anim.is_none() {
            self.split.layout = None;
        }
        self.split_save();
        cx.notify();
    }

    /// Opens a conversation in a pane at the edge of a target pane (or of the whole area).
    pub(super) fn split_drop_at(
        &mut self,
        session: &str,
        target: Target,
        edge: Edge,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.session_index(session) else {
            return;
        };
        let Some(base) = self.split_base() else {
            self.open(ix, cx);
            return;
        };
        let Some(new) = base.dropped(session, target, edge) else {
            return;
        };
        let enter =
            (edge != Edge::Center && base.showing(session).is_none()).then(|| (new.active, edge));
        self.ensure_tab(ix);
        self.split_commit(&base, new, enter, Vec::new(), cx);
        self.open(ix, cx);
    }

    /// "Abrir ao lado", hand off and second opinion: beside the source when a split exists or is
    /// possible, otherwise as a plain tab.
    pub(super) fn open_beside(&mut self, ix: usize, edge: Edge, cx: &mut Context<Self>) {
        let session = self.conversations[ix].session_id.clone();
        let planned = self
            .split_base()
            .and_then(|base| base.opened_beside(&session, edge).map(|new| (base, new)));
        let Some((base, new)) = planned else {
            self.open(ix, cx);
            return;
        };
        let enter = base.showing(&session).is_none().then(|| (new.active, edge));
        self.ensure_tab(ix);
        self.split_commit(&base, new, enter, Vec::new(), cx);
        self.open(ix, cx);
    }

    /// Closes a pane; its conversation stays open as a tab.
    pub(super) fn split_close(&mut self, leaf: u32, cx: &mut Context<Self>) {
        let Some(layout) = self.split.layout.clone() else {
            return;
        };
        let Some(new) = layout.removed(leaf) else {
            return;
        };
        let session = layout.session_of(leaf).unwrap_or("").to_owned();
        if let Some(ix) = self.session_index(&session) {
            self.ensure_tab(ix);
        }
        let now = Instant::now();
        let rect = self
            .split_current(now)
            .into_iter()
            .find(|(id, _)| *id == leaf)
            .map_or(WHOLE, |(_, r)| r);
        let ghost = Ghost {
            session,
            rect,
            hug: layout.hug_edge(leaf),
        };
        let active = new.active;
        let moved = active != layout.active;
        let target = new.session_of(active).and_then(|s| self.session_index(s));
        self.split_commit(&layout, new, None, vec![ghost], cx);
        if moved {
            if let Some(ix) = target {
                self.open(ix, cx);
            }
        }
    }

    /// A press inside a pane makes it the active one before the press is handled.
    pub(super) fn split_activate(&mut self, leaf: u32, cx: &mut Context<Self>) {
        let Some(layout) = self.split.layout.as_mut() else {
            return;
        };
        if layout.active == leaf {
            return;
        }
        let Some(session) = layout.session_of(leaf).map(str::to_owned) else {
            return;
        };
        layout.active = leaf;
        self.split_save();
        if let Some(ix) = self.session_index(&session) {
            self.open(ix, cx);
        }
        cx.notify();
    }

    /// The selected tab changed (Shell::show): the active pane follows it.
    pub(super) fn split_sync(&mut self) {
        let session = self.active_session_id();
        if session.is_empty() {
            return;
        }
        if let Some(layout) = self.split.layout.as_mut() {
            if layout.select(&session) {
                self.split_save();
            }
        }
    }

    /// After the saved state loads: brings the saved panes back, dropping those whose session is gone.
    pub(super) fn split_restore(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.split.restored, true) {
            return;
        }
        let Some(saved) = read(&file()) else { return };
        let layout = saved.pruned(&|s| self.session_alive(s));
        if layout.count() < 2 {
            self.split_save();
            return;
        }
        let sessions: Vec<String> = layout
            .leaves()
            .into_iter()
            .map(|(_, s)| s.to_owned())
            .collect();
        let active = layout.active_session().to_owned();
        for session in &sessions {
            if let Some(ix) = self.session_index(session) {
                self.ensure_tab(ix);
            }
        }
        self.split.layout = Some(layout);
        if let Some(ix) = self.session_index(&active) {
            self.open(ix, cx);
        }
        self.split_save();
    }

    fn split_housekeeping(&mut self, now: Instant) {
        if let Some(layout) = &self.split.layout {
            if layout.leaves().iter().any(|(_, s)| !self.session_alive(s)) {
                let pruned = layout.pruned(&|s| self.session_alive(s));
                self.split.layout = Some(pruned);
                self.split.anim = None;
            }
        }
        if self
            .split
            .anim
            .as_ref()
            .is_some_and(|a| a.progress(now) >= 1.)
        {
            self.split.anim = None;
        }
        if self.split.anim.is_none() && self.split.layout.as_ref().is_some_and(|l| l.count() < 2) {
            self.split.layout = None;
            self.split_save();
        }
    }

    /// What `session` dragged to `position` would land on, within the area's `bounds`.
    fn split_aim(
        &mut self,
        session: &str,
        position: Point<Pixels>,
        bounds: Bounds<Pixels>,
    ) -> bool {
        let before = self.split.hover;
        self.split.hover = None;
        if bounds.contains(&position) {
            if let Some(base) = self.split_base() {
                let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let nx = f32::from(position.x - bounds.left()) / width;
                let ny = f32::from(position.y - bounds.top()) / height;
                let panes = match &self.split.layout {
                    Some(_) => self.split_current(Instant::now()),
                    None => base.rects().0,
                };
                if let Some((target, edge)) = target_at(&panes, nx, ny, width, height) {
                    let state = base.drop_state(session, target, edge);
                    if state != DropState::Nothing {
                        self.split.hover = Some((target, edge, state));
                    }
                }
            }
        }
        before != self.split.hover
    }

    /// Mouse moves that belong to the panes: a divider being dragged, and a tab pulled down
    /// over the area. True when the move is consumed.
    pub(super) fn split_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) -> bool {
        let left = event.pressed_button == Some(MouseButton::Left);
        if let Some(branch) = self.split.resizing {
            let (Some(area), Some(layout)) = (self.split.area.get(), self.split.layout.as_ref())
            else {
                self.split.resizing = None;
                return false;
            };
            if !left {
                self.split.resizing = None;
                return true;
            }
            if let Some(divider) = layout.rects().1.into_iter().find(|d| d.id == branch) {
                let (origin, span, at) = if divider.row {
                    (
                        f32::from(area.left()) + divider.rect.x * f32::from(area.size.width),
                        divider.rect.w * f32::from(area.size.width),
                        f32::from(event.position.x),
                    )
                } else {
                    (
                        f32::from(area.top()) + divider.rect.y * f32::from(area.size.height),
                        divider.rect.h * f32::from(area.size.height),
                        f32::from(event.position.y),
                    )
                };
                if let Some(next) = layout.resized(branch, (at - origin) / span.max(1.)) {
                    self.split.layout = Some(next);
                    cx.notify();
                }
            }
            return true;
        }
        // A tab dragged below the strip aims at the panes.
        let pulled = self.drag.as_ref().filter(|d| d.active).map(|d| d.id);
        if let (Some(tab), true, true) = (pulled, left, event.position.y > px(TITLE_BAR + 12.)) {
            let session = self
                .tabs
                .iter()
                .find(|t| t.id == tab)
                .map(|t| self.conversations[t.conversation].session_id.clone());
            if let (Some(session), Some(area)) = (session, self.split.area.get()) {
                if self.split_aim(&session, event.position, area) {
                    cx.notify();
                }
            }
        } else if pulled.is_some() && self.split.hover.take().is_some() {
            cx.notify();
        }
        false
    }

    /// The button went up: a tab pulled over the panes lands there; nothing stays aimed or resized.
    pub(super) fn split_release(&mut self, cx: &mut Context<Self>) {
        self.split.resizing = None;
        let pulled = self.drag.as_ref().filter(|d| d.active).map(|d| d.id);
        let aimed = self.split.hover.take();
        if let (Some(tab), Some((target, edge, DropState::Ok))) = (pulled, aimed) {
            let session = self
                .tabs
                .iter()
                .find(|t| t.id == tab)
                .map(|t| self.conversations[t.conversation].session_id.clone());
            if let Some(session) = session {
                self.drag = None;
                self.split_drop_at(&session, target, edge, cx);
            }
        }
        cx.notify();
    }

    /// The conversation area: the one transcript, or the panes, with the drop zone over them.
    pub(super) fn split_view(
        &mut self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.split_housekeeping(now);
        let dragging = cx.has_active_drag() || self.drag.as_ref().is_some_and(|d| d.active);
        let hover = self.split.hover.filter(|_| dragging);
        let cell = self.split.area.clone();
        let probe = gpui::canvas(|_, _, _| {}, move |bounds, _, _, _| cell.set(Some(bounds)))
            .absolute()
            .size_full();
        let mut area = div()
            .id("split-area")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .child(probe)
            .on_drag_move(
                cx.listener(|this, event: &gpui::DragMoveEvent<ConvDrag>, _, cx| {
                    let session = event.drag(cx).session.clone();
                    if this.split_aim(&session, event.event.position, event.bounds) {
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(|this, drag: &ConvDrag, _, cx| {
                if let Some((target, edge, DropState::Ok)) = this.split.hover.take() {
                    this.split_drop_at(&drag.session.clone(), target, edge, cx);
                }
                cx.notify();
            }));
        let Some(layout) = self.split.layout.clone() else {
            let body = self.transcript(now, moving, cx);
            let overlay = self.drop_overlay(hover, &[(1, WHOLE)], true);
            return area.child(body).children(overlay).into_any_element();
        };
        let anim_e = self
            .split
            .anim
            .as_ref()
            .map(|a| eased(a.progress(now), Curve::Expo));
        if anim_e.is_some() {
            *moving = true;
        }
        let framed = self.split_framed(now);
        let boxes = geometry(&layout, self.split.anim.as_ref(), anim_e.unwrap_or(1.));
        let rects: Vec<(u32, Rect)> = boxes
            .iter()
            .filter(|b| !b.ghost)
            .map(|b| (b.id, b.rect))
            .collect();
        let mut ordered: Vec<PaneBox> = boxes;
        // The active pane is built and drawn last: its composer owns the caret and the menus' anchor.
        ordered.sort_by_key(|b| (!b.ghost && b.id == layout.active(), b.ghost));
        for pane in ordered {
            if pane.rect.w < 0.002 || pane.rect.h < 0.002 {
                continue;
            }
            let Some(ix) = self.session_index(&pane.session) else {
                continue;
            };
            self.ensure_loaded(ix);
            let active = !pane.ghost && pane.id == layout.active();
            let body = self.pane_body(ix, active, now, moving, cx);
            area = area.child(self.pane_element(&pane, ix, active, framed, body, cx));
        }
        if self.split.anim.is_none() {
            for divider in layout.rects().1 {
                area = area.child(self.divider_handle(divider, cx));
            }
        }
        let overlay = self.drop_overlay(hover, &rects, false);
        area.children(overlay).into_any_element()
    }

    /// One pane's transcript and composer, built for its own conversation.
    fn pane_body(
        &mut self,
        ix: usize,
        active: bool,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.split.building.set(Some(ix));
        let mut jump = self
            .split
            .jumps
            .remove(&ix)
            .unwrap_or_else(|| Tween::still(0., now));
        std::mem::swap(&mut self.jump, &mut jump);
        // The find bar belongs to the active pane.
        let find = (!active).then(|| std::mem::take(&mut self.find));
        let body = self.transcript(now, moving, cx);
        if let Some(find) = find {
            self.find = find;
        }
        std::mem::swap(&mut self.jump, &mut jump);
        self.split.jumps.insert(ix, jump);
        self.split.building.set(None);
        body
    }

    fn pane_element(
        &self,
        pane: &PaneBox,
        ix: usize,
        active: bool,
        framed: f32,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let conversation = &self.conversations[ix];
        let id = pane.id;
        let r = pane.rect;
        let frame_color = if active {
            accent_wash(0.5 * framed)
        } else {
            content(0.08 * framed)
        };
        let title = conversation.title.clone();
        let session = conversation.session_id.clone();
        let header = div()
            .id(("pane-header", id))
            .flex_shrink_0()
            .h(px(HEADER * framed))
            .overflow_hidden()
            .opacity(framed)
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(content(0.08))
            .cursor_pointer()
            .on_drag(
                ConvDrag {
                    session,
                    title: title.clone(),
                },
                |drag, _, _, cx| cx.new(|_| DragChip(drag.title.clone())),
            )
            .child(provider_glyph(conversation.provider, 14.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.5))
                    .text_color(content(if active { 0.92 } else { 0.6 }))
                    .child(title),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(content(0.4))
                    .child(provider_title(conversation.provider)),
            )
            .child(
                div()
                    .id(("pane-close", id))
                    .flex_shrink_0()
                    .size(px(22.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.07)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.split_close(id, cx);
                    }))
                    .child(icon("close", 12.).text_color(content(0.6))),
            );
        let frame = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(crate::theme::RADIUS_LG))
            .border_1()
            .border_color(frame_color)
            .bg(pane_fill())
            .child(header)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .child(body),
            );
        let mut inner = div().absolute().p(px(GAP * framed)).child(frame);
        inner = match pane.slide {
            Some((align, full)) => {
                let (w, h) = (r.w.max(0.002), r.h.max(0.002));
                match align {
                    Edge::Left | Edge::Right => {
                        let inner = inner.top_0().h_full().w(gpui::relative(full.w / w));
                        if align == Edge::Left {
                            inner.left_0()
                        } else {
                            inner.right_0()
                        }
                    }
                    _ => {
                        let inner = inner.left_0().w_full().h(gpui::relative(full.h / h));
                        if align == Edge::Top {
                            inner.top_0()
                        } else {
                            inner.bottom_0()
                        }
                    }
                }
            }
            None => inner.size_full().top_0().left_0(),
        };
        let mut wrapper = div()
            .id(("pane", id))
            .absolute()
            .left(gpui::relative(r.x))
            .top(gpui::relative(r.y))
            .w(gpui::relative(r.w))
            .h(gpui::relative(r.h))
            .overflow_hidden()
            .child(inner);
        if !pane.ghost {
            wrapper = wrapper.capture_any_mouse_down(
                cx.listener(move |this, _, _, cx| this.split_activate(id, cx)),
            );
        }
        wrapper.into_any_element()
    }

    fn divider_handle(&self, divider: Divider, cx: &mut Context<Self>) -> AnyElement {
        let id = divider.id;
        let r = divider.rect;
        let handle = div()
            .id(("pane-divider", id))
            .absolute()
            .hover(|s| s.bg(accent_wash(0.25)));
        let handle = if divider.row {
            handle
                .left(gpui::relative(r.x + r.w * divider.ratio))
                .top(gpui::relative(r.y))
                .h(gpui::relative(r.h))
                .w(px(GAP * 2.))
                .ml(px(-GAP))
                .cursor_col_resize()
        } else {
            handle
                .top(gpui::relative(r.y + r.h * divider.ratio))
                .left(gpui::relative(r.x))
                .w(gpui::relative(r.w))
                .h(px(GAP * 2.))
                .mt(px(-GAP))
                .cursor_row_resize()
        };
        handle
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.split.resizing = Some(id);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The zone where the dragged conversation would land, lit with the accent wash and hairline.
    fn drop_overlay(
        &self,
        hover: Option<(Target, Edge, DropState)>,
        panes: &[(u32, Rect)],
        single: bool,
    ) -> Option<AnyElement> {
        let (target, edge, state) = hover?;
        let base = match target {
            Target::Root => WHOLE,
            Target::Pane(id) => panes
                .iter()
                .find(|(p, _)| *p == id)
                .map(|(_, r)| *r)
                .or(single.then_some(WHOLE))?,
        };
        let r = base.half(edge);
        let full = state == DropState::Full;
        let label = if full {
            crate::i18n::t("Limite de painéis atingido")
        } else {
            match edge {
                Edge::Left => crate::i18n::t("À esquerda"),
                Edge::Right => crate::i18n::t("À direita"),
                Edge::Top => crate::i18n::t("Acima"),
                Edge::Bottom => crate::i18n::t("Abaixo"),
                Edge::Center => crate::i18n::t("Substituir"),
            }
        };
        Some(
            div()
                .absolute()
                .left(gpui::relative(r.x))
                .top(gpui::relative(r.y))
                .w(gpui::relative(r.w))
                .h(gpui::relative(r.h))
                .p(px(GAP))
                .child(
                    div()
                        .size_full()
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(if full { content(0.3) } else { accent_wash(0.7) })
                        .bg(if full {
                            content(0.06)
                        } else {
                            accent_wash(0.16)
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .px(px(10.))
                                .h(px(24.))
                                .rounded(px(crate::theme::RADIUS_SM))
                                .flex()
                                .items_center()
                                .bg(floating_fill())
                                .text_size(px(12.))
                                .text_color(content(0.85))
                                .child(label),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Layout {
        Layout::single("a")
            .dropped("b", Target::Pane(1), Edge::Right)
            .unwrap()
    }

    fn sessions(layout: &Layout) -> Vec<&str> {
        layout.leaves().into_iter().map(|(_, s)| s).collect()
    }

    #[test]
    fn dropping_on_each_edge_puts_the_new_pane_on_that_side() {
        for (edge, row, new_first) in [
            (Edge::Left, true, true),
            (Edge::Right, true, false),
            (Edge::Top, false, true),
            (Edge::Bottom, false, false),
        ] {
            let layout = Layout::single("a")
                .dropped("b", Target::Pane(1), edge)
                .unwrap();
            assert_eq!(
                sessions(&layout),
                if new_first { ["b", "a"] } else { ["a", "b"] }
            );
            let (_, dividers) = layout.rects();
            assert_eq!((dividers[0].row, dividers[0].ratio), (row, 0.5));
            assert_eq!(layout.active_session(), "b", "the dropped pane is active");
        }
    }

    #[test]
    fn root_target_splits_the_whole_area() {
        let layout = two().dropped("c", Target::Root, Edge::Bottom).unwrap();
        let (panes, dividers) = layout.rects();
        assert_eq!(sessions(&layout), ["a", "b", "c"]);
        let c = panes
            .iter()
            .find(|(id, _)| layout.session_of(*id) == Some("c"))
            .unwrap()
            .1;
        assert_eq!(
            c,
            Rect {
                x: 0.,
                y: 0.5,
                w: 1.,
                h: 0.5
            }
        );
        assert_eq!(dividers.len(), 2);
    }

    #[test]
    fn the_pane_limit_refuses_a_fifth_conversation() {
        let mut layout = Layout::single("a");
        for session in ["b", "c", "d"] {
            layout = layout
                .dropped(session, Target::Pane(layout.active()), Edge::Right)
                .unwrap();
        }
        assert_eq!(layout.count(), MAX_PANES);
        assert_eq!(
            layout.drop_state("e", Target::Root, Edge::Left),
            DropState::Full
        );
        assert!(layout.dropped("e", Target::Root, Edge::Left).is_none());
        // Replacing a pane and moving an open one still work at the limit.
        assert_eq!(
            layout.drop_state("e", Target::Pane(1), Edge::Center),
            DropState::Ok
        );
        assert_eq!(
            layout.drop_state("a", Target::Pane(layout.active()), Edge::Left),
            DropState::Ok
        );
    }

    #[test]
    fn dropping_a_conversation_on_its_own_pane_changes_nothing() {
        let layout = two();
        assert_eq!(
            layout.drop_state("a", Target::Pane(1), Edge::Left),
            DropState::Nothing
        );
        assert_eq!(
            Layout::single("a").drop_state("a", Target::Pane(1), Edge::Right),
            DropState::Nothing
        );
        assert_eq!(
            layout.drop_state("c", Target::Pane(99), Edge::Left),
            DropState::Nothing
        );
        assert_eq!(
            layout.drop_state("c", Target::Root, Edge::Center),
            DropState::Nothing
        );
    }

    #[test]
    fn center_replaces_and_an_open_conversation_swaps_places() {
        let layout = two();
        let replaced = layout.dropped("c", Target::Pane(1), Edge::Center).unwrap();
        assert_eq!(sessions(&replaced), ["c", "b"]);
        assert_eq!(replaced.active_session(), "c");
        let swapped = layout.dropped("b", Target::Pane(1), Edge::Center).unwrap();
        assert_eq!(sessions(&swapped), ["b", "a"]);
        assert_eq!(swapped.active_session(), "b");
    }

    #[test]
    fn dragging_an_open_pane_to_another_edge_moves_it() {
        let layout = two().dropped("c", Target::Pane(1), Edge::Bottom).unwrap();
        let moved = layout.dropped("c", Target::Root, Edge::Left).unwrap();
        assert_eq!(sessions(&moved), ["c", "a", "b"]);
        assert_eq!(moved.count(), 3);
        assert_eq!(moved.active_session(), "c");
    }

    #[test]
    fn removing_a_pane_collapses_its_branch_and_hands_over_the_active_flag() {
        let layout = two();
        let b = layout.showing("b").unwrap();
        assert_eq!(layout.active(), b);
        let left = layout.removed(b).unwrap();
        assert_eq!(sessions(&left), ["a"]);
        assert_eq!(left.active_session(), "a");
        assert_eq!(left.rects().1.len(), 0, "no branch is left");
        assert!(left.removed(1).is_none(), "the last pane stays");
        // Removing an inactive pane keeps the active one.
        let kept = layout.removed(1).unwrap();
        assert_eq!(kept.active_session(), "b");
    }

    #[test]
    fn removing_inside_a_deeper_tree_gives_the_space_to_the_sibling() {
        let layout = two().dropped("c", Target::Pane(1), Edge::Bottom).unwrap();
        let c = layout.showing("c").unwrap();
        let (before, _) = layout.rects();
        assert_eq!(before.iter().find(|(id, _)| *id == c).unwrap().1.h, 0.5);
        let after = layout.removed(c).unwrap();
        let a = after.showing("a").unwrap();
        assert_eq!(
            after.rects().0.iter().find(|(id, _)| *id == a).unwrap().1,
            Rect {
                x: 0.,
                y: 0.,
                w: 0.5,
                h: 1.
            }
        );
        assert_eq!(after.active_session(), "a", "the sibling takes over");
    }

    #[test]
    fn a_pane_closing_hugs_the_edge_it_sits_on() {
        let layout = two();
        assert_eq!(layout.hug_edge(1), Edge::Left);
        assert_eq!(layout.hug_edge(layout.showing("b").unwrap()), Edge::Right);
        let column = Layout::single("a")
            .dropped("b", Target::Pane(1), Edge::Top)
            .unwrap();
        assert_eq!(column.hug_edge(column.showing("b").unwrap()), Edge::Top);
        assert_eq!(column.hug_edge(1), Edge::Bottom);
    }

    #[test]
    fn a_divider_keeps_a_fifth_of_each_side() {
        let layout = two();
        let branch = layout.rects().1[0].id;
        assert_eq!(layout.resized(branch, 0.7).unwrap().rects().1[0].ratio, 0.7);
        assert_eq!(
            layout.resized(branch, 0.01).unwrap().rects().1[0].ratio,
            MIN_RATIO
        );
        assert_eq!(
            layout.resized(branch, 5.).unwrap().rects().1[0].ratio,
            1. - MIN_RATIO
        );
        assert!(layout.resized(branch, 0.5).is_none(), "no change");
        assert!(layout.resized(999, 0.3).is_none());
    }

    #[test]
    fn ratios_split_the_rects() {
        let layout = two().resized(two().rects().1[0].id, 0.25).unwrap();
        let (panes, dividers) = layout.rects();
        assert_eq!(
            panes[0].1,
            Rect {
                x: 0.,
                y: 0.,
                w: 0.25,
                h: 1.
            }
        );
        assert_eq!(
            panes[1].1,
            Rect {
                x: 0.25,
                y: 0.,
                w: 0.75,
                h: 1.
            }
        );
        assert_eq!(dividers[0].rect, WHOLE);
    }

    #[test]
    fn the_selection_activates_its_pane_or_replaces_the_active_one() {
        let mut layout = two();
        assert!(layout.select("a"));
        assert_eq!(layout.active_session(), "a");
        assert!(!layout.select("a"));
        assert!(layout.select("z"));
        assert_eq!(sessions(&layout), ["z", "b"]);
    }

    #[test]
    fn open_beside_goes_next_to_the_active_pane_or_the_other_one() {
        let layout = Layout::single("a");
        let beside = layout.opened_beside("b", Edge::Right).unwrap();
        assert_eq!(sessions(&beside), ["a", "b"]);
        // A session already in the active pane moves next to the other pane.
        let three = beside.dropped("c", Target::Pane(1), Edge::Bottom).unwrap();
        let moved = three.opened_beside("c", Edge::Right).unwrap();
        assert_eq!(moved.active_session(), "c");
        assert_eq!(moved.count(), 3);
        assert!(
            Layout::single("a")
                .opened_beside("a", Edge::Right)
                .is_none()
        );
    }

    #[test]
    fn panes_of_gone_sessions_close_but_the_last_one_stays() {
        let layout = two().dropped("c", Target::Root, Edge::Bottom).unwrap();
        let pruned = layout.pruned(&|s| s == "a");
        assert_eq!(sessions(&pruned), ["a"]);
        let none = layout.pruned(&|_| false);
        assert_eq!(none.count(), 1);
    }

    #[test]
    fn edges_follow_the_pointer() {
        assert_eq!(edge_at(0.5, 0.5), Edge::Center);
        assert_eq!(edge_at(0.05, 0.5), Edge::Left);
        assert_eq!(edge_at(0.95, 0.4), Edge::Right);
        assert_eq!(edge_at(0.5, 0.05), Edge::Top);
        assert_eq!(edge_at(0.6, 0.95), Edge::Bottom);
    }

    #[test]
    fn the_area_border_splits_the_whole_area_once_there_are_several_panes() {
        let layout = two();
        let (panes, _) = layout.rects();
        assert_eq!(
            target_at(&panes, 0.01, 0.5, 1000., 600.),
            Some((Target::Root, Edge::Left))
        );
        assert_eq!(
            target_at(&panes, 0.5, 0.99, 1000., 600.),
            Some((Target::Root, Edge::Bottom))
        );
        let (target, edge) = target_at(&panes, 0.1, 0.5, 1000., 600.).unwrap();
        assert_eq!((target, edge), (Target::Pane(1), Edge::Left));
        let single = Layout::single("a").rects().0;
        assert_eq!(
            target_at(&single, 0.01, 0.5, 1000., 600.),
            Some((Target::Pane(1), Edge::Left))
        );
        assert_eq!(
            target_at(&single, 0.5, 0.5, 1000., 600.),
            Some((Target::Pane(1), Edge::Center))
        );
    }

    #[test]
    fn rects_slide_from_the_edge_and_back() {
        let to = Rect {
            x: 0.5,
            y: 0.,
            w: 0.5,
            h: 1.,
        };
        assert_eq!(
            to.collapsed(Edge::Right),
            Rect {
                x: 1.,
                y: 0.,
                w: 0.,
                h: 1.
            }
        );
        assert_eq!(
            to.collapsed(Edge::Left),
            Rect {
                x: 0.5,
                y: 0.,
                w: 0.,
                h: 1.
            }
        );
        assert_eq!(to.collapsed(Edge::Right).lerp(to, 1.), to);
        assert_eq!(
            to.collapsed(Edge::Right).lerp(to, 0.5),
            Rect {
                x: 0.75,
                y: 0.,
                w: 0.25,
                h: 1.
            }
        );
        assert_eq!(
            to.half(Edge::Left),
            Rect {
                x: 0.5,
                y: 0.,
                w: 0.25,
                h: 1.
            }
        );
    }

    #[test]
    fn the_layout_survives_a_json_round_trip() {
        let layout = two().dropped("c", Target::Pane(1), Edge::Bottom).unwrap();
        let layout = layout.resized(layout.rects().1[0].id, 0.35).unwrap();
        let text = layout.to_json().to_string();
        let back = Layout::from_json(&serde_json::from_str(&text).unwrap()).unwrap();
        assert_eq!(back, layout);
        // New ids keep counting from where they were.
        let more = back.dropped("d", Target::Root, Edge::Left).unwrap();
        assert_eq!(more.count(), 4);
    }

    #[test]
    fn a_saved_layout_that_is_broken_is_refused() {
        let good = two().to_json();
        let mut over = good.clone();
        over["version"] = json!(2);
        assert!(Layout::from_json(&over).is_none());
        let mut gone = good.clone();
        gone["active"] = json!(77);
        assert!(Layout::from_json(&gone).is_none());
        let mut same = good.clone();
        same["root"]["b"]["session"] = json!("a");
        assert!(Layout::from_json(&same).is_none(), "one session, one pane");
        let mut reused = good.clone();
        reused["root"]["b"]["leaf"] = json!(1);
        assert!(Layout::from_json(&reused).is_none());
        let mut ratio = good;
        ratio["root"]["ratio"] = json!(0.0);
        assert_eq!(
            Layout::from_json(&ratio).unwrap().rects().1[0].ratio,
            MIN_RATIO
        );
        assert!(Layout::from_json(&json!({})).is_none());
    }

    #[test]
    fn the_file_round_trips_and_a_single_pane_removes_it() {
        let dir = std::env::temp_dir().join(format!("sirus-split-{}", std::process::id()));
        let path = dir.join("split-layout.json");
        let layout = two();
        write(&path, Some(&layout));
        assert_eq!(read(&path), Some(layout));
        write(&path, None);
        assert_eq!(read(&path), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
