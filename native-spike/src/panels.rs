//! The side panel's panes. Each is its own view with its own state; `open` builds one for a
//! conversation. Order matches `PANES` in shell.rs.
use crate::backend::Backend;
use gpui::{AnyView, App};

#[derive(Clone)]
pub struct PanelContext {
    pub backend: Backend,
    pub session_id: String,
    pub worktree: String,
    pub provider: &'static str,
    pub model: String,
    pub version: String,
}

pub fn open(kind: usize, context: PanelContext, cx: &mut App) -> Option<AnyView> {
    match kind {
        0 => Some(crate::panel_terminal::new(context, cx)),
        1 => Some(crate::panel_files::new(context, cx)),
        2 => Some(crate::panel_changes::new(context, cx)),
        3 => Some(crate::panel_browser::new(context, cx)),
        4 => Some(crate::panel_sidechat::new(context, cx)),
        5 => Some(crate::panel_simulator::new(context, cx)),
        _ => None,
    }
}
