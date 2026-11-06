use bloop_core::domain::Generation;
use bloop_core::session::{Command, Core, Event};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardType, NSPasteboardWriting};
use objc2_core_graphics::CGEvent;
use objc2_foundation::{NSArray, NSData, NSInteger, ns_string};

use bloop_core::session::InsertOutcome;

use super::clipboard::save_within;
use super::{
    KEY_V, Target, Targets, Transaction, is_posted, mark_posted, restore, save, string_type,
    transient_type, write_text,
};

/// The data of each item of the pasteboard, by type, in item order.
type Contents = Vec<Vec<(String, Vec<u8>)>>;

fn generations(count: usize) -> Vec<Generation> {
    let mut core = Core::new();
    (0..count)
        .map(|_| {
            let [Command::StartHoldTimer(generation)] = core.handle(Event::DictationKeyDown)[..]
            else {
                panic!("one hold timer");
            };
            assert_eq!(core.handle(Event::DictationKeyUp), [] as [Command; 0]);
            generation
        })
        .collect()
}

fn target(process: i32) -> Target {
    Target {
        process: Some(process),
        inputs: [0; 4],
    }
}

fn item(types: &[(&NSPasteboardType, &[u8])]) -> Retained<NSPasteboardItem> {
    let item = NSPasteboardItem::new();
    for (data_type, data) in types {
        assert!(item.setData_forType(&NSData::with_bytes(data), data_type));
    }
    item
}

fn bloop_text() -> Retained<NSPasteboardItem> {
    item(&[(string_type(), &b"Bloop"[..])])
}

/// Replaces the pasteboard content with these items, as another application does.
fn write(pasteboard: &NSPasteboard, items: &[Retained<NSPasteboardItem>]) -> NSInteger {
    pasteboard.clearContents();
    let objects: Vec<&ProtocolObject<dyn NSPasteboardWriting>> = items
        .iter()
        .map(|item| ProtocolObject::from_ref(&**item))
        .collect();
    assert!(pasteboard.writeObjects(&NSArray::from_slice(&objects)));
    pasteboard.changeCount()
}

fn contents(pasteboard: &NSPasteboard) -> Contents {
    let items = pasteboard.pasteboardItems().expect("pasteboard items");
    items
        .iter()
        .map(|item| {
            let mut types: Vec<(String, Vec<u8>)> = item
                .types()
                .iter()
                .map(|data_type| {
                    let data = item.dataForType(&data_type).expect("data for type");
                    (data_type.to_string(), data.to_vec())
                })
                .collect();
            types.sort();
            types
        })
        .collect()
}

fn byte_count(contents: &Contents) -> usize {
    contents.iter().flatten().map(|(_, data)| data.len()).sum()
}

fn with_transient_marker(contents: Contents) -> Contents {
    contents
        .into_iter()
        .map(|mut types| {
            types.push((transient_type().to_string(), Vec::new()));
            types.sort();
            types
        })
        .collect()
}

#[test]
fn take_returns_the_target_of_the_generation() {
    let [first, second] = generations(2)[..] else {
        panic!("two generations");
    };
    let mut targets = Targets::default();
    targets.record(first, target(1));
    targets.record(second, target(2));

    assert_eq!(targets.take(first), Some(target(1)));
    assert_eq!(targets.take(second), Some(target(2)));
    assert_eq!(targets.take(second), None);
}

#[test]
fn take_keeps_the_other_targets() {
    let [waiting, pasted] = generations(2)[..] else {
        panic!("two generations");
    };
    let mut targets = Targets::default();
    targets.record(waiting, target(1));
    targets.record(pasted, target(2));

    assert_eq!(targets.take(pasted), Some(target(2)));
    assert_eq!(targets.take(waiting), Some(target(1)));
}

#[test]
fn repeated_jobs_without_a_paste_keep_no_target() {
    let mut targets = Targets::default();

    for generation in generations(1_000) {
        targets.record(generation, target(1));
        targets.remove(generation);
    }

    assert_eq!(targets.0.len(), 0);
}

#[test]
fn an_unknown_generation_keeps_the_targets() {
    let [recorded, unknown] = generations(2)[..] else {
        panic!("two generations");
    };
    let mut targets = Targets::default();
    targets.record(recorded, target(1));

    assert_eq!(targets.take(unknown), None);
    assert_eq!(targets.take(recorded), Some(target(1)));
}

#[test]
fn only_marked_events_are_posted_by_bloop() {
    let event = CGEvent::new_keyboard_event(None, KEY_V, true).expect("keyboard event");
    assert!(!is_posted(&event));

    mark_posted(&event);

    assert!(is_posted(&event));
}

#[test]
fn a_read_reports_landed_only_after_the_restore() {
    let mut transaction = Transaction::default();

    assert!(transaction.read());
    assert!(!transaction.read());
    assert!(!transaction.read_time_out());
    assert!(transaction.restore_due());
    assert_eq!(
        transaction.result(InsertOutcome::Landed),
        Some(InsertOutcome::Landed)
    );
    assert!(!transaction.restore_due());
}

#[test]
fn without_a_read_bloop_gives_up_once_and_restores_nothing() {
    let mut transaction = Transaction::default();

    assert!(!transaction.restore_due());
    assert!(transaction.read_time_out());
    assert!(!transaction.read_time_out());
    assert!(!transaction.read());
    assert!(!transaction.restore_due());
}

#[test]
fn a_cancel_keeps_the_restore_and_drops_the_result() {
    let mut transaction = Transaction::default();
    assert!(transaction.read());

    transaction.cancel();

    assert!(transaction.restore_due());
    assert_eq!(transaction.result(InsertOutcome::Landed), None);
}

#[test]
fn restore_writes_the_saved_items_back_with_the_transient_marker() {
    let pasteboard = NSPasteboard::pasteboardWithUniqueName();
    let first = item(&[
        (ns_string!("com.example.bloop.alpha"), &b"alpha"[..]),
        (ns_string!("com.example.bloop.beta"), &b"beta"[..]),
    ]);
    let second = item(&[
        (ns_string!("com.example.bloop.gamma"), &[0, 1, 2, 255][..]),
        (ns_string!("com.example.bloop.delta"), &[][..]),
    ]);
    write(&pasteboard, &[first, second]);
    let before = contents(&pasteboard);
    let saved = save(&pasteboard);
    let written = write(&pasteboard, &[bloop_text()]);

    restore(&pasteboard, saved, written);

    assert_eq!(contents(&pasteboard), with_transient_marker(before));
    pasteboard.clearContents();
}

#[test]
fn save_within_keeps_a_clipboard_at_the_item_and_byte_bounds() {
    let pasteboard = NSPasteboard::pasteboardWithUniqueName();
    write(
        &pasteboard,
        &[
            item(&[(ns_string!("com.example.bloop.alpha"), &b"alpha"[..])]),
            item(&[(ns_string!("com.example.bloop.beta"), &b"beta"[..])]),
        ],
    );
    let before = contents(&pasteboard);
    let saved = save_within(&pasteboard, before.len(), byte_count(&before));
    let written = write(&pasteboard, &[bloop_text()]);

    restore(&pasteboard, saved, written);

    assert_eq!(contents(&pasteboard), with_transient_marker(before));
    pasteboard.clearContents();
}

#[test]
fn save_within_gives_nothing_for_one_item_above_the_byte_bound() {
    let pasteboard = NSPasteboard::pasteboardWithUniqueName();
    write(
        &pasteboard,
        &[item(&[(
            ns_string!("com.example.bloop.alpha"),
            &[7; 16][..],
        )])],
    );
    let bytes = byte_count(&contents(&pasteboard));

    assert!(save_within(&pasteboard, 1, bytes - 1).is_none());
    pasteboard.clearContents();
}

#[test]
fn save_within_gives_nothing_for_more_items_than_the_item_bound() {
    let pasteboard = NSPasteboard::pasteboardWithUniqueName();
    write(
        &pasteboard,
        &[
            item(&[(ns_string!("com.example.bloop.alpha"), &b"alpha"[..])]),
            item(&[(ns_string!("com.example.bloop.beta"), &b"beta"[..])]),
            item(&[(ns_string!("com.example.bloop.gamma"), &b"gamma"[..])]),
        ],
    );

    assert!(save_within(&pasteboard, 2, usize::MAX).is_none());
    pasteboard.clearContents();
}

#[test]
fn restore_leaves_an_empty_clipboard_empty() {
    let pasteboard = NSPasteboard::pasteboardWithUniqueName();
    pasteboard.clearContents();
    let saved = save(&pasteboard);
    assert_eq!(saved.as_ref().map(Vec::len), Some(0));
    let written = write(&pasteboard, &[bloop_text()]);

    restore(&pasteboard, saved, written);

    assert_eq!(contents(&pasteboard), Contents::new());
    pasteboard.clearContents();
}

#[test]
fn restore_keeps_a_clipboard_that_changed_after_the_paste() {
    let pasteboard = NSPasteboard::pasteboardWithUniqueName();
    write(
        &pasteboard,
        &[item(&[(
            ns_string!("com.example.bloop.alpha"),
            &b"alpha"[..],
        )])],
    );
    let saved = save(&pasteboard);
    let written = write(&pasteboard, &[bloop_text()]);
    write(&pasteboard, &[item(&[(string_type(), &b"other"[..])])]);
    let changed = contents(&pasteboard);

    restore(&pasteboard, saved, written);

    assert_eq!(contents(&pasteboard), changed);
    pasteboard.clearContents();
}

#[test]
fn the_change_count_after_the_write_is_the_one_that_clear_contents_returned() {
    let pasteboard = NSPasteboard::pasteboardWithUniqueName();
    let cleared = pasteboard.clearContents();
    let item = bloop_text();
    assert!(pasteboard.writeObjects(&NSArray::from_slice(&[ProtocolObject::from_ref(&*item)])));

    assert_eq!(pasteboard.changeCount(), cleared);
    pasteboard.clearContents();
}

#[test]
fn write_text_leaves_one_string_item() {
    let pasteboard = NSPasteboard::pasteboardWithUniqueName();
    write(
        &pasteboard,
        &[
            item(&[(ns_string!("com.example.bloop.alpha"), &b"alpha"[..])]),
            item(&[(ns_string!("com.example.bloop.beta"), &b"beta"[..])]),
        ],
    );

    write_text(&pasteboard, ns_string!("Bloop"));

    let items = pasteboard.pasteboardItems().expect("pasteboard items");
    assert_eq!(items.len(), 1);
    let text = items
        .iter()
        .next()
        .and_then(|item| item.stringForType(string_type()))
        .map(|text| text.to_string());
    assert_eq!(text, Some("Bloop".to_owned()));
    pasteboard.clearContents();
}
