//! The runtime the back end runs in (ADR-131). The code was written against Tauri's `AppHandle`,
//! `Emitter`, `Manager`, `State` and `async_runtime`; the host (there is no Tauri any more) offers
//! the small stand-ins below with the same shape: managed state, events by name and a global tokio
//! runtime. Modules name `crate::runtime::…`, so one implementation serves the window, the phone
//! and the host.

pub use axum::http;
pub use headless::{async_runtime, AppHandle, Emitter, EventId, Listener, Manager, State};

mod headless {
    use std::any::{Any, TypeId};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, OnceLock};

    use parking_lot::{Mutex, RwLock};
    use serde::Serialize;

    pub type EventId = u32;

    /// One event as listeners see it: the payload is its JSON text, as in Tauri.
    pub struct Event {
        payload: String,
    }

    impl Event {
        pub fn payload(&self) -> &str {
            &self.payload
        }
    }

    type Handler = Arc<dyn Fn(Event) + Send + Sync>;

    struct Inner {
        data_dir: PathBuf,
        /// Managed state lives as long as the process, as Tauri's does.
        states: RwLock<HashMap<TypeId, &'static (dyn Any + Send + Sync)>>,
        listeners: Mutex<Vec<(EventId, String, Handler)>>,
        next_listener: AtomicU32,
    }

    /// The host's stand-in for `tauri::AppHandle`: managed state and an event bus.
    #[derive(Clone)]
    pub struct AppHandle(Arc<Inner>);

    impl AppHandle {
        pub fn new(data_dir: PathBuf) -> Self {
            Self(Arc::new(Inner {
                data_dir,
                states: RwLock::new(HashMap::new()),
                listeners: Mutex::new(Vec::new()),
                next_listener: AtomicU32::new(1),
            }))
        }

        /// The folder the host keeps its state in (`--data-dir`).
        pub fn data_dir(&self) -> &Path {
            &self.0.data_dir
        }

        /// There are no windows on the host, so callers take their windowless path.
        pub fn get_webview_window(&self, _label: &str) -> Option<NoWindow> {
            None
        }

        /// Tauri runs these one after another on the UI thread; the host keeps that
        /// order on one dedicated thread, so code written for it stays serialized.
        pub fn run_on_main_thread<F: FnOnce() + Send + 'static>(
            &self,
            task: F,
        ) -> Result<(), EmitError> {
            type Task = Box<dyn FnOnce() + Send>;
            static MAIN: OnceLock<Mutex<std::sync::mpsc::Sender<Task>>> = OnceLock::new();
            let sender = MAIN.get_or_init(|| {
                let (sender, receiver) = std::sync::mpsc::channel::<Task>();
                std::thread::Builder::new()
                    .name("sirus-main".into())
                    .spawn(move || {
                        while let Ok(task) = receiver.recv() {
                            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(task)).is_err()
                            {
                                tracing::error!("a main-thread task panicked");
                            }
                        }
                    })
                    .expect("the main-thread worker starts");
                Mutex::new(sender)
            });
            sender.lock().send(Box::new(task)).map_err(|_| EmitError)
        }
    }

    /// No window exists on the host: `get_webview_window` is always `None`, so the
    /// methods below can never run.
    pub enum NoWindow {}

    impl NoWindow {
        pub fn is_focused(&self) -> Result<bool, EmitError> {
            match *self {}
        }
    }

    /// `tauri::State`: a shared reference to managed state.
    pub struct State<'r, T: Send + Sync + 'static>(&'r T);

    impl<'r, T: Send + Sync + 'static> State<'r, T> {
        pub fn inner(&self) -> &'r T {
            self.0
        }
    }

    impl<T: Send + Sync + 'static> std::ops::Deref for State<'_, T> {
        type Target = T;
        fn deref(&self) -> &T {
            self.0
        }
    }

    impl<T: Send + Sync + 'static> Clone for State<'_, T> {
        fn clone(&self) -> Self {
            State(self.0)
        }
    }

    pub trait Manager {
        fn manage<T: Send + Sync + 'static>(&self, state: T) -> bool;
        fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'_, T>>;
        fn state<T: Send + Sync + 'static>(&self) -> State<'_, T> {
            self.try_state()
                .unwrap_or_else(|| panic!("state {} is not managed", std::any::type_name::<T>()))
        }
    }

    impl Manager for AppHandle {
        fn manage<T: Send + Sync + 'static>(&self, state: T) -> bool {
            let mut states = self.0.states.write();
            if states.contains_key(&TypeId::of::<T>()) {
                return false;
            }
            let leaked: &'static T = Box::leak(Box::new(state));
            states.insert(TypeId::of::<T>(), leaked);
            true
        }

        fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'_, T>> {
            let found: &'static (dyn Any + Send + Sync) =
                *self.0.states.read().get(&TypeId::of::<T>())?;
            found.downcast_ref::<T>().map(State)
        }
    }

    /// Never fails; the `Result` mirrors Tauri's signature so `?`/`let _ =` read the same.
    #[derive(Debug)]
    pub struct EmitError;

    impl std::fmt::Display for EmitError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("event payload could not be serialized")
        }
    }

    impl std::error::Error for EmitError {}

    pub trait Emitter {
        fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<(), EmitError>;
    }

    impl Emitter for AppHandle {
        fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<(), EmitError> {
            let handlers: Vec<Handler> = self
                .0
                .listeners
                .lock()
                .iter()
                .filter(|(_, name, _)| name == event)
                .map(|(_, _, handler)| handler.clone())
                .collect();
            if handlers.is_empty() {
                return Ok(());
            }
            let payload = serde_json::to_string(&payload).map_err(|_| EmitError)?;
            for handler in handlers {
                handler(Event {
                    payload: payload.clone(),
                });
            }
            Ok(())
        }
    }

    pub trait Listener {
        fn listen_any<F: Fn(Event) + Send + Sync + 'static>(
            &self,
            event: impl Into<String>,
            handler: F,
        ) -> EventId;
        fn unlisten(&self, id: EventId);
    }

    impl Listener for AppHandle {
        fn listen_any<F: Fn(Event) + Send + Sync + 'static>(
            &self,
            event: impl Into<String>,
            handler: F,
        ) -> EventId {
            let id = self.0.next_listener.fetch_add(1, Ordering::Relaxed);
            self.0
                .listeners
                .lock()
                .push((id, event.into(), Arc::new(handler)));
            id
        }

        fn unlisten(&self, id: EventId) {
            self.0
                .listeners
                .lock()
                .retain(|(listener, _, _)| *listener != id);
        }
    }

    /// `tauri::async_runtime`: one process-wide multi-threaded tokio runtime.
    pub mod async_runtime {
        use std::future::Future;
        use std::sync::OnceLock;

        pub use tokio::task::JoinHandle;

        fn runtime() -> &'static tokio::runtime::Runtime {
            static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
            RUNTIME.get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .thread_name("sirus-host")
                    .build()
                    .expect("the tokio runtime starts")
            })
        }

        pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
        where
            F: Future + Send + 'static,
            F::Output: Send + 'static,
        {
            runtime().spawn(future)
        }

        pub fn spawn_blocking<F, R>(function: F) -> JoinHandle<R>
        where
            F: FnOnce() -> R + Send + 'static,
            R: Send + 'static,
        {
            runtime().spawn_blocking(function)
        }

        pub fn block_on<F: Future>(future: F) -> F::Output {
            runtime().block_on(future)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn state_and_events_work_like_tauri() {
            let app = AppHandle::new(PathBuf::from("/tmp"));
            assert!(app.try_state::<u32>().is_none());
            assert!(app.manage(7u32));
            assert!(!app.manage(8u32), "state is managed once");
            assert_eq!(*app.state::<u32>(), 7);
            let seen = Arc::new(Mutex::new(Vec::new()));
            let sink = seen.clone();
            let id = app.listen_any("ping", move |event| {
                sink.lock().push(event.payload().to_string())
            });
            app.emit("ping", serde_json::json!({ "a": 1 })).unwrap();
            app.emit("other", 1).unwrap();
            app.unlisten(id);
            app.emit("ping", 2).unwrap();
            assert_eq!(*seen.lock(), vec![r#"{"a":1}"#.to_string()]);
        }
    }
}
