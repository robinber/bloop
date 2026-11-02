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

/// Reads all items of the clipboard, with the data of all their types.
///
/// Returns `None` if macOS cannot list the items. A type that cannot be read is left out.
pub(super) fn save(pasteboard: &NSPasteboard) -> Option<Vec<SavedItem>> {
    let Some(items) = pasteboard.pasteboardItems() else {
        tracing::warn!("Bloop cannot read the clipboard, thus it will not restore it");
        return None;
    };
    let mut missing = 0_usize;
    let saved = items
        .iter()
        .map(|item| {
            item.types()
                .iter()
                .filter_map(|data_type| {
                    let data = item.dataForType(&data_type);
                    if data.is_none() {
                        missing += 1;
                    }
                    data.map(|data| (data_type, data))
                })
                .collect()
        })
        .collect();
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
