//! `MainSink`: delivers events to the driver through the main queue.

use std::cell::RefCell;

use bloop_core::ports::platform::EventSink;
use bloop_core::session::Event;
use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;

type Handler = Box<dyn FnMut(Event)>;

thread_local! {
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

/// Sends events to the handler that [`set_handler`] installs. Any thread can use it.
#[derive(Debug, Clone, Copy, Default)]
pub struct MainSink;

impl EventSink for MainSink {
    fn send(&self, event: Event) {
        DispatchQueue::main().exec_async(move || deliver(event));
    }
}

/// Installs the function that receives each event on the main thread. It replaces the previous one.
///
/// Events that arrive before this call are lost. Call it before the run loop starts.
pub fn set_handler(_mtm: MainThreadMarker, handler: impl FnMut(Event) + 'static) {
    let handler: Handler = Box::new(handler);
    let installed = HANDLER.try_with(|slot| match slot.try_borrow_mut() {
        Ok(mut slot) => {
            *slot = Some(handler);
            true
        }
        Err(_) => false,
    });
    if !matches!(installed, Ok(true)) {
        tracing::error!("the event handler cannot change while it runs");
    }
}

/// Removes the handler and drops the components it owns after the run loop returns.
/// Events that arrive later have no handler.
pub fn clear_handler(_mtm: MainThreadMarker) {
    let removed = HANDLER.try_with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return Err(());
        };
        Ok(slot.take())
    });
    match removed {
        // Drop outside the borrow, because component cleanup can call AppKit.
        Ok(Ok(handler)) => drop(handler),
        Ok(Err(())) | Err(_) => {
            tracing::error!("the event handler cannot be removed while it runs")
        }
    }
}

/// Gives the event to the handler. Call it only from a block on the main queue.
///
/// If the handler is running, the event goes to the end of the main queue. Thus the handler
/// never runs inside itself.
pub(crate) fn deliver(event: Event) {
    let busy = HANDLER.try_with(|slot| match slot.try_borrow_mut() {
        Ok(mut slot) => {
            if let Some(handler) = slot.as_mut() {
                handler(event);
            }
            None
        }
        Err(_) => Some(event),
    });
    if let Ok(Some(event)) = busy {
        DispatchQueue::main().exec_async(move || deliver(event));
    }
}
