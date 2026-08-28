//! OpenTelemetry-shaped spans and events, written as journald lines.
//!
//! The OTel *data model* is what makes ordering answerable: a span has a
//! parent, a start and a duration, so "what happened inside what" is a
//! property of the data rather than something reconstructed from the order
//! lines happen to appear in. The SDK, an OTLP exporter and a collector are
//! all downstream of that model and none of them are here: records go to
//! stderr, journald stamps `__MONOTONIC_TIMESTAMP`, `_PID`, `_BOOT_ID` and
//! `_SYSTEMD_UNIT` on them for free, and converting the result to OTLP is a
//! text-processing job for whoever wants one.
//!
//! That matters because the first consumer is a screen locker, whose
//! `deny.toml` opens with "supply-chain gate for the binary that IS the
//! login screen". This crate depends on `std` and nothing else, opens no
//! sockets, and spawns no threads.
//!
//! # Shape
//!
//! ```text
//! span=lock.session   trace=<32hex> id=<16hex> parent=- dur_ms=2413 outcome=Unlocked
//! span=flow.phase     trace=<32hex> id=<16hex> parent=<16hex> dur_ms=560 phase=PreLock
//! event=flow.transition trace=<32hex> parent=<16hex> from=Committing to=Locked
//! ```
//!
//! A span emits when it is dropped, so it cannot be left unended and its
//! duration cannot be forgotten. Ordering is read from `parent` plus
//! journald's timestamps.
//!
//! # Propagation
//!
//! [`Trace::from_env`] adopts a W3C `TRACEPARENT` if the parent process set
//! one, so a lock that begins in a shell script and ends in a locker is one
//! trace. [`Span::traceparent`] produces the value to hand to a child.

use std::cell::Cell;
use std::fmt::Write as _;
use std::io::Write as _;
use std::time::Instant;

/// How much to emit. Read once from `SPAN_LINES`.
///
/// The split exists because answering "in what order did these happen"
/// needs per-frame records while normal operation does not: a settled
/// session should be silent, and a bounded ramp is worth tens of records.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Detail {
    /// Emit nothing.
    Off,
    /// Session lifetime, phases and transitions. The default.
    Session,
    /// Adds per-wake and per-frame spans.
    Frames,
}

impl Detail {
    fn from_env() -> Self {
        Self::parse(std::env::var("SPAN_LINES").ok().as_deref())
    }

    /// Split out from [`Detail::from_env`] so the policy is testable without
    /// mutating process-global environment from parallel test threads.
    fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("off") => Detail::Off,
            Some("frames") => Detail::Frames,
            // Anything else, including unset and a typo, is the safe middle:
            // a typo must not silently turn tracing off.
            _ => Detail::Session,
        }
    }
}

fn hex(bytes: usize) -> String {
    // Ids need to be unique, not unguessable: they correlate lines in one
    // user's journal. /dev/urandom keeps that true across a fork without
    // pulling in a PRNG crate; the clock fallback keeps it working if
    // /dev/urandom is unavailable, at the cost of collisions in the same
    // nanosecond, which would mean two traces in one process anyway.
    let mut out = String::with_capacity(bytes * 2);
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let mut raw = vec![0_u8; bytes];
        if std::io::Read::read_exact(&mut f, &mut raw).is_ok() {
            for b in raw {
                let _ = write!(out, "{b:02x}");
            }
            return out;
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mut mixed = nanos ^ (pid << 64);
    for _ in 0..bytes {
        let _ = write!(out, "{:02x}", (mixed & 0xff) as u8);
        mixed = mixed.rotate_right(8);
    }
    out
}

/// A trace: an id shared by every span in one causal chain, and the detail
/// level those spans honour.
#[derive(Clone, Debug)]
pub struct Trace {
    id: String,
    detail: Detail,
    /// Span id inherited from a parent process, if any.
    inherited_parent: Option<String>,
}

impl Trace {
    /// Adopt `TRACEPARENT` if the parent process set a well-formed one,
    /// otherwise begin a new trace.
    ///
    /// A malformed value is ignored rather than rejected: a broken header
    /// from some other tool must not stop a session tracing itself.
    pub fn from_env() -> Self {
        let detail = Detail::from_env();
        if let Ok(tp) = std::env::var("TRACEPARENT") {
            if let Some((trace, parent)) = parse_traceparent(&tp) {
                return Self {
                    id: trace,
                    detail,
                    inherited_parent: Some(parent),
                };
            }
        }
        Self {
            id: hex(16),
            detail,
            inherited_parent: None,
        }
    }

    /// A trace with a known id, for tests and for callers that carry the id
    /// themselves.
    pub fn with_id(id: impl Into<String>, detail: Detail) -> Self {
        Self {
            id: id.into(),
            detail,
            inherited_parent: None,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn detail(&self) -> Detail {
        self.detail
    }

    /// Open a root span. Its parent is whatever a parent process handed us,
    /// or none.
    pub fn span(&self, name: &'static str) -> Span {
        Span::new(
            self.clone(),
            name,
            self.inherited_parent.clone(),
            Detail::Session,
        )
    }
}

fn parse_traceparent(value: &str) -> Option<(String, String)> {
    // 00-<32 hex trace>-<16 hex span>-<2 hex flags>
    let mut parts = value.trim().split('-');
    let version = parts.next()?;
    let trace = parts.next()?;
    let span = parts.next()?;
    let _flags = parts.next()?;
    if version.len() != 2 || trace.len() != 32 || span.len() != 16 {
        return None;
    }
    let hexy = |s: &str| s.bytes().all(|b| b.is_ascii_hexdigit());
    if !hexy(version) || !hexy(trace) || !hexy(span) {
        return None;
    }
    // An all-zero id is "no parent" per the spec, not a parent named zero.
    if trace.bytes().all(|b| b == b'0') || span.bytes().all(|b| b == b'0') {
        return None;
    }
    Some((trace.to_ascii_lowercase(), span.to_ascii_lowercase()))
}

/// An open span. Emits its record when dropped.
pub struct Span {
    trace: Trace,
    name: &'static str,
    id: String,
    parent: Option<String>,
    started: Instant,
    attrs: String,
    /// The detail level at which this span is worth emitting.
    needs: Detail,
    emitted: Cell<bool>,
}

impl Span {
    fn new(trace: Trace, name: &'static str, parent: Option<String>, needs: Detail) -> Self {
        Self {
            trace,
            name,
            id: hex(8),
            parent,
            started: Instant::now(),
            attrs: String::new(),
            needs,
            emitted: Cell::new(false),
        }
    }

    /// A child span, emitted at `Detail::Session`.
    pub fn child(&self, name: &'static str) -> Span {
        Span::new(
            self.trace.clone(),
            name,
            Some(self.id.clone()),
            Detail::Session,
        )
    }

    /// A child span emitted only at `Detail::Frames` — per-wake and
    /// per-frame work, which is worth records while diagnosing and silence
    /// the rest of the time.
    pub fn frame_child(&self, name: &'static str) -> Span {
        Span::new(
            self.trace.clone(),
            name,
            Some(self.id.clone()),
            Detail::Frames,
        )
    }

    /// Attach an attribute. Values containing a space or `=` are quoted so a
    /// reader can always split on whitespace then on the first `=`.
    pub fn attr(mut self, key: &str, value: impl std::fmt::Display) -> Self {
        self.push_attr(key, value);
        self
    }

    /// As [`Span::attr`], for a span already bound to a variable.
    pub fn set(&mut self, key: &str, value: impl std::fmt::Display) {
        self.push_attr(key, value);
    }

    fn push_attr(&mut self, key: &str, value: impl std::fmt::Display) {
        let rendered = value.to_string();
        let _ = write!(self.attrs, " {key}={}", quote(&rendered));
    }

    /// Record something instantaneous inside this span.
    pub fn event(&self, name: &str, attrs: &[(&str, &str)]) {
        if let Some(line) = self.render_event(name, attrs) {
            emit(&line);
        }
    }

    /// The record `event` would write, or `None` if the detail level silences
    /// it. Split out so the shape and the gating are testable without
    /// capturing stderr.
    fn render_event(&self, name: &str, attrs: &[(&str, &str)]) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        let mut line = format!("event={name} trace={} parent={}", self.trace.id, self.id);
        for (k, v) in attrs {
            let _ = write!(line, " {k}={}", quote(v));
        }
        Some(line)
    }

    /// The record this span will write when dropped, or `None` if the detail
    /// level silences it.
    fn render_span(&self) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        Some(format!(
            "span={} trace={} id={} parent={} dur_ms={}{}",
            self.name,
            self.trace.id,
            self.id,
            self.parent.as_deref().unwrap_or("-"),
            self.started.elapsed().as_millis(),
            self.attrs,
        ))
    }

    fn enabled(&self) -> bool {
        self.trace.detail != Detail::Off && self.trace.detail >= self.needs
    }

    /// The W3C header to hand to a child process, so its spans join this
    /// trace.
    pub fn traceparent(&self) -> String {
        format!("00-{}-{}-01", self.trace.id, self.id)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn trace(&self) -> &Trace {
        &self.trace
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if self.emitted.replace(true) {
            return;
        }
        if let Some(line) = self.render_span() {
            emit(&line);
        }
    }
}

fn quote(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".into();
    }
    if value.bytes().any(|b| b == b' ' || b == b'=' || b == b'"') {
        format!("{:?}", value)
    } else {
        value.to_string()
    }
}

fn emit(line: &str) {
    // stderr, because journald already stamps it with a monotonic timestamp
    // and the unit, and because a locker must not depend on a socket being
    // there. A failed write is dropped: tracing must never be able to break
    // the thing it is tracing.
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_w3c_shaped() {
        let t = Trace::with_id(hex(16), Detail::Session);
        assert_eq!(t.id().len(), 32, "trace id must be 32 hex chars");
        let s = t.span("x");
        assert_eq!(s.id().len(), 16, "span id must be 16 hex chars");
        assert!(t.id().bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(s.id().bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn traceparent_round_trips() {
        let t = Trace::with_id("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session);
        let s = t.span("root");
        let tp = s.traceparent();
        let (trace, parent) = parse_traceparent(&tp).expect("our own header must parse");
        assert_eq!(trace, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(parent, s.id());
    }

    #[test]
    fn a_malformed_traceparent_is_ignored_not_fatal() {
        // A broken header from some other tool must not stop a session
        // tracing itself, and must not be adopted as if it were valid.
        for bad in [
            "",
            "garbage",
            "00-tooshort-0000000000000001-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000001",
            "00-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-0000000000000001-01",
            // all-zero ids mean "no parent" in the spec, not a real parent
            "00-00000000000000000000000000000000-0000000000000001-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
        ] {
            assert!(parse_traceparent(bad).is_none(), "accepted {bad:?}");
        }
    }

    #[test]
    fn detail_defaults_to_session_even_for_a_typo() {
        // Turning tracing off by mistyping the variable would be the worst
        // possible failure: silence that looks like health.
        assert_eq!(Detail::parse(None), Detail::Session);
        assert_eq!(Detail::parse(Some("frame")), Detail::Session);
        assert_eq!(Detail::parse(Some("OFF")), Detail::Session);
        assert_eq!(Detail::parse(Some("")), Detail::Session);
        // Only the two documented spellings change anything.
        assert_eq!(Detail::parse(Some("off")), Detail::Off);
        assert_eq!(Detail::parse(Some(" frames\n")), Detail::Frames);
    }

    #[test]
    fn detail_orders_from_quiet_to_loud() {
        // `Span` gates on `trace.detail < span.needs`, so the ordering is
        // load-bearing, not cosmetic.
        assert!(Detail::Off < Detail::Session);
        assert!(Detail::Session < Detail::Frames);
    }

    /// Strip the two random ids so a record's *shape* can be asserted
    /// exactly. Their format is covered by `ids_are_w3c_shaped`.
    fn skeleton(line: &str) -> String {
        line.split_whitespace()
            .map(|f| match f.split_once('=') {
                Some((k @ ("id" | "parent"), v)) if v != "-" => format!("{k}=<id>"),
                _ => f.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn a_root_span_renders_the_documented_shape() {
        let t = Trace::with_id("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session);
        let s = t.span("lock.session").attr("outcome", "Unlocked");
        let line = s
            .render_span()
            .expect("session detail must emit a session span");
        assert_eq!(
            skeleton(&line),
            "span=lock.session trace=4bf92f3577b34da6a3ce929d0e0e4736 id=<id> parent=- dur_ms=0 outcome=Unlocked"
        );
    }

    #[test]
    fn a_child_span_names_its_parent_and_shares_the_trace() {
        let t = Trace::with_id("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session);
        let root = t.span("lock.session");
        let child = root.child("flow.phase").attr("phase", "PreLock");
        let line = child.render_span().unwrap();
        assert!(
            line.contains(&format!("parent={}", root.id())),
            "child must point at its parent; got {line}"
        );
        assert!(line.contains("trace=4bf92f3577b34da6a3ce929d0e0e4736"));
        assert_ne!(child.id(), root.id(), "ids must be distinct");

        let event = child
            .render_event(
                "flow.transition",
                &[("from", "Committing"), ("to", "Locked")],
            )
            .unwrap();
        assert_eq!(
            skeleton(&event),
            "event=flow.transition trace=4bf92f3577b34da6a3ce929d0e0e4736 parent=<id> from=Committing to=Locked"
        );
    }

    #[test]
    fn off_silences_every_record() {
        // The whole point of `off` is that a locker can be told to say
        // nothing at all; a leaking span would defeat it.
        let t = Trace::with_id("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Off);
        let root = t.span("lock.session");
        assert!(root.render_span().is_none());
        assert!(root.render_event("flow.transition", &[]).is_none());
        assert!(root.frame_child("frame.present").render_span().is_none());
    }

    #[test]
    fn frame_spans_are_silent_until_frames_is_asked_for() {
        // A settled session must be silent: per-frame records are for
        // diagnosis, and emitting them by default would make idle noisy.
        let quiet = Trace::with_id("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session);
        let root = quiet.span("lock.session");
        assert!(root.render_span().is_some(), "session spans still emit");
        let frame = root.frame_child("frame.present");
        assert!(
            frame.render_span().is_none(),
            "frame span leaked at session detail"
        );
        assert!(
            frame.render_event("frame.damage", &[]).is_none(),
            "an event inherits its span's gate"
        );

        let loud = Trace::with_id("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Frames);
        let root = loud.span("lock.session");
        assert!(
            root.render_span().is_some(),
            "frames detail keeps session spans"
        );
        assert!(root.frame_child("frame.present").render_span().is_some());
    }

    #[test]
    fn attributes_that_would_break_parsing_are_quoted() {
        assert_eq!(quote("PreLock"), "PreLock");
        assert_eq!(quote("two words"), "\"two words\"");
        assert_eq!(quote("a=b"), "\"a=b\"");
        assert_eq!(quote(""), "\"\"");
    }
}
