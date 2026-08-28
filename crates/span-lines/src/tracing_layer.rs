//! A [`tracing`] bridge that emits span-lines records.
//!
//! Same record format as the crate's own API, byte for byte, so a consumer
//! choosing between them changes how it *writes* instrumentation and
//! nothing about what a reader sees.
//!
//! # Why a layer rather than a second format
//!
//! `tracing` is already in the dependency tree of anything using calloop,
//! zbus or slint, so the macros cost little to adopt and contributors
//! already know them. What `tracing` does not give is a record shape a
//! journal reader can parse without a library, which is what this layer
//! supplies.
//!
//! # Targets are an allowlist, and it is required
//!
//! Installing a subscriber makes a process start collecting *everything*
//! instrumented in its tree, not just its own spans. For a login screen
//! that matters: zbus events carry D-Bus paths and error strings, and a
//! journal is readable by `adm`. So [`layer`] takes the target prefixes to
//! admit as an argument rather than a builder option that can be forgotten.
//!
//! ```no_run
//! # #[cfg(feature = "tracing")] {
//! use tracing_subscriber::prelude::*;
//!
//! tracing_subscriber::registry()
//!     .with(span_lines::tracing_layer::layer(&["vigil"]))
//!     .init();
//! # }
//! ```
//!
//! # Levels carry the detail tier
//!
//! `INFO` and above is [`Detail::Session`]; `DEBUG` and `TRACE` are
//! [`Detail::Frames`]. So `SPAN_LINES=frames` turns on per-frame spans
//! written as `debug_span!`, and the existing tiering needs no second knob.

use std::fmt::Write as _;
use std::sync::Mutex;

use tracing_core::field::{Field, Visit};
use tracing_core::span::{Attributes, Id, Record};
use tracing_core::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use crate::{encode, AtExit, Detail};

/// Build a layer admitting only spans and events whose target starts with
/// one of `target_prefixes`.
///
/// The detail tier comes from `SPAN_LINES`, as it does for the crate's own
/// API.
pub fn layer(target_prefixes: &[&'static str]) -> SpanLinesLayer {
    SpanLinesLayer::new(target_prefixes, Detail::from_env())
}

/// State the layer keeps for one open span.
struct Open {
    name: &'static str,
    /// Our id, not tracing's: tracing ids are per-subscriber and reused
    /// after close, which would let two spans in one trace share an id.
    id: String,
    parent: Option<String>,
    started: std::time::Instant,
    start_us: u128,
    attrs: String,
    needs: Detail,
}

struct Inner {
    targets: Vec<&'static str>,
    detail: Detail,
    /// Spans opened and not yet closed.
    ///
    /// tracing-subscriber's registry has no iterator over live spans, so
    /// the layer keeps its own set. That is what makes [`crate::exit`] able
    /// to close what is still open instead of losing it.
    open: Mutex<Vec<(u64, Open)>>,
}

/// A [`tracing_subscriber::Layer`] writing span-lines records.
///
/// Cheap to clone, and clones share one set of open spans - so a caller can
/// hand one to the subscriber and keep another to flush with, without the
/// layer needing to be wrapped in an `Arc` (which `tracing-subscriber` does
/// not accept as a layer).
#[derive(Clone)]
pub struct SpanLinesLayer {
    inner: std::sync::Arc<Inner>,
}

impl SpanLinesLayer {
    /// A layer at an explicit detail tier, for a consumer that decides the
    /// tier itself rather than reading `SPAN_LINES`.
    pub fn with_detail(target_prefixes: &[&'static str], detail: Detail) -> Self {
        Self::new(target_prefixes, detail)
    }

    fn new(target_prefixes: &[&'static str], detail: Detail) -> Self {
        Self {
            inner: std::sync::Arc::new(Inner {
                targets: target_prefixes.to_vec(),
                detail,
                open: Mutex::new(Vec::new()),
            }),
        }
    }

    fn detail(&self) -> Detail {
        self.inner.detail
    }

    fn admits(&self, target: &str) -> bool {
        // strip_prefix rather than `format!("{prefix}::")`: the allocation
        // ran on every span and every event, and measured 30-40x the cost
        // of the comparison it was performing.
        self.inner.targets.iter().any(|prefix| {
            target
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
        })
    }

    fn tier(level: &Level) -> Detail {
        if *level <= Level::INFO {
            Detail::Session
        } else {
            Detail::Frames
        }
    }

    fn enabled(&self, level: &Level) -> bool {
        self.detail() != Detail::Off && self.detail() >= Self::tier(level)
    }

    /// Close every span still open, newest first, marking each as cut short.
    ///
    /// A record that says `status=exit` is the difference between a reader
    /// seeing a truncated trace and a reader seeing a crash.
    pub fn close_open(&self) {
        let open = match self.inner.open.lock() {
            Ok(mut open) => std::mem::take(&mut *open),
            Err(_) => return,
        };
        for (_, span) in open.into_iter().rev() {
            self.write_span(&span, Some("exit"));
        }
    }

    fn write_span(&self, span: &Open, status: Option<&str>) {
        let mut line = format!(
            "span={} trace={} id={} parent={} seq={} t_us={} dur_us={}{}",
            encode(span.name),
            crate::process_trace().id(),
            span.id,
            span.parent.as_deref().unwrap_or("-"),
            crate::next_seq(),
            span.start_us,
            span.started.elapsed().as_micros(),
            span.attrs,
        );
        if let Some(status) = status {
            let _ = write!(line, " status={}", encode(status));
        }
        crate::emit(&line);
    }
}

/// The name to record for an event.
///
/// `tracing` synthesises a name from the source location for a plain
/// `info!("...")` - literally `event src/adv.rs:104`. Recording that would
/// put the source path in the journal and change the `event=` key whenever
/// anyone edits a line above the call, so a synthesised name becomes `log`
/// and the text stays in the `message` field where it belongs.
///
/// An event that wants a stable name says so: `event!(name: "flow.transition", ...)`.
fn event_name(name: &'static str) -> &'static str {
    if name.starts_with("event ") {
        "log"
    } else {
        name
    }
}

/// Render tracing field values into span-lines attributes.
struct Attrs(String);

impl Visit for Attrs {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // Field names are `&'static str` in the macros, matching the crate's
        // rule that a key is never runtime text; both are encoded anyway, so
        // a careless literal cannot split a record either.
        let _ = write!(
            self.0,
            " {}={}",
            encode(field.name()),
            encode(&format!("{value:?}"))
        );
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        // Strings without the Debug quotes, which would otherwise be
        // percent-escaped into the value and read back with them attached.
        let _ = write!(self.0, " {}={}", encode(field.name()), encode(value));
    }
}

impl<S> tracing_subscriber::Layer<S> for SpanLinesLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    /// The highest level this layer will ever want.
    ///
    /// Without it the dispatcher leaves every callsite in the process
    /// enabled, so `SPAN_LINES=off` quietened this layer's output while
    /// still paying for every `trace!` in every dependency - and any
    /// library gating expensive work on `tracing::enabled!()` would still
    /// do that work.
    fn max_level_hint(&self) -> Option<tracing_core::LevelFilter> {
        Some(match self.detail() {
            Detail::Off => tracing_core::LevelFilter::OFF,
            Detail::Session => tracing_core::LevelFilter::INFO,
            Detail::Frames => tracing_core::LevelFilter::TRACE,
        })
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let meta = attrs.metadata();
        if !self.enabled(meta.level()) || !self.admits(meta.target()) {
            return;
        }
        let mut fields = Attrs(String::new());
        attrs.record(&mut fields);
        // Parent is the nearest ancestor this layer actually recorded, so a
        // record never names a span that was filtered out and never
        // printed.
        let parent = ctx
            .span_scope(id)
            .into_iter()
            .flatten()
            .skip(1)
            .find_map(|span| self.id_of(span.id().into_u64()));
        let started = std::time::Instant::now();
        let open = Open {
            name: meta.name(),
            id: crate::hex(8),
            parent,
            started,
            start_us: crate::micros_since_origin(started),
            attrs: fields.0,
            needs: Self::tier(meta.level()),
        };
        if let Ok(mut live) = self.inner.open.lock() {
            live.push((id.into_u64(), open));
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        // Gate before visiting. Without this, every `Span::record` in every
        // foreign crate formatted its values into a String this layer threw
        // away - measured at 1000 wasted `Debug::fmt` calls for a
        // filtered-out target at SPAN_LINES=off - and the "we never touch
        // another crate's field values" property behind the allowlist was
        // true of on_new_span and on_event only.
        let Some(meta) = ctx.metadata(id) else {
            return;
        };
        if !self.enabled(meta.level()) || !self.admits(meta.target()) {
            return;
        }
        let mut fields = Attrs(String::new());
        values.record(&mut fields);
        if let Ok(mut live) = self.inner.open.lock() {
            if let Some((_, span)) = live.iter_mut().find(|(raw, _)| *raw == id.into_u64()) {
                span.attrs.push_str(&fields.0);
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let meta = event.metadata();
        if !self.enabled(meta.level()) || !self.admits(meta.target()) {
            return;
        }
        let parent = ctx
            .event_scope(event)
            .into_iter()
            .flatten()
            .find_map(|span| self.id_of(span.id().into_u64()));
        let mut fields = Attrs(String::new());
        event.record(&mut fields);
        crate::emit(&format!(
            "event={} trace={} parent={} seq={} t_us={}{}",
            encode(event_name(meta.name())),
            crate::process_trace().id(),
            parent.as_deref().unwrap_or("-"),
            crate::next_seq(),
            crate::micros_since_origin(std::time::Instant::now()),
            fields.0,
        ));
    }

    fn on_close(&self, id: Id, _ctx: Context<'_, S>) {
        let taken = match self.inner.open.lock() {
            Ok(mut live) => live
                .iter()
                .position(|(raw, _)| *raw == id.into_u64())
                .map(|at| live.remove(at).1),
            Err(_) => None,
        };
        if let Some(span) = taken {
            if self.detail() >= span.needs {
                // Same rule as the crate's own Drop: a span unwound by a
                // panic must not be byte-identical to one that completed,
                // or a crashed lock reads as a successful one.
                self.write_span(&span, crate::drop_status(std::thread::panicking()));
            }
        }
    }
}

impl SpanLinesLayer {
    fn id_of(&self, raw: u64) -> Option<String> {
        self.inner
            .open
            .lock()
            .ok()?
            .iter()
            .find(|(id, _)| *id == raw)
            .map(|(_, span)| span.id.clone())
    }
}

/// Flush handle registered with [`crate::at_exit`] by [`install`].
struct CloseOnExit(SpanLinesLayer);

impl AtExit for CloseOnExit {
    fn flush(&self) {
        self.0.close_open();
    }
}

/// Build a layer and register it to close its open spans on [`crate::exit`].
///
/// This is the constructor to prefer: a layer built with [`layer`] and never
/// registered loses whatever is still open when the process exits, which is
/// the failure this crate's `end()` exists to prevent for its own API.
pub fn install(target_prefixes: &[&'static str]) -> SpanLinesLayer {
    let layer = SpanLinesLayer::new(target_prefixes, Detail::from_env());
    crate::at_exit(CloseOnExit(layer.clone()));
    layer
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::prelude::*;

    /// Run `body` with a layer installed on this thread only, and return
    /// the records it wrote.
    fn records(detail: Detail, body: impl FnOnce()) -> Vec<String> {
        crate::test_drain();
        let layer = SpanLinesLayer::with_detail(&["admitted"], detail);
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, body);
        crate::test_drain()
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
    fn a_span_is_written_in_the_documented_field_order() {
        // The whole point of the bridge: a reader cannot tell which API
        // wrote a record.
        let written = records(Detail::Session, || {
            let _span = tracing::info_span!(target: "admitted", "lock.session").entered();
        });
        assert_eq!(written.len(), 1, "{written:?}");
        let keys: Vec<&str> = written[0]
            .split_whitespace()
            .filter_map(|t| t.split_once('='))
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            keys,
            ["span", "trace", "id", "parent", "seq", "t_us", "dur_us"],
            "{}",
            written[0]
        );
        assert_eq!(field(&written[0], "span"), "lock.session");
        assert_eq!(field(&written[0], "parent"), "-");
        assert_eq!(field(&written[0], "id").len(), 16);
        assert_eq!(field(&written[0], "trace").len(), 32);
    }

    #[test]
    fn both_apis_write_records_a_reader_can_join() {
        // The claim this bridge rests on. It is not about field order - it
        // is that a consumer mixing the two APIs produces one trace. The
        // failure mode is silent and environment-dependent: with a
        // TRACEPARENT set both adopt the same id and it looks fine; with
        // none set - a locker started by a compositor rather than a shell
        // wrapper - they would mint separate ids and nothing joins.
        crate::test_drain();
        let layer = SpanLinesLayer::with_detail(&["admitted"], Detail::Session);
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let _via_layer = tracing::info_span!(target: "admitted", "lock.session").entered();
        });
        let via_layer = crate::test_drain().pop().expect("layer record");

        let direct = crate::Trace::from_env().span("lock.session");
        drop(direct);
        let via_direct = crate::test_drain().pop().expect("direct record");

        assert_eq!(
            field(&via_layer, "trace"),
            field(&via_direct, "trace"),
            "both APIs must write one trace:\n  {via_layer}\n  {via_direct}"
        );
        let keys = |line: &str| -> Vec<String> {
            line.split_whitespace()
                .filter_map(|t| t.split_once('='))
                .map(|(k, _)| k.to_string())
                .collect()
        };
        assert_eq!(keys(&via_layer), keys(&via_direct), "field sets must match");
        assert_ne!(
            field(&via_layer, "id"),
            field(&via_direct, "id"),
            "distinct spans must still have distinct ids"
        );
        // One sequence, or a reader cannot order records across the APIs.
        let a: u64 = field(&via_layer, "seq").parse().unwrap();
        let b: u64 = field(&via_direct, "seq").parse().unwrap();
        assert!(a < b, "seq must be one shared counter: {a} then {b}");
    }

    #[test]
    fn a_target_outside_the_allowlist_is_never_recorded() {
        // Installing a subscriber makes a process collect everything in its
        // tree - zbus, calloop, slint. On a login screen that is a
        // disclosure question, so the allowlist is the whole safety story.
        let written = records(Detail::Frames, || {
            let _mine = tracing::info_span!(target: "admitted", "lock.session").entered();
            let _theirs = tracing::info_span!(target: "zbus", "dbus.call").entered();
            tracing::info!(target: "zbus", "connected");
        });
        assert_eq!(written.len(), 1, "only the admitted span: {written:?}");
        assert_eq!(field(&written[0], "span"), "lock.session");
    }

    #[test]
    fn a_prefix_matches_only_on_a_module_boundary() {
        // "admitted" must not admit "admitted_evil".
        let written = records(Detail::Session, || {
            let _ = tracing::info_span!(target: "admitted::inner", "yes").entered();
            let _ = tracing::info_span!(target: "admittedly", "no").entered();
        });
        let names: Vec<String> = written.iter().map(|r| field(r, "span")).collect();
        assert_eq!(names, ["yes"], "{written:?}");
    }

    #[test]
    fn errors_and_warnings_are_session_tier() {
        // The comparison that decides this is the one place an inverted
        // operator would be catastrophic: ERROR would become frame-tier and
        // go silent by default. The previous test only exercised INFO and
        // DEBUG, so it could not see that.
        let written = records(Detail::Session, || {
            let _e = tracing::error_span!(target: "admitted", "boom").entered();
            let _w = tracing::warn_span!(target: "admitted", "careful").entered();
            let _i = tracing::info_span!(target: "admitted", "normal").entered();
            let _t = tracing::trace_span!(target: "admitted", "chatter").entered();
        });
        let mut names: Vec<String> = written.iter().map(|r| field(r, "span")).collect();
        names.sort();
        assert_eq!(names, ["boom", "careful", "normal"], "{written:?}");
    }

    #[test]
    fn the_level_hint_follows_the_detail_tier() {
        // Without a hint the dispatcher leaves every callsite in the
        // process enabled, so SPAN_LINES=off quietens this layer's output
        // while every trace! in every dependency still dispatches, and
        // anything gating work on `tracing::enabled!()` still does it.
        //
        // Asserted on the hint itself rather than on
        // `LevelFilter::current()`: that is process-global, and the other
        // tests in this binary install their own subscribers in parallel,
        // so the observed global max is whatever the loudest of them wants.
        type Reg = tracing_subscriber::Registry;
        for (detail, expected) in [
            (Detail::Off, tracing_core::LevelFilter::OFF),
            (Detail::Session, tracing_core::LevelFilter::INFO),
            (Detail::Frames, tracing_core::LevelFilter::TRACE),
        ] {
            let layer = SpanLinesLayer::with_detail(&["admitted"], detail);
            assert_eq!(
                tracing_subscriber::Layer::<Reg>::max_level_hint(&layer),
                Some(expected),
                "{detail:?}"
            );
        }
    }

    #[test]
    fn a_foreign_span_s_fields_are_never_formatted() {
        // The allowlist's promise is that this process does not touch
        // another crate's field values. on_record bypassed it, so every
        // Span::record in zbus or calloop was formatted and thrown away.
        use std::sync::atomic::{AtomicUsize, Ordering};
        static FORMATTED: AtomicUsize = AtomicUsize::new(0);
        struct Counted;
        impl std::fmt::Debug for Counted {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                FORMATTED.fetch_add(1, Ordering::Relaxed);
                f.write_str("counted")
            }
        }

        FORMATTED.store(0, Ordering::Relaxed);
        records(Detail::Frames, || {
            let foreign =
                tracing::info_span!(target: "zbus", "dbus.call", value = tracing::field::Empty);
            for _ in 0..100 {
                foreign.record("value", tracing::field::debug(Counted));
            }
        });
        assert_eq!(
            FORMATTED.load(Ordering::Relaxed),
            0,
            "a filtered-out span's values must never be formatted"
        );

        // An admitted span still records normally.
        FORMATTED.store(0, Ordering::Relaxed);
        let written = records(Detail::Session, || {
            let mine = tracing::info_span!(target: "admitted", "lock.session", value = tracing::field::Empty);
            let _entered = mine.enter();
            mine.record("value", tracing::field::debug(Counted));
        });
        assert_eq!(FORMATTED.load(Ordering::Relaxed), 1);
        assert_eq!(field(&written[0], "value"), "counted");
    }

    #[test]
    fn the_level_selects_the_detail_tier() {
        // debug_span is frame-tier: silent by default, so a settled process
        // stays quiet without a second knob.
        let quiet = records(Detail::Session, || {
            let _session = tracing::info_span!(target: "admitted", "lock.session").entered();
            let _frame = tracing::debug_span!(target: "admitted", "frame.present").entered();
        });
        let names: Vec<String> = quiet.iter().map(|r| field(r, "span")).collect();
        assert_eq!(names, ["lock.session"], "{quiet:?}");

        let loud = records(Detail::Frames, || {
            let _session = tracing::info_span!(target: "admitted", "lock.session").entered();
            let _frame = tracing::debug_span!(target: "admitted", "frame.present").entered();
        });
        assert_eq!(loud.len(), 2, "{loud:?}");

        let off = records(Detail::Off, || {
            let _session = tracing::info_span!(target: "admitted", "lock.session").entered();
        });
        assert!(off.is_empty(), "{off:?}");
    }

    #[test]
    fn a_child_names_a_parent_that_was_actually_written() {
        let written = records(Detail::Session, || {
            let outer = tracing::info_span!(target: "admitted", "lock.session");
            let _outer = outer.enter();
            let inner = tracing::info_span!(target: "admitted", "flow.phase");
            drop(inner.enter());
        });
        assert_eq!(written.len(), 2, "{written:?}");
        // Children close first.
        let (child, parent) = (&written[0], &written[1]);
        assert_eq!(field(child, "span"), "flow.phase");
        assert_eq!(field(child, "parent"), field(parent, "id"));
    }

    #[test]
    fn a_filtered_ancestor_is_skipped_rather_than_dangling() {
        // A frame span is silent at session tier. A child of it must not
        // point at an id that appears nowhere in the journal - the same
        // orphan-record rule the crate's own API follows.
        let written = records(Detail::Session, || {
            let root = tracing::info_span!(target: "admitted", "lock.session");
            let _root = root.enter();
            let frame = tracing::debug_span!(target: "admitted", "frame.present");
            let _frame = frame.enter();
            let inner = tracing::info_span!(target: "admitted", "frame.subtask");
            drop(inner.enter());
        });
        let ids: Vec<String> = written.iter().map(|r| field(r, "id")).collect();
        for record in &written {
            let parent = field(record, "parent");
            assert!(
                parent == "-" || ids.contains(&parent),
                "record names an unwritten parent: {record}"
            );
        }
        let subtask = written
            .iter()
            .find(|r| field(r, "span") == "frame.subtask")
            .expect("subtask must be written");
        let root = written
            .iter()
            .find(|r| field(r, "span") == "lock.session")
            .expect("root must be written");
        assert_eq!(
            field(subtask, "parent"),
            field(root, "id"),
            "the silent frame span must be skipped, not named"
        );
    }

    #[test]
    fn an_event_carries_its_fields_and_its_span() {
        let written = records(Detail::Session, || {
            let _phase = tracing::info_span!(target: "admitted", "flow.phase").entered();
            tracing::event!(
                name: "flow.transition",
                target: "admitted",
                tracing::Level::INFO,
                from = "Committing",
                to = "Locked"
            );
        });
        let event = written
            .iter()
            .find(|r| r.starts_with("event="))
            .unwrap_or_else(|| panic!("no event in {written:?}"));
        assert_eq!(field(event, "event"), "flow.transition");
        assert_eq!(field(event, "from"), "Committing");
        assert_eq!(field(event, "to"), "Locked");
        let span = written.iter().find(|r| r.starts_with("span=")).unwrap();
        assert_eq!(field(event, "parent"), field(span, "id"));
    }

    #[test]
    fn a_plain_log_macro_does_not_put_a_source_path_in_the_record() {
        // tracing names a bare `info!("...")` after its call site, so the
        // key would change whenever someone edits a line above it, and the
        // source path would land in a journal readable by adm.
        assert_eq!(event_name("event src/adv.rs:104"), "log");
        assert_eq!(event_name("flow.transition"), "flow.transition");

        let written = records(Detail::Session, || {
            tracing::info!(target: "admitted", peer = "org.freedesktop.login1", "connected");
        });
        let event = written.iter().find(|r| r.starts_with("event=")).unwrap();
        assert_eq!(field(event, "event"), "log");
        assert!(!event.contains(".rs"), "source path leaked: {event}");
        assert_eq!(field(event, "message"), "connected");
        assert_eq!(field(event, "peer"), "org.freedesktop.login1");
    }

    #[test]
    fn field_values_are_encoded_like_the_crate_s_own_api() {
        let written = records(Detail::Session, || {
            let _ = tracing::info_span!(
                target: "admitted",
                "lock.session",
                note = "two words",
                bad = "a=b\nspan=forged"
            )
            .entered();
        });
        assert_eq!(written.len(), 1, "a value must not split a record");
        assert_eq!(field(&written[0], "note"), "two%20words");
        assert_eq!(field(&written[0], "bad"), "a%3Db%0Aspan%3Dforged");
    }

    #[test]
    fn a_span_unwound_by_a_panic_says_so_through_the_layer_too() {
        // The crate's own API grew `status=panic` because a crashed lock
        // and a clean one otherwise read the same. The bridge has to keep
        // that, or adopting it silently loses the distinction.
        crate::test_drain();
        let layer = SpanLinesLayer::with_detail(&["admitted"], Detail::Session);
        let subscriber = tracing_subscriber::registry().with(layer);
        let panicked = tracing::subscriber::with_default(subscriber, || {
            std::panic::catch_unwind(|| {
                let _span = tracing::info_span!(target: "admitted", "lock.session").entered();
                std::panic::panic_any("boom");
            })
        });
        assert!(panicked.is_err(), "the test's own panic must have fired");
        let written = crate::test_drain();
        assert_eq!(written.len(), 1, "{written:?}");
        assert_eq!(
            field(&written[0], "status"),
            "panic",
            "a span unwound by a panic must be distinguishable: {}",
            written[0]
        );

        // ... and a clean close still carries no status.
        crate::test_drain();
        let layer = SpanLinesLayer::with_detail(&["admitted"], Detail::Session);
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let _span = tracing::info_span!(target: "admitted", "lock.session").entered();
        });
        let clean = crate::test_drain().pop().unwrap();
        assert!(!clean.contains(" status="), "{clean}");
    }

    #[test]
    fn open_spans_are_closed_and_marked_when_the_process_exits() {
        // std::process::exit runs no destructors, so without this the root
        // span of every abrupt exit is simply missing - and a reader cannot
        // tell "never started" from "still running" from "died".
        crate::test_drain();
        let layer = SpanLinesLayer::with_detail(&["admitted"], Detail::Session);
        let subscriber = tracing_subscriber::registry().with(layer.clone());
        let guard = tracing::subscriber::set_default(subscriber);
        let root = tracing::info_span!(target: "admitted", "lock.session");
        let entered = root.enter();
        assert!(crate::test_drain().is_empty(), "nothing written while open");

        layer.close_open();
        let written = crate::test_drain();
        assert_eq!(written.len(), 1, "{written:?}");
        assert_eq!(field(&written[0], "span"), "lock.session");
        assert_eq!(
            field(&written[0], "status"),
            "exit",
            "a cut-short span must say so: {}",
            written[0]
        );

        drop(entered);
        drop(guard);
        assert!(
            crate::test_drain().is_empty(),
            "a span closed by exit must not be written again on drop"
        );
    }
}
