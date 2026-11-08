//! Captures provider log fields for privacy regression tests.

use std::fmt;
use std::sync::{Arc, Mutex, Once};

use tracing::field::{Field, Visit};
use tracing::{Event, Metadata, Subscriber, span};

#[derive(Clone, Default)]
pub(crate) struct Log(Arc<Mutex<String>>);

impl Log {
    pub(crate) fn text(&self) -> String {
        self.0.lock().unwrap().clone()
    }
}

/// Installs a global `Log` one time. Without it, a thread without a subscriber can register a callsite first,
/// and the callsite then keeps `Interest::never()`, also for the scoped `Log` of a privacy test.
pub(crate) fn install_fallback() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        tracing::subscriber::set_global_default(Log::default())
            .expect("no other global subscriber");
    });
}

impl Subscriber for Log {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }

    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}

    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut text = self.0.lock().unwrap();
        event.record(&mut Fields(&mut text));
    }

    fn enter(&self, _: &span::Id) {}

    fn exit(&self, _: &span::Id) {}
}

struct Fields<'a>(&'a mut String);

impl Visit for Fields<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        use std::fmt::Write as _;

        writeln!(self.0, "{}={value:?}", field.name()).unwrap();
    }
}
