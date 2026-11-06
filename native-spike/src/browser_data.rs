//! Data the browser keeps for the whole app, and the Web Inspector (`browser.rs` `clear_data` and
//! `devtools` in Sirus).
use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::msg_send;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSDate, NSObjectProtocol, NSSet};
use objc2_web_kit::{
    WKWebView, WKWebsiteDataStore, WKWebsiteDataTypeCookies, WKWebsiteDataTypeDiskCache,
    WKWebsiteDataTypeFetchCache, WKWebsiteDataTypeMemoryCache,
};

/// Clears cookies, or the caches, of the shared data store (every conversation's browser).
pub fn clear(cookies: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let types = unsafe {
        if cookies {
            NSSet::from_slice(&[WKWebsiteDataTypeCookies])
        } else {
            NSSet::from_slice(&[
                WKWebsiteDataTypeDiskCache,
                WKWebsiteDataTypeMemoryCache,
                WKWebsiteDataTypeFetchCache,
            ])
        }
    };
    let done = RcBlock::new(|| {});
    unsafe {
        WKWebsiteDataStore::defaultDataStore(mtm)
            .removeDataOfTypes_modifiedSince_completionHandler(
                &types,
                &NSDate::distantPast(),
                &done,
            );
    }
}

/// Opens Safari's Web Inspector for the tab (WebKit's `_inspector`).
pub fn inspector(web: &WKWebView) -> bool {
    if !web.respondsToSelector(objc2::sel!(_inspector)) {
        return false;
    }
    unsafe {
        let inspector: *mut AnyObject = msg_send![web, _inspector];
        if inspector.is_null() {
            return false;
        }
        let _: () = msg_send![inspector, show];
    }
    true
}
