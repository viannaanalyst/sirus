//! Following a session's folder on disk (ADR-109). A panel that shows the workspace holds a
//! `Follow`: it asks the host's `workspace_watch` to watch the folder, listens for the batched
//! `workspace-changed` events and calls the panel back for each one that names its folder. Each
//! panel is one "view" for the host, which keeps one watcher per folder however many views there
//! are and drops it with the last. Dropping the `Follow` (the panel or its session going away)
//! stops that view, also when the host's answer had not arrived yet.
use crate::backend::Backend;
use gpui::Context;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
struct State {
    /// The path `workspace-changed` carries for this folder, once the host started watching.
    root: Option<String>,
    stopped: bool,
}

pub struct Follow {
    backend: Backend,
    session_id: String,
    state: Rc<RefCell<State>>,
}

impl Follow {
    /// Starts following `session_id`'s folder; `on_change` runs on the panel for each batch of
    /// changes. A host that cannot watch (the call fails) leaves the panel as it was.
    pub fn start<V: 'static>(
        backend: Backend,
        session_id: String,
        on_change: fn(&mut V, &mut Context<V>),
        cx: &mut Context<V>,
    ) -> Self {
        let state = Rc::new(RefCell::new(State::default()));
        // Listen first, so a change right after the watch starts is not missed.
        let events = backend.subscribe("workspace-changed");
        let reply = backend.request("workspace_watch", watch_args(&session_id, true));
        let task_state = state.clone();
        let task_backend = backend.clone();
        let task_session = session_id.clone();
        cx.spawn(async move |this, cx| {
            let root = match reply.recv().await {
                Ok(Ok(value)) => value.as_str().map(str::to_owned),
                _ => None,
            };
            let Some(root) = root else {
                return;
            };
            if task_state.borrow().stopped {
                // The panel went away while the host was answering.
                task_backend.invoke("workspace_watch", watch_args(&task_session, false));
                return;
            }
            task_state.borrow_mut().root = Some(root.clone());
            while let Ok(payload) = events.recv().await {
                if task_state.borrow().stopped {
                    break;
                }
                if !is_for(&payload, &root) {
                    continue;
                }
                if this.update(cx, |this, cx| on_change(this, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            backend,
            session_id,
            state,
        }
    }
}

impl Drop for Follow {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        state.stopped = true;
        if state.root.take().is_some() {
            self.backend
                .invoke("workspace_watch", watch_args(&self.session_id, false));
        }
    }
}

fn watch_args(session_id: &str, watch: bool) -> Value {
    json!({ "sessionId": session_id, "watch": watch })
}

/// Whether a `workspace-changed` payload names this folder.
fn is_for(payload: &Value, root: &str) -> bool {
    payload["path"].as_str() == Some(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_args_start_and_stop() {
        assert_eq!(
            watch_args("s1", true),
            json!({ "sessionId": "s1", "watch": true })
        );
        assert_eq!(watch_args("s1", false)["watch"], false);
    }

    #[test]
    fn only_the_followed_folders_events_count() {
        let event = json!({ "path": "/w/app" });
        assert!(is_for(&event, "/w/app"));
        assert!(!is_for(&event, "/w/other"));
        assert!(!is_for(&json!({}), "/w/app"));
    }
}
