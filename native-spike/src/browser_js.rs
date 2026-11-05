//! The page scripts of the browser pane and the agent's browser tools. The `.js` files in
//! `browser_js/` are copies of the constants in `src-tauri/src/browser.rs` and
//! `browser_agent.rs` (Sirus Code runs the same text), so the refs and result shapes the tool
//! docs promise stay true. The login scripts are read straight from `src-tauri/src`.

macro_rules! script {
    ($name:ident, $file:literal) => {
        pub const $name: &str = include_str!(concat!("browser_js/", $file));
    };
}

// Document start, every tab.
script!(PAGE_BRIDGE, "page_bridge.js");
script!(NETWORK, "network.js");

// Bodies run with `callAsyncJavaScript`; `args` is the JSON text of their argument.
script!(SNAPSHOT, "snapshot.js");
script!(CLICK, "click.js");
script!(TYPE, "type.js");
script!(SCROLL, "scroll.js");
script!(PRESS, "press.js");
script!(WAIT, "wait.js");
script!(LOGS, "logs.js");
script!(HOVER, "hover.js");
script!(SELECT, "select.js");
script!(DRAG, "drag.js");
script!(UPLOAD, "upload.js");
script!(NETWORK_READ, "network_read.js");

// Plain expressions evaluated in the page (the element picker).
script!(ANNOTATE_START, "annotate_start.js");
script!(ANNOTATE_FINISH, "annotate_finish.js");
script!(ANNOTATE_MEASURE, "annotate_measure.js");
script!(ANNOTATE_CANCEL, "annotate_cancel.js");

/// Reports submitted password forms (client content world); see `browser_login.rs`.
pub const LOGIN_CAPTURE: &str = concat!(
    "(",
    include_str!("../../src-tauri/src/browser_login_capture.js"),
    ")(window, document, function (message) { window.webkit.messageHandlers.sirusLogin.postMessage(message); });"
);
/// The body that fills a saved login; `args` carries the values and is never spliced in.
pub const LOGIN_FILL: &str = include_str!("../../src-tauri/src/browser_login_fill.js");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_present_and_secret_free() {
        for body in [
            SNAPSHOT,
            CLICK,
            TYPE,
            HOVER,
            SELECT,
            UPLOAD,
            LOGS,
            NETWORK_READ,
        ] {
            assert!(!body.trim().is_empty());
        }
        assert!(SNAPSHOT.contains("data-sirus-ref"));
        assert!(LOGIN_FILL.contains("JSON.parse(args)"));
        assert!(LOGIN_CAPTURE.contains("sirusLogin"));
    }
}
