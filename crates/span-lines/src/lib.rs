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

    /// Attach an attribute.
    ///
    /// The key is `&'static str` on purpose. Keys and event names are
    /// written into the record without escaping, so a runtime key is a
    /// record-forgery primitive: a key of `"a outcome"` splits into two
    /// fields, and one containing a newline mints a second, perfectly
    /// well-formed record carrying this process's real `_PID` and
    /// `_SYSTEMD_UNIT`. Requiring a literal makes that uncompilable rather
    /// than merely discouraged. Attacker-influenced text belongs in the
    /// *value*, which is encoded.
    pub fn attr(mut self, key: &'static str, value: impl std::fmt::Display) -> Self {
        self.push_attr(key, value);
        self
    }

    /// As [`Span::attr`], for a span already bound to a variable.
    pub fn set(&mut self, key: &'static str, value: impl std::fmt::Display) {
        self.push_attr(key, value);
    }

    fn push_attr(&mut self, key: &'static str, value: impl std::fmt::Display) {
        let rendered = value.to_string();
        let _ = write!(self.attrs, " {key}={}", encode(&rendered));
    }

    /// Record something instantaneous inside this span.
    pub fn event(&self, name: &'static str, attrs: &[(&'static str, &str)]) {
        if let Some(line) = self.render_event(name, attrs) {
            emit(&line);
        }
    }

    /// The record `event` would write, or `None` if the detail level silences
    /// it. Split out so the shape and the gating are testable without
    /// capturing stderr.
    fn render_event(&self, name: &'static str, attrs: &[(&'static str, &str)]) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        let mut line = format!(
            "event={} trace={} parent={}",
            encode(name),
            self.trace.id,
            self.id
        );
        for (k, v) in attrs {
            let _ = write!(line, " {k}={}", encode(v));
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
            encode(self.name),
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

/// Percent-encode anything that could be mistaken for record structure.
///
/// Quoting was the obvious first answer and it does not work: a quoted
/// value containing a space cannot survive a whitespace split, and the
/// halves parse back as *two* fields, so `phase="a b=c"` fabricates an
/// attribute `b=c` that nobody wrote. Blacklisting bytes does not work
/// either - the first version triggered on space, `=` and `"` only, so a
/// value holding a newline silently became two journal entries, and a tab
/// or U+00A0 defeated `split_whitespace` while looking innocent in a
/// terminal.
///
/// Encoding inverts the rule: everything is escaped unless it is known to
/// be inert. A value can then never contain whitespace, `=`, a control
/// character or a non-ASCII byte, which is what makes "split on
/// whitespace, then on the first `=`, then percent-decode" true as
/// written rather than true most of the time.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        // Inert set chosen so the common attributes stay readable without a
        // decoder: versions, phase names, connectors, paths, durations.
        if b.is_ascii_alphanumeric()
            || matches!(b, b'.' | b'_' | b'-' | b':' | b'/' | b'+' | b'@' | b',')
        {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Records longer than this are truncated and flagged.
///
/// A single `write` of at most `PIPE_BUF` (4096) is atomic, which is what
/// keeps a record from interleaving with a foreign writer on fd 2. The cap
/// sits below that with room for the ` trunc=1` marker and a margin, and
/// well below journald's default `LineMax` of 48K.
const MAX_RECORD: usize = 2048;

/// Assemble the bytes of one record, newline included.
///
/// Split out from [`emit`] because the two properties that matter - one
/// physical line, one buffer - are otherwise only observable under strace.
fn frame(line: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len() + 1);
    if line.len() > MAX_RECORD {
        // Every byte of a record is ASCII (names are literals, values are
        // percent-encoded), so a byte index is always a char boundary.
        out.extend_from_slice(&line.as_bytes()[..MAX_RECORD]);
        out.extend_from_slice(b" trunc=1");
    } else {
        out.extend_from_slice(line.as_bytes());
    }
    out.push(b'\n');
    out
}

fn emit(line: &str) {
    // stderr, because journald already stamps it with a monotonic timestamp
    // and the unit, and because a locker must not depend on a socket being
    // there.
    //
    // One `write_all` of one buffer, not `writeln!`. Rust's stderr is
    // unbuffered, so `writeln!(err, "{line}")` is two syscalls - the record,
    // then the newline - and `stderr().lock()` only excludes other *Rust*
    // writers. A C-side writer on the same fd (a PAM module, Mesa,
    // libwayland) takes no such lock and can land between them, appending
    // its bytes inside the record's final attribute. A reviewer measured
    // 12.8% of records spliced that way under load.
    //
    // A failed write is dropped. Note this does not make tracing
    // unconditionally safe: see the module docs on blocking.
    let record = frame(line);
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(&record);
}

/// Keys and event names must be literals, so attacker-influenced text
/// cannot reach a position that is written unescaped.
///
/// ```compile_fail
/// let t = span_lines::Trace::with_id("4bf92f3577b34da6a3ce929d0e0e4736", span_lines::Detail::Session);
/// let s = t.span("lock.session");
/// let untrusted = String::from("pam.msg\nevent=auth.success");
/// s.event(&untrusted, &[]);
/// ```
///
/// ```compile_fail
/// let t = span_lines::Trace::with_id("4bf92f3577b34da6a3ce929d0e0e4736", span_lines::Detail::Session);
/// let untrusted = String::from("k outcome");
/// let _ = t.span("lock.session").attr(&untrusted, "Unlocked");
/// ```
///
/// A literal key with an untrusted *value* is the supported shape:
///
/// ```
/// let t = span_lines::Trace::with_id("4bf92f3577b34da6a3ce929d0e0e4736", span_lines::Detail::Session);
/// let untrusted = String::from("whatever the compositor said");
/// let _ = t.span("lock.session").attr("output", untrusted);
/// ```
fn _keys_and_names_are_literals() {}

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

    /// The reader the record format promises: split on whitespace, split
    /// each token on its first `=`, percent-decode. Deliberately naive -
    /// its whole job is to be the dumbest thing a consumer might write.
    fn naive_read(line: &str) -> Vec<(String, String)> {
        fn decode(s: &str) -> String {
            let mut out = Vec::new();
            let b = s.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'%' && i + 3 <= b.len() {
                    if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                        out.push(byte);
                        i += 3;
                        continue;
                    }
                }
                out.push(b[i]);
                i += 1;
            }
            String::from_utf8_lossy(&out).into_owned()
        }
        line.split_whitespace()
            .filter_map(|tok| tok.split_once('='))
            .map(|(k, v)| (k.to_string(), decode(v)))
            .collect()
    }

    #[test]
    fn a_record_is_one_buffer_and_one_line() {
        let t = Trace::with_id("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session);
        let line = t
            .span("lock.session")
            .attr("phase", "PreLock")
            .render_span()
            .unwrap();
        let bytes = frame(&line);
        assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
        assert_eq!(*bytes.last().unwrap(), b'\n', "newline must terminate");
        assert_eq!(
            &bytes[..bytes.len() - 1],
            line.as_bytes(),
            "payload must be intact"
        );
        assert!(
            bytes.is_ascii(),
            "a non-ASCII byte would break the truncation index"
        );
    }

    #[test]
    fn an_oversized_record_is_capped_and_says_so() {
        // Without a cap one unbounded attribute pushes a record past
        // PIPE_BUF, at which point the kernel may split it and a foreign
        // writer on fd 2 can land in the gap.
        let t = Trace::with_id("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session);
        let mut span = t.span("lock.session");
        span.set("blob", "x".repeat(8000));
        let bytes = frame(&span.render_span().unwrap());
        assert!(
            bytes.len() <= MAX_RECORD + b" trunc=1\n".len(),
            "cap not applied: {}",
            bytes.len()
        );
        assert!(
            bytes.ends_with(b" trunc=1\n"),
            "truncation must be visible to a reader"
        );
        assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
    }

    #[test]
    fn structural_bytes_are_encoded_away() {
        assert_eq!(encode("PreLock"), "PreLock");
        assert_eq!(encode("0.3.3"), "0.3.3");
        assert_eq!(encode("DP-1"), "DP-1");
        assert_eq!(encode(""), "");
        assert_eq!(encode("two words"), "two%20words");
        assert_eq!(encode("a=b"), "a%3Db");
        assert_eq!(encode("a\nb"), "a%0Ab");
        assert_eq!(encode("a\tb"), "a%09b");
        assert_eq!(encode("a\rb"), "a%0Db");
        assert_eq!(encode("a\"b"), "a%22b");
        // U+00A0 is whitespace to `split_whitespace` but not to a byte
        // filter looking for b' '. That gap is what encoding closes.
        assert_eq!(encode("a\u{a0}b"), "a%C2%A0b");
        assert_eq!(encode("a\u{2028}b"), "a%E2%80%A8b");
        // The escape character must escape itself or decoding is ambiguous.
        assert_eq!(encode("100%"), "100%25");
    }

    #[test]
    fn a_hostile_value_cannot_forge_or_split_a_record() {
        // Every one of these defeated the previous quoting scheme: the
        // newline minted a second journal entry, the space-and-equals pair
        // fabricated an attribute, and the tab broke the split silently.
        for hostile in [
            "boom\nspan=forged trace=0 id=0 parent=- dur_ms=1 outcome=Unlocked",
            "a b=c",
            "two words",
            "x\ty",
            "a\u{a0}outcome=Unlocked",
            "\"quoted\"",
            "100% =",
        ] {
            let t = Trace::with_id("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session);
            let line = t
                .span("lock.session")
                .attr("note", hostile)
                .attr("after", "sentinel")
                .render_span()
                .unwrap();

            assert_eq!(line.lines().count(), 1, "record split into two: {line:?}");
            let fields = naive_read(&line);
            let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
            assert_eq!(
                keys,
                ["span", "trace", "id", "parent", "dur_ms", "note", "after"],
                "unexpected fields from {hostile:?}: {line}"
            );
            assert_eq!(
                fields.iter().find(|(k, _)| k == "note").unwrap().1,
                hostile,
                "value did not round-trip"
            );
            assert_eq!(
                fields.iter().find(|(k, _)| k == "after").unwrap().1,
                "sentinel"
            );
        }
    }
}
