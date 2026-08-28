//! The exit path, end to end, in a process of its own.
//!
//! `Sink` and the `AtExit` registry are process-global, so exercising them
//! beside the unit tests would let one test's installed layer fire during
//! another's flush. An integration test binary gets its own process, which
//! is the only way to assert on them honestly.
#![cfg(feature = "tracing")]

use std::sync::{Arc, Mutex};

use tracing_subscriber::prelude::*;

/// A sink that keeps what it was given, which is also the demonstration
/// that the output destination is genuinely pluggable.
#[derive(Clone, Default)]
struct Collector(Arc<Mutex<Vec<String>>>);

impl span_lines::Sink for Collector {
    fn write(&self, record: &[u8]) {
        self.0
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(record).trim_end().to_string());
    }
}

impl Collector {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

fn field(line: &str, key: &str) -> String {
    line.split_whitespace()
        .filter_map(|token| token.split_once('='))
        .find(|(k, _)| *k == key)
        .unwrap_or_else(|| panic!("no {key} in {line}"))
        .1
        .to_string()
}

#[test]
fn an_installed_layer_survives_repeated_flushes() {
    let sink = Collector::default();
    assert!(
        span_lines::set_sink(sink.clone()),
        "first set_sink must win"
    );
    assert!(
        !span_lines::set_sink(Collector::default()),
        "a second set_sink must report that it lost"
    );

    let layer = span_lines::tracing_layer::install(&["exit_path"]);
    tracing_subscriber::registry().with(layer).init();

    // A span left open, as it would be at std::process::exit.
    let first = tracing::info_span!(target: "exit_path", "lock.session");
    let entered = first.enter();
    assert!(sink.take().is_empty(), "nothing is written while open");

    // A consumer's own flush - a signal handler, say.
    span_lines::flush();
    let written = sink.take();
    assert_eq!(written.len(), 1, "{written:?}");
    assert_eq!(field(&written[0], "span"), "lock.session");
    assert_eq!(
        field(&written[0], "status"),
        "exit",
        "a cut-short span must say so: {}",
        written[0]
    );
    drop(entered);

    // The exit path must still be armed. Draining the registry here is what
    // made an independent flush() silently disable it.
    let second = tracing::info_span!(target: "exit_path", "lock.session.again");
    let entered = second.enter();
    sink.take();
    span_lines::flush();
    let written = sink.take();
    assert_eq!(
        written.len(),
        1,
        "flush must remain armed for a later exit(): {written:?}"
    );
    assert_eq!(field(&written[0], "span"), "lock.session.again");
    drop(entered);
}
