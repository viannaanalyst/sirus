//! `sirus-host`: the back end of Sirus without a window (ADR-131): the Mac window and the phone
//! talk to it. Build with `cargo build --release --no-default-features --features host --bin sirus-host`.

fn main() {
    #[cfg(target_os = "macos")]
    connect_window_server();
    std::process::exit(sirus_code_lib::run_host());
}

/// Window screenshots abort the process when it has no connection to the window server, which a
/// process with no window never opens by itself. Opens one on the main thread and stays out of
/// the Dock.
#[cfg(target_os = "macos")]
fn connect_window_server() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    #[link(name = "AppKit", kind = "framework")]
    extern "C" {
        fn NSApplicationLoad() -> bool;
    }
    // NSApplicationActivationPolicyProhibited.
    const PROHIBITED: isize = 2;
    unsafe {
        NSApplicationLoad();
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: bool = msg_send![app, setActivationPolicy: PROHIBITED];
    }
}
