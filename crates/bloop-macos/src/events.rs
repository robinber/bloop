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
    install(Box::new(handler));
}

fn install(handler: Handler) {
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
    remove();
}

fn remove() {
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
            tracing::error!("the event handler cannot be removed while it runs");
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

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use bloop_core::session::Event;

    use super::{HANDLER, Handler, deliver, install, remove};

    fn recording(events: &Rc<RefCell<Vec<Event>>>) -> Handler {
        let events = Rc::clone(events);
        Box::new(move |event| events.borrow_mut().push(event))
    }

    /// Records, when it drops, whether the handler slot is free.
    struct DropProbe(Rc<Cell<Option<bool>>>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            let free = HANDLER.with(|slot| slot.try_borrow_mut().is_ok());
            self.0.set(Some(free));
        }
    }

    #[test]
    fn deliver_without_a_handler_does_not_panic() {
        deliver(Event::Escape);
    }

    #[test]
    fn deliver_runs_the_handler() {
        let events = Rc::default();
        install(recording(&events));

        deliver(Event::Escape);

        assert_matches!(events.borrow()[..], [Event::Escape]);
    }

    #[test]
    fn a_deliver_inside_the_handler_does_not_run_the_handler_inside_itself() {
        let calls = Rc::new(Cell::new(0));
        let counted = Rc::clone(&calls);
        install(Box::new(move |_| {
            counted.set(counted.get() + 1);
            deliver(Event::Escape);
        }));

        deliver(Event::Escape);

        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn clearing_drops_the_handler_outside_the_borrow() {
        let free = Rc::new(Cell::new(None));
        let probe = DropProbe(Rc::clone(&free));
        install(Box::new(move |_| {
            let _ = &probe;
        }));
        assert_eq!(free.get(), None);

        remove();

        assert_eq!(free.get(), Some(true));
    }
}
