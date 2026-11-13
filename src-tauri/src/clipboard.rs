//! Text clipboard bridging for a live session.
//!
//! Only plain text is exchanged, capped at [`MAX_TEXT_BYTES`]. The session
//! threads poll the local pasteboard's change count and forward changes to the
//! peer; applying a remote value updates the ignore state so the same text is
//! not echoed straight back. macOS may show its pasteboard privacy prompt the
//! first time a session reads the clipboard; the host's Advanced setting is the
//! opt-out.

use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

/// Largest single clipboard value forwarded over the session channel.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// The pasteboard's change counter. Compare successive values to notice an
/// external copy without reading the contents every poll.
pub fn change_count() -> i64 {
    autoreleasepool(|_| NSPasteboard::generalPasteboard().changeCount() as i64)
}

/// The current plain-text value, if the pasteboard holds text this app can read.
pub fn read_text() -> Option<String> {
    autoreleasepool(|_| {
        let value =
            NSPasteboard::generalPasteboard().stringForType(unsafe { NSPasteboardTypeString })?;
        Some(value.to_string())
    })
}

/// Replaces the pasteboard's plain-text value. Returns whether AppKit accepted
/// the write.
pub fn write_text(text: &str) -> bool {
    if text.len() > MAX_TEXT_BYTES {
        return false;
    }
    autoreleasepool(|_| {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
    })
}

/// Whether a value is worth forwarding: non-empty, structured-clone safe and
/// below the cap.
pub fn is_forwardable(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_TEXT_BYTES
}
