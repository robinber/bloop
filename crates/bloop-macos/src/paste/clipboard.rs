//! Clipboard save, restore, and text write for the paste transaction.
//!
//! Before a paste, Bloop saves the data of all clipboard items. After the paste, it writes them back
//! only if nobody changed the clipboard.

#![expect(
    unsafe_code,
    reason = "the string type reads the AppKit constant NSPasteboardTypeString, an external static"
)]

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardType, NSPasteboardTypeString, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSInteger, NSString, ns_string};

/// The data of one clipboard item, for each of its types.
pub(super) type SavedItem = Vec<(Retained<NSPasteboardType>, Retained<NSData>)>;

const MAX_SAVED_ITEMS: usize = 64;
const MAX_SAVED_BYTES: usize = 64 * 1024 * 1024;

/// Reads all items of the clipboard, with the data of all their types.
///
/// Returns `None` if macOS cannot list the items. A type that cannot be read is left out.
/// A clipboard with more than [`MAX_SAVED_ITEMS`] items also gives `None`, before any data is read.
/// Data above [`MAX_SAVED_BYTES`] bytes gives `None` too, but the check runs after each type is
/// copied. Thus this limit bounds the memory that the snapshot keeps until the restore, not the
/// time or the memory of the read.
pub(super) fn save(pasteboard: &NSPasteboard) -> Option<Vec<SavedItem>> {
    save_within(pasteboard, MAX_SAVED_ITEMS, MAX_SAVED_BYTES)
}

/// Reads the clipboard as [`save`] does, with no more than `max_items` items and `max_bytes` bytes.
pub(super) fn save_within(
    pasteboard: &NSPasteboard,
    max_items: usize,
    max_bytes: usize,
) -> Option<Vec<SavedItem>> {
    let Some(items) = pasteboard.pasteboardItems() else {
        tracing::warn!("Bloop cannot read the clipboard, thus it will not restore it");
        return None;
    };
    let count = items.count();
    if count > max_items {
        tracing::warn!(
            items = count,
            "the clipboard has too many items, Bloop will not restore it"
        );
        return None;
    }
    let mut bytes = 0_usize;
    let mut missing = 0_usize;
    let mut saved = Vec::with_capacity(count);
    for item in &items {
        let mut types = Vec::new();
        for data_type in item.types() {
            let Some(data) = item.dataForType(&data_type) else {
                missing += 1;
                continue;
            };
            bytes = bytes.saturating_add(data.len());
            if bytes > max_bytes {
                tracing::warn!(
                    items = count,
                    bytes,
                    "the clipboard data is too large, Bloop will not restore it"
                );
                return None;
            }
            types.push((data_type, data));
        }
        saved.push(types);
    }
    if missing > 0 {
        tracing::warn!(
            missing,
            "Bloop cannot read some clipboard types, it will restore the others"
        );
    }
    Some(saved)
}

/// Returns `true` if nobody changed the clipboard since Bloop wrote it, at the change count `written`.
pub(super) fn owns_clipboard(pasteboard: &NSPasteboard, written: NSInteger) -> bool {
    let owned = pasteboard.changeCount() == written;
    if !owned {
        tracing::info!("the clipboard changed after the paste, Bloop keeps the new content");
    }
    owned
}

/// Writes the saved items back, if the clipboard did not change since `written`.
///
/// Without a saved clipboard, the text stays in the clipboard.
/// The restored items have the transient marker. Thus a clipboard manager does not record them again.
pub(super) fn restore(
    pasteboard: &NSPasteboard,
    saved: Option<Vec<SavedItem>>,
    written: NSInteger,
) {
    let Some(saved) = saved else {
        return;
    };
    if !owns_clipboard(pasteboard, written) {
        return;
    }
    pasteboard.clearContents();
    if saved.is_empty() {
        return;
    }
    let mut refused = 0_usize;
    let items: Vec<Retained<NSPasteboardItem>> = saved
        .iter()
        .map(|types| {
            let item = NSPasteboardItem::new();
            for (data_type, data) in types {
                if !item.setData_forType(data, data_type) {
                    refused += 1;
                }
            }
            if !item.setData_forType(&NSData::new(), transient_type()) {
                refused += 1;
            }
            item
        })
        .collect();
    if refused > 0 {
        tracing::warn!(refused, "the clipboard refused some restored types");
    }
    let objects: Vec<&ProtocolObject<dyn NSPasteboardWriting>> = items
        .iter()
        .map(|item| ProtocolObject::from_ref(&**item))
        .collect();
    if !pasteboard.writeObjects(&NSArray::from_slice(&objects)) {
        tracing::error!("the clipboard refused the restored content");
    }
}

/// Ends a paste, after the read or when Bloop abandons it: writes the saved items back as
/// [`restore`] does.
///
/// Without a saved clipboard, it replaces the item of Bloop with a normal text item, if the clipboard
/// did not change since `written`. Thus the text loses the transient and concealed markers, and the
/// item does not depend on a provider that no longer exists.
pub(super) fn finalize(
    pasteboard: &NSPasteboard,
    saved: Option<Vec<SavedItem>>,
    written: NSInteger,
    text: &NSString,
) {
    if saved.is_some() {
        restore(pasteboard, saved, written);
    } else if owns_clipboard(pasteboard, written) {
        write_text(pasteboard, text);
    }
}

/// Replaces the clipboard content with a normal text item.
pub(super) fn write_text(pasteboard: &NSPasteboard, text: &NSString) {
    pasteboard.clearContents();
    if !pasteboard.setString_forType(text, string_type()) {
        tracing::error!("the clipboard refused the text");
    }
}

pub(super) fn string_type() -> &'static NSPasteboardType {
    // SAFETY: AppKit sets this constant before `main` and never changes it.
    unsafe { NSPasteboardTypeString }
}

pub(super) fn transient_type() -> &'static NSPasteboardType {
    ns_string!("org.nspasteboard.TransientType")
}
