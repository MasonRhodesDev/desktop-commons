//! OpenTelemetry-shaped spans and events, written as journald lines.
//!
//! The OTel *data model* is what makes ordering answerable: a span has a
//! parent, a start and a duration, so "what happened inside what" is a
//! property of the data rather than something reconstructed from the order
//! lines happen to appear in. The SDK, an OTLP exporter and a collector are
//! all downstream of that model and none of them are here: records go to
//! stderr, journald stamps `__MONOTONIC_TIMESTAMP`, `_PID` and `_BOOT_ID`
//! on them for free, and converting the result to OTLP is a
//! text-processing job for whoever wants one.
//!
//! Do not expect `_SYSTEMD_UNIT` to identify the producer. journald
//! attributes an entry by the *writer's* cgroup, not by the unit that owns
//! the stream, so a process started in a transient scope from a service -
//! which is how a locker is launched - lands with no user unit at all.
//! Measured on systemd 261: a `systemd-run --user --scope` child writing to
//! a service's inherited stderr produced entries with
//! `_SYSTEMD_USER_UNIT` unset. Filter by `_COMM` instead of `-u`.
//!
//! That matters because the first consumer is a screen locker, whose
//! `deny.toml` opens with "supply-chain gate for the binary that IS the
//! login screen". This crate depends on `std` and nothing else, opens no
//! sockets, and spawns no threads.
//!
//! # Shape
//!
//! ```text
//! span=lock.session trace=<32hex> id=<16hex> parent=-       seq=0 t_us=0      dur_us=2413918 outcome=Unlocked
//! span=flow.phase   trace=<32hex> id=<16hex> parent=<16hex> seq=1 t_us=1204   dur_us=560310  phase=PreLock
//! event=flow.transition trace=<32hex> parent=<16hex>        seq=2 t_us=561514 from=Committing to=Locked
//! ```
//!
//! Read it by splitting on whitespace, then each token on its first `=`,
//! then percent-decoding the value. That rule is exact, not approximate:
//! keys and names are `&'static str` and values are encoded, so no field
//! can contain whitespace or `=`.
//!
//! A span emits when it is dropped, so it cannot be left unended and its
//! duration cannot be forgotten - but `Drop` does not run on
//! [`std::process::exit`], so call [`Span::end`] before any path that does
//! not unwind, or [`exit`] instead of `std::process::exit`.
//!
//! # Ordering
//!
//! A span is the half-open interval `[t_us, t_us + dur_us)` and an event is
//! the point `t_us`, both measured in microseconds from this process's
//! first use of the crate. So "did the transition land inside that frame"
//! is an intersection test, and a transition that *overlaps* a frame is
//! distinguishable from one that merely follows it - which record order
//! alone cannot tell you, because spans emit at their end. `seq` totally
//! orders this process's records when two land in the same microsecond.
//!
//! Across processes, use journald's `__MONOTONIC_TIMESTAMP`
//! (`journalctl -o short-precise`, or `-o json`); `t_us` is per-process and
//! is not comparable between them.
//!
//! # Where to emit from
//!
//! Emit from the adapter, not from a pure controller. A pure crate that
//! forbids host effects cannot construct a [`Span`] - this crate writes to
//! stderr - so a state machine should report transitions through whatever
//! seam it already has for journaling, and the adapter executing that seam
//! opens and closes the spans:
//!
//! ```ignore
//! // in the executor, not in the controller
//! match cmd {
//!     FlowCmd::Journal(note) => self.phase.event("flow.transition", &[("note", &note.to_string())]),
//!     // ...
//! }
//! ```
//!
//! # Volume
//!
//! [`Detail::Session`] is the default and is quiet: a handful of records
//! per lock. [`Detail::Frames`] is a diagnostic mode, not something to
//! leave on - one span per frame per output, plus one per wake.
//!
//! An earlier version of this section warned that `frames` would exceed
//! journald's rate limit and have records dropped. That appears to be
//! wrong for this transport: `RateLimitBurst` governs the syslog and native
//! protocols, and a burst of 15,000 lines through a unit's stderr was
//! measured arriving complete, with no suppression, on systemd 261 at
//! stock configuration. Do not rely on the rate limiter to bound this.
//!
//! The cost that is real is the one under **Blocking** below: nothing
//! throttles a producer except the reader, so `frames` raises the rate at
//! which a stalled journald can park the traced thread. Measure stalls, not
//! drops.
//!
//! # Propagation
//!
//! [`Trace::from_env`] adopts a W3C `TRACEPARENT` if the parent process set
//! one, so a lock that begins in a shell script and ends in a locker is one
//! trace. [`Span::traceparent`] produces the value to hand to a child.
//! Trace flags are carried verbatim but do not gate local emission: see
//! [`Trace`].
//!
//! # Security
//!
//! The first consumer is the login screen, so two rules are not optional.
//!
//! **Nothing secret becomes an attribute.** There is no redaction here and
//! no way to mark a value sensitive. journald is readable by the user and
//! commonly by `adm`/`wheel`, and a locker has passwords, PAM prompts,
//! usernames and monitor serials within one line of an `attr` call. Note
//! that a duration is a disclosure too: a `dur_us` on an authentication
//! span is a keystroke-timing side channel.
//!
//! **Attacker-influenced text goes in a value, never a key or a name.**
//! Keys and event names are written without encoding, so they are
//! `&'static str` and the unsafe call does not compile. Values are
//! percent-encoded and cannot forge a field or split a record.
//!
//! `SPAN_LINES` is read from the environment, which the user owns. `off`
//! is therefore a convenience, not a security control - a durable
//! `environment.d` line can silence these records, and anything that needs
//! a guaranteed audit trail must not rely on them. Prefer setting the
//! variable per unit rather than exporting it into a shell, since it is a
//! single global level with no per-target scoping: an exported
//! `SPAN_LINES=frames` turns up every instrumented tool run from that
//! shell.
//!
//! # Blocking
//!
//! A failed write is dropped, but a *blocked* one is not: `emit` writes to
//! stderr synchronously, and if the reader stalls - journald stopped, or a
//! wrapper piping stderr with nobody draining it - the calling thread parks
//! in `write(2)`. For a locker that means the UI stops repainting while the
//! screen stays locked. The exposure is bounded by volume, which is why
//! `session` is the default and `frames` is diagnostic, and it is the same
//! mechanism any `eprintln!` in the consumer already has. It is not fixed
//! here; a bounded non-blocking emitter is tracked separately.

#[cfg(feature = "tracing")]
pub mod tracing_layer;

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// The W3C `sampled` flag, used for traces this process starts.
const SAMPLED: &str = "01";

/// The instant every `t_us` is measured from: the first time this process
/// touches the crate.
///
/// Process-global rather than per-trace on purpose. A trace can span
/// processes, so a per-trace origin would not be comparable across them
/// anyway - that is journald's `__MONOTONIC_TIMESTAMP`'s job. What `t_us`
/// buys is ordering *within* one process at a resolution the journal's
/// default output does not show.
fn origin() -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(Instant::now)
}

/// The trace this process's records belong to when no caller supplies one
/// per record - notably every record the tracing layer writes, since
/// `tracing` has no place to carry a trace id.
///
/// Adopts `TRACEPARENT` exactly as [`Trace::from_env`] does, so a lock that
/// starts in a shell wrapper is still one trace whichever API writes it.
pub fn process_trace() -> &'static Trace {
    static TRACE: OnceLock<Trace> = OnceLock::new();
    TRACE.get_or_init(Trace::read_env)
}

/// A total order over the records this process emits, breaking ties when
/// two land in the same microsecond.
fn next_seq() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

fn micros_since_origin(at: Instant) -> u128 {
    at.saturating_duration_since(origin()).as_micros()
}

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

/// A per-process random base for ids, read once.
///
/// Once, not per span: `Span::new` used to open, read and close
/// `/dev/urandom` for every span it constructed - including spans the
/// detail level would silence, so `SPAN_LINES=off` cost the same three
/// syscalls per span as full tracing, and a frame span cost them every
/// frame.
fn seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        // Ids need to be unique, not unguessable: they correlate lines in
        // one user's journal.
        let mut raw = [0_u8; 8];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            if std::io::Read::read_exact(&mut f, &mut raw).is_ok() {
                return u64::from_le_bytes(raw);
            }
        }
        // Fallback for a process that cannot open /dev/urandom. The pid is
        // mixed into the low bits as well as the high ones, because the
        // previous version shifted it to bits 64..96 where an 8-byte span
        // id never reached it - two processes forking in the same
        // nanosecond minted identical span ids.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let pid = u64::from(std::process::id());
        nanos ^ pid.rotate_left(32) ^ pid
    })
}

/// Counter feeding [`id_from`]; distinct from `next_seq` so record order
/// and id derivation cannot be inferred from one another.
fn next_id_counter() -> u64 {
    static IDS: AtomicU64 = AtomicU64::new(0);
    IDS.fetch_add(1, Ordering::Relaxed)
}

/// SplitMix64. A counter alone would make ids sequential and visibly
/// correlated; this scatters them for the cost of a few instructions.
fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// `bytes * 2` lowercase hex characters, never all zero.
///
/// Pure, so the properties below can be asserted against adversarial
/// inputs rather than against whatever the machine happened to produce.
/// All-zero matters because W3C reads an all-zero id as "no parent": the
/// old clock fallback could mint one, `traceparent()` would hand it to a
/// child, and the child's own parser would reject it and silently start a
/// new trace - breaking exactly the continuity the crate advertises.
fn id_from(seed: u64, counter: u64, bytes: usize) -> String {
    let mut out = String::with_capacity(bytes * 2);
    let mut acc = seed ^ splitmix(counter);
    for round in 0..bytes.div_ceil(8) {
        acc = splitmix(acc ^ (round as u64).wrapping_add(1));
        let _ = write!(out, "{acc:016x}");
    }
    out.truncate(bytes * 2);
    nonzero(out)
}

/// Force an id away from all-zero.
///
/// Split out because it is unreachable from a real seed - splitmix is a
/// bijection, so exactly one input per width lands there - and an
/// unreachable guard that no test exercises is a guard nobody knows is
/// broken. The fallback seed makes it reachable in practice: a stopped
/// clock and an unlucky pid arrive here.
fn nonzero(mut id: String) -> String {
    // `all` is vacuously true for an empty string, so without the emptiness
    // check a zero-width id came back one character long - a width contract
    // broken by the very guard meant to protect the value.
    if !id.is_empty() && id.bytes().all(|b| b == b'0') {
        id.pop();
        id.push('1');
    }
    id
}

fn hex(bytes: usize) -> String {
    id_from(seed(), next_id_counter(), bytes)
}

/// A trace: an id shared by every span in one causal chain, and the detail
/// level those spans honour.
#[derive(Clone, Debug)]
pub struct Trace {
    id: String,
    detail: Detail,
    /// Span id inherited from a parent process, if any.
    inherited_parent: Option<String>,
    /// W3C trace flags, propagated verbatim to children.
    ///
    /// They are *carried*, not obeyed: whether this process records is
    /// governed by [`Detail`], because these are local journal records
    /// rather than sampled telemetry, and a parent's sampling decision has
    /// no bearing on whether a locker's own journal should be readable.
    /// Re-flagging a child as sampled when the parent said `00` would be a
    /// lie to whoever collects downstream, so the value is preserved.
    flags: String,
}

impl Trace {
    /// Adopt `TRACEPARENT` if the parent process set a well-formed one,
    /// otherwise begin a new trace.
    ///
    /// A malformed value is ignored rather than rejected: a broken header
    /// from some other tool must not stop a session tracing itself.
    ///
    /// One trace per process: repeated calls return the same id, and it is
    /// the id the `tracing` bridge uses too. Minting a fresh one per call
    /// would mean a consumer mixing the two APIs wrote records a reader
    /// cannot join - and would do so only when no `TRACEPARENT` was set,
    /// which is the normal case for a locker started by a compositor
    /// rather than a shell wrapper. Use [`Trace::adopt`] for a deliberately
    /// separate trace.
    pub fn from_env() -> Self {
        process_trace().clone()
    }

    /// The uncached construction behind [`Trace::from_env`].
    fn read_env() -> Self {
        let detail = Detail::from_env();
        if let Ok(tp) = std::env::var("TRACEPARENT") {
            if let Some((trace, parent, flags)) = parse_traceparent(&tp) {
                return Self {
                    id: trace,
                    detail,
                    inherited_parent: Some(parent),
                    flags,
                };
            }
        }
        Self {
            id: hex(16),
            detail,
            inherited_parent: None,
            flags: SAMPLED.to_string(),
        }
    }

    /// Adopt a trace id this caller already carries, rather than minting
    /// one.
    ///
    /// `None` if the id is not 32 lowercase-able hex digits, or is all
    /// zero. Validated rather than trusted for two reasons: the id is
    /// written into every `trace=` field, where an unchecked string is the
    /// same record-forgery primitive that keys and names were; and this
    /// crate is published, so callers outside this workspace will exist.
    ///
    /// Note what validation cannot catch. Passing a *fixed* id from real
    /// code is well-formed and still wrong - every session collapses onto
    /// one trace and correlation stops meaning anything. This is for a
    /// caller propagating an id it received, not for inventing one.
    pub fn adopt(id: &str, detail: Detail) -> Option<Self> {
        let id = id.trim();
        if id.len() != 32
            || !id.bytes().all(|b| b.is_ascii_hexdigit())
            || id.bytes().all(|b| b == b'0')
        {
            return None;
        }
        Some(Self {
            id: id.to_ascii_lowercase(),
            detail,
            inherited_parent: None,
            flags: SAMPLED.to_string(),
        })
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

/// Parse a W3C `traceparent`, returning `(trace id, parent span id, flags)`.
///
/// Leniency here is deliberate and bounded. Uppercase hex is accepted and
/// lowercased, and surrounding whitespace is trimmed, because a sloppy
/// producer upstream should not cost a session its trace. What is *not*
/// accepted is anything the spec calls invalid, because honouring those
/// means adopting a parent that a conforming reader would not:
///
/// - version `ff` is reserved as invalid, and is the one value a producer
///   uses to say "this header is garbage";
/// - version `00` is exactly four fields, so trailing data means the
///   header was not written to the version it claims (later versions may
///   append, and those are still accepted);
/// - flags must be two hex digits, not merely present.
fn parse_traceparent(value: &str) -> Option<(String, String, String)> {
    let mut parts = value.trim().split('-');
    let version = parts.next()?;
    let trace = parts.next()?;
    let span = parts.next()?;
    let flags = parts.next()?;
    let trailing = parts.next().is_some();

    if version.len() != 2 || trace.len() != 32 || span.len() != 16 || flags.len() != 2 {
        return None;
    }
    let hexy = |s: &str| s.bytes().all(|b| b.is_ascii_hexdigit());
    if !hexy(version) || !hexy(trace) || !hexy(span) || !hexy(flags) {
        return None;
    }
    if version.eq_ignore_ascii_case("ff") {
        return None;
    }
    if version == "00" && trailing {
        return None;
    }
    // An all-zero id is "no parent" per the spec, not a parent named zero.
    if trace.bytes().all(|b| b == b'0') || span.bytes().all(|b| b == b'0') {
        return None;
    }
    Some((
        trace.to_ascii_lowercase(),
        span.to_ascii_lowercase(),
        flags.to_ascii_lowercase(),
    ))
}

/// An open span. Emits its record when dropped.
pub struct Span {
    trace: Trace,
    name: &'static str,
    id: String,
    parent: Option<String>,
    started: Instant,
    /// Microseconds from [`origin`] to this span's start. Emitted as
    /// `t_us`, which is what makes a span an interval rather than a point.
    start_us: u128,
    attrs: String,
    /// The detail level at which this span is worth emitting.
    needs: Detail,
    emitted: bool,
}

impl Span {
    fn new(trace: Trace, name: &'static str, parent: Option<String>, needs: Detail) -> Self {
        // One clock read, not two. `dur_us` is derived from `started` while
        // `t_us` is published from `start_us`, so reading the clock twice
        // made the published interval end later than the measured one by
        // the gap between the calls.
        let started = Instant::now();
        Self {
            trace,
            name,
            id: hex(8),
            parent,
            started,
            start_us: micros_since_origin(started),
            attrs: String::new(),
            needs,
            emitted: false,
        }
    }

    /// A child span.
    ///
    /// Emitted at `Detail::Session`, or at whatever its parent requires if
    /// that is louder. A child cannot be chattier than its parent: a
    /// session-level child of a silent frame span would print
    /// `parent=<id>` naming a span that appears nowhere in the journal, and
    /// a reader reconstructing the tree would hit a dangling edge - which
    /// defeats the one property the record format exists to provide.
    pub fn child(&self, name: &'static str) -> Span {
        Span::new(
            self.trace.clone(),
            name,
            Some(self.id.clone()),
            self.needs.max(Detail::Session),
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
        // The key is encoded too. `&'static str` stops *attacker* text
        // reaching this position, which is the security property, but it
        // does not stop a careless literal: `attr("bad key", "v")` compiled
        // happily and emitted `bad key=v`, which a reader parses as a field
        // named `bad` that is silently dropped plus a field `key=v`. Same
        // corruption class, self-inflicted. Encoding is free here because a
        // well-chosen key is already inert.
        let _ = write!(self.attrs, " {}={}", encode(key), encode(&rendered));
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
            "event={} trace={} parent={} seq={} t_us={}",
            encode(name),
            self.trace.id,
            self.id,
            next_seq(),
            micros_since_origin(Instant::now()),
        );
        for (k, v) in attrs {
            let _ = write!(line, " {}={}", encode(k), encode(v));
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
            "span={} trace={} id={} parent={} seq={} t_us={} dur_us={}{}",
            encode(self.name),
            self.trace.id,
            self.id,
            self.parent.as_deref().unwrap_or("-"),
            next_seq(),
            self.start_us,
            self.started.elapsed().as_micros(),
            self.attrs,
        ))
    }

    /// Emit this span now and consume it.
    ///
    /// Required before any path that does not unwind. `Drop` is the normal
    /// ending and covers ordinary returns, but `std::process::exit` runs no
    /// destructors, and the first consumer ends *every* terminal path that
    /// way - vigil-lock calls it at twelve sites, one per lock outcome. A
    /// root `lock.session` span left to `Drop` there would emit nothing on
    /// exactly the outcomes worth recording.
    pub fn end(mut self) {
        // Same status rule as Drop. A caller can reach end() from a
        // wrapping type's own Drop while that thread is unwinding, and a
        // span ending in a panic should say so however it was ended.
        self.finish(drop_status(std::thread::panicking()));
    }

    /// Emit once. Returns the record written, for tests; `None` if the span
    /// was already ended or the detail level silences it.
    fn finish(&mut self, status: Option<&'static str>) -> Option<String> {
        if self.emitted {
            return None;
        }
        self.emitted = true;
        if let Some(status) = status {
            self.push_attr("status", status);
        }
        let line = self.render_span()?;
        emit(&line);
        Some(line)
    }

    fn enabled(&self) -> bool {
        // `needs` is only ever Session or Frames, both above Off, so the
        // comparison already excludes Off; testing it separately was dead.
        self.trace.detail >= self.needs
    }

    /// The W3C header to hand to a child process, so its spans join this
    /// trace.
    pub fn traceparent(&self) -> String {
        format!("00-{}-{}-{}", self.trace.id, self.id, self.trace.flags)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn trace(&self) -> &Trace {
        &self.trace
    }
}

/// What a span's outcome field should say given how it is ending.
///
/// A span dropped during unwinding used to be byte-identical to one that
/// completed, so a crashed lock and a clean lock read the same.
pub(crate) fn drop_status(panicking: bool) -> Option<&'static str> {
    if panicking {
        Some("panic")
    } else {
        None
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        self.finish(drop_status(std::thread::panicking()));
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
/// One `write` per record narrows the window in which a foreign writer on
/// fd 2 can interleave, and keeping a record small keeps it narrow. Note
/// what is *not* claimed: POSIX guarantees an atomic write only up to
/// `PIPE_BUF` (4096) and only for pipes and FIFOs, while journald's stdout
/// stream is an `AF_UNIX` `SOCK_STREAM` socket, which carries no such
/// guarantee. So this is a large reduction in the splice window rather
/// than a proof there is none. The cap sits below `PIPE_BUF` with room for
/// the ` trunc=1` marker, and well below journald's default `LineMax` of
/// 48K.
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

// Records emitted on this thread, for tests only.
//
// Tests used to assert against `render_span()` directly, which does not
// mark the span emitted - so the span then emitted a *second* time when it
// dropped, with a different `seq` and a longer `dur_us`. The suite was
// violating the two invariants this crate had just introduced. Capturing
// what `emit` actually wrote lets tests exercise the real path (Drop or
// end(), through finish()) instead of a stand-in for it.
//
// Thread-local so tests running in parallel cannot see each other's
// records.
#[cfg(test)]
thread_local! {
    static EMITTED: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Where records go.
///
/// The default writes to stderr for journald to stamp, which is what every
/// consumer in this suite wants. It is an interface rather than a hardcoded
/// destination so a consumer with a different destination - a test
/// collecting records, a tool writing to a file, a harness asserting on
/// them - can supply one without the crate growing options for each case.
///
/// Implementations must not panic and must not block indefinitely: a sink
/// is called from `Drop`, so a panicking one panics during unwinding and a
/// blocking one stalls the thread being traced.
pub trait Sink: Send + Sync + 'static {
    /// Write one complete record. The bytes end in a single newline and are
    /// entirely ASCII.
    fn write(&self, record: &[u8]);
}

/// The default sink: one `write_all` to stderr.
pub struct StderrSink;

impl Sink for StderrSink {
    fn write(&self, record: &[u8]) {
        // One `write_all` of one buffer, not `writeln!`. Rust's stderr is
        // unbuffered, so `writeln!(err, "{line}")` is two syscalls - the
        // record, then the newline - and `stderr().lock()` only excludes
        // other *Rust* writers. A C-side writer on the same fd (a PAM
        // module, Mesa, libwayland) takes no such lock and can land between
        // them, appending its bytes inside the record's final attribute. A
        // reviewer measured 12.8% of records spliced that way under load.
        // See MAX_RECORD for what one write does and does not guarantee.
        //
        // A failed write is dropped. Note this does not make tracing
        // unconditionally safe: see the module docs on blocking.
        let mut err = std::io::stderr().lock();
        let _ = err.write_all(record);
    }
}

static SINK: OnceLock<Box<dyn Sink>> = OnceLock::new();

/// Install the process-wide sink. First call wins.
///
/// Returns `false` if a sink was already in place, so a caller can tell "I
/// configured this" from "records are going somewhere else". Note the
/// default counts: the first record emitted installs [`StderrSink`]
/// lazily, so calling this after anything has been traced also returns
/// `false`. Install it before opening the first span.
#[must_use = "a losing set_sink means records are going somewhere else"]
pub fn set_sink(sink: impl Sink) -> bool {
    SINK.set(Box::new(sink)).is_ok()
}

fn sink() -> &'static dyn Sink {
    SINK.get_or_init(|| Box::new(StderrSink)).as_ref()
}

/// Something to run before the process exits.
///
/// The registry exists because `std::process::exit` runs no destructors, so
/// a span waiting on `Drop` is lost on exactly the paths worth recording.
/// It is a list rather than a single hook so anything else that needs to
/// finish writing - a second layer, a consumer's own buffered state - can
/// join without this crate knowing about it.
pub trait AtExit: Send + Sync + 'static {
    /// Finish outstanding work. Called once, on the exiting thread.
    fn flush(&self);
}

static AT_EXIT: Mutex<Vec<std::sync::Arc<dyn AtExit>>> = Mutex::new(Vec::new());

/// Register something to flush before [`exit`].
///
/// There is no way to unregister. The registry is process-global and grows
/// with each call, which is fine for the intended one-per-process
/// installation and worth knowing if something registers per iteration.
pub fn at_exit(hook: impl AtExit) {
    if let Ok(mut hooks) = AT_EXIT.lock() {
        hooks.push(std::sync::Arc::new(hook));
    }
}

/// Run every registered hook, most recently registered first.
///
/// Separate from [`exit`] so a consumer that ends some other way - a signal
/// handler setting a flag, a test - can flush without terminating. Calling
/// it repeatedly is safe and required to be: a hook must be idempotent,
/// because the whole point is that `exit` can still flush after someone
/// else already did.
pub fn flush() {
    // Clone the handles out rather than running under the lock: a hook that
    // registers another one would otherwise deadlock. Cloning rather than
    // *taking* them is what keeps a later `exit` able to flush - draining
    // here meant an independent `flush()` silently disarmed the exit path,
    // losing exactly the spans the registry exists to save.
    let hooks = match AT_EXIT.lock() {
        Ok(hooks) => hooks.clone(),
        Err(_) => return,
    };
    for hook in hooks.iter().rev() {
        hook.flush();
    }
}

/// [`flush`], then `std::process::exit`.
///
/// Use this anywhere a traced program would call `std::process::exit`.
/// Destructors do not run there, so a span left to `Drop` is lost on
/// precisely the outcomes worth recording - and because nothing is emitted,
/// the journal cannot distinguish "never started" from "still running" from
/// "died". Being a named function also makes the rule greppable: a bare
/// `std::process::exit` in an instrumented binary is a bug you can search
/// for.
///
/// This rescues what the registered hooks know about. The `tracing` layer
/// registers one, so its open spans are closed and marked `status=exit`. A
/// [`Span`] from this crate's own API is not reachable from here - end it
/// with [`Span::end`] before calling this.
pub fn exit(code: i32) -> ! {
    flush();
    std::process::exit(code)
}

/// Take everything emitted on this thread since the last call. Tests only.
#[cfg(test)]
pub(crate) fn test_drain() -> Vec<String> {
    EMITTED.with(|recorded| std::mem::take(&mut *recorded.borrow_mut()))
}

fn emit(line: &str) {
    #[cfg(test)]
    EMITTED.with(|recorded| recorded.borrow_mut().push(line.to_string()));

    // The destination is the installed Sink - stderr by default, because
    // journald stamps it and a locker must not depend on a socket being
    // there.
    sink().write(&frame(line));
}

/// Keys and event names must be literals, so attacker-influenced text
/// cannot reach a position that is written unescaped.
///
/// ```compile_fail
/// let t = span_lines::Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", span_lines::Detail::Session).unwrap();
/// let s = t.span("lock.session");
/// let untrusted = String::from("pam.msg\nevent=auth.success");
/// s.event(&untrusted, &[]);
/// ```
///
/// ```compile_fail
/// let t = span_lines::Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", span_lines::Detail::Session).unwrap();
/// let untrusted = String::from("k outcome");
/// let _ = t.span("lock.session").attr(&untrusted, "Unlocked");
/// ```
///
/// A literal key with an untrusted *value* is the supported shape:
///
/// ```
/// let t = span_lines::Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", span_lines::Detail::Session).unwrap();
/// let untrusted = String::from("whatever the compositor said");
/// let _ = t.span("lock.session").attr("output", untrusted);
/// ```
fn _keys_and_names_are_literals() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_w3c_shaped() {
        let t = Trace::adopt(&hex(16), Detail::Session).unwrap();
        assert_eq!(t.id().len(), 32, "trace id must be 32 hex chars");
        let s = t.span("x");
        assert_eq!(s.id().len(), 16, "span id must be 16 hex chars");
        assert!(t.id().bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(s.id().bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn a_malformed_trace_id_is_refused_rather_than_written_into_a_record() {
        // The id lands unencoded in every `trace=` field, so an unchecked
        // string is the same forgery primitive keys and names were.
        for bad in [
            "",
            "zz zz\nspan=whatever",
            "4bf92f3577b34da6a3ce929d0e0e473",   // 31
            "4bf92f3577b34da6a3ce929d0e0e47366", // 33
            "4bf92f3577b34da6a3ce929d0e0e473g",
            "00000000000000000000000000000000",
        ] {
            assert!(
                Trace::adopt(bad, Detail::Session).is_none(),
                "adopted {bad:?}"
            );
        }
        // Uppercase is normalised, not refused.
        let t = Trace::adopt("4BF92F3577B34DA6A3CE929D0E0E4736", Detail::Session).unwrap();
        assert_eq!(t.id(), "4bf92f3577b34da6a3ce929d0e0e4736");
    }

    #[test]
    fn a_traceparent_the_spec_calls_invalid_is_not_adopted() {
        // Leniency is a choice about sloppy producers, not about producers
        // saying "this is garbage". These must all be refused.
        let t = "4bf92f3577b34da6a3ce929d0e0e4736";
        let p = "00f067aa0ba902b7";
        for bad in [
            // ff is reserved as invalid; it is how a producer signals a
            // header that must not be trusted.
            &format!("ff-{t}-{p}-01"),
            &format!("FF-{t}-{p}-01"),
            // Version 00 is exactly four fields.
            &format!("00-{t}-{p}-01-extra"),
            // Flags must be two hex digits, not merely present.
            &format!("00-{t}-{p}-zz"),
            &format!("00-{t}-{p}-1"),
            &format!("00-{t}-{p}-"),
            &format!("00-{t}-{p}-0100"),
        ] {
            assert!(parse_traceparent(bad).is_none(), "adopted {bad:?}");
        }

        // A future version may append fields, and the spec says to accept
        // it and ignore what we do not understand.
        let (trace, parent, flags) =
            parse_traceparent(&format!("01-{t}-{p}-01-cc")).expect("future version must parse");
        assert_eq!(
            (trace.as_str(), parent.as_str(), flags.as_str()),
            (t, p, "01")
        );
    }

    #[test]
    fn an_unsampled_parent_stays_unsampled_for_its_children() {
        // Flags are carried, not obeyed. Re-flagging a child as sampled
        // when the parent said 00 lies to whoever collects downstream.
        let t = "4bf92f3577b34da6a3ce929d0e0e4736";
        let (_, _, flags) = parse_traceparent(&format!("00-{t}-00f067aa0ba902b7-00")).unwrap();
        assert_eq!(flags, "00");

        let adopted = Trace {
            id: t.to_string(),
            detail: Detail::Session,
            inherited_parent: Some("00f067aa0ba902b7".into()),
            flags,
        };
        let span = adopted.span("lock.session");
        assert!(
            span.traceparent().ends_with("-00"),
            "child header must preserve the parent's flags: {}",
            span.traceparent()
        );
        // ... and a trace we start ourselves is sampled.
        let mine = Trace::adopt(t, Detail::Session).unwrap();
        assert!(mine.span("lock.session").traceparent().ends_with("-01"));
    }

    #[test]
    fn traceparent_round_trips() {
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
        let s = t.span("root");
        let tp = s.traceparent();
        let (trace, parent, flags) = parse_traceparent(&tp).expect("our own header must parse");
        assert_eq!(flags, "01");
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

    /// Whether a span would write anything, without rendering it.
    ///
    /// Rendering to find out consumed a `seq` for a record that was never
    /// emitted, leaving gaps in the counter the tests then asserted on.
    fn emits(span: &Span) -> bool {
        span.enabled()
    }

    /// Take everything emitted on this thread since the last call.
    fn drain() -> Vec<String> {
        super::test_drain()
    }

    /// Emit an event through the real path and return the record it wrote.
    fn event_of(span: &Span, name: &'static str, attrs: &[(&'static str, &str)]) -> String {
        drain();
        span.event(name, attrs);
        let mut written = drain();
        assert_eq!(written.len(), 1, "expected exactly one event: {written:?}");
        written.pop().unwrap()
    }

    /// End a span through the real path and return the one record it wrote.
    ///
    /// Asserts exactly one record, which is the guard against a span
    /// emitting twice - the failure the old `render_span()`-in-tests habit
    /// produced silently.
    fn record(span: Span) -> String {
        drain();
        drop(span);
        let mut written = drain();
        assert_eq!(written.len(), 1, "expected exactly one record: {written:?}");
        written.pop().unwrap()
    }

    /// Assert a span writes nothing at all, through the real path.
    fn silent(span: Span) {
        drain();
        drop(span);
        assert_eq!(
            drain(),
            Vec::<String>::new(),
            "span was expected to be silent"
        );
    }

    /// Strip the two random ids so a record's *shape* can be asserted
    /// exactly. Their format is covered by `ids_are_w3c_shaped`.
    fn skeleton(line: &str) -> String {
        line.split_whitespace()
            .map(|f| match f.split_once('=') {
                Some((k @ ("id" | "parent"), v)) if v != "-" => format!("{k}=<id>"),
                // seq and the clock fields are inherently variable; the
                // tests that care about them read them numerically.
                Some((k @ ("seq" | "t_us" | "dur_us"), _)) => format!("{k}=<n>"),
                _ => f.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn a_root_span_renders_the_documented_shape() {
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
        let line = record(t.span("lock.session").attr("outcome", "Unlocked"));
        assert_eq!(
            skeleton(&line),
            "span=lock.session trace=4bf92f3577b34da6a3ce929d0e0e4736 id=<id> parent=- seq=<n> t_us=<n> dur_us=<n> outcome=Unlocked"
        );
    }

    #[test]
    fn a_child_span_names_its_parent_and_shares_the_trace() {
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
        let root = t.span("lock.session");
        let child = root.child("flow.phase").attr("phase", "PreLock");
        let child_id = child.id().to_string();
        assert_ne!(child_id, root.id(), "ids must be distinct");

        drain();
        child.event(
            "flow.transition",
            &[("from", "Committing"), ("to", "Locked")],
        );
        let event = drain().pop().expect("event must be emitted");
        assert_eq!(
            skeleton(&event),
            "event=flow.transition trace=4bf92f3577b34da6a3ce929d0e0e4736 parent=<id> seq=<n> t_us=<n> from=Committing to=Locked"
        );

        let line = record(child);
        assert!(
            line.contains(&format!("parent={}", root.id())),
            "child must point at its parent; got {line}"
        );
        assert!(line.contains("trace=4bf92f3577b34da6a3ce929d0e0e4736"));
    }

    #[test]
    fn off_silences_every_record() {
        // The whole point of `off` is that a locker can be told to say
        // nothing at all; a leaking span would defeat it.
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Off).unwrap();
        let root = t.span("lock.session");
        assert!(!emits(&root));
        silent(root.frame_child("frame.present"));
        silent(root);
    }

    #[test]
    fn frame_spans_are_silent_until_frames_is_asked_for() {
        // A settled session must be silent: per-frame records are for
        // diagnosis, and emitting them by default would make idle noisy.
        let quiet = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
        let root = quiet.span("lock.session");
        assert!(emits(&root), "session spans still emit");
        let frame = root.frame_child("frame.present");
        assert!(!emits(&frame), "frame span leaked at session detail");
        assert!(!emits(&frame), "an event inherits its span's gate");

        let loud = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Frames).unwrap();
        let root = loud.span("lock.session");
        assert!(emits(&root), "frames detail keeps session spans");
        assert!(emits(&root.frame_child("frame.present")));
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

    fn field(line: &str, key: &str) -> u128 {
        naive_read(line)
            .into_iter()
            .find(|(k, _)| k == key)
            .unwrap_or_else(|| panic!("no {key} in {line}"))
            .1
            .parse()
            .unwrap()
    }

    /// `[start, end)` in microseconds, read the way a consumer would.
    fn interval(line: &str) -> (u128, u128) {
        let t = field(line, "t_us");
        (t, t + field(line, "dur_us"))
    }

    #[test]
    fn an_id_is_never_all_zero() {
        // W3C reads an all-zero id as "no parent", so minting one would
        // make traceparent() hand a child a header the child rejects.
        for (seed, counter) in [(0, 0), (u64::MAX, u64::MAX), (0, 1), (1, 0)] {
            for bytes in [8, 16] {
                let id = id_from(seed, counter, bytes);
                assert_eq!(id.len(), bytes * 2);
                assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
                assert!(
                    id.bytes().any(|b| b != b'0'),
                    "all-zero id from seed={seed} counter={counter} bytes={bytes}"
                );
            }
        }
        // The guard itself, reached directly.
        assert_eq!(nonzero("0000000000000000".into()), "0000000000000001");
        assert_eq!(
            nonzero("00000000000000000000000000000000".into()),
            "00000000000000000000000000000001"
        );
        assert_eq!(nonzero("00000000000000ab".into()), "00000000000000ab");
        assert_eq!(
            nonzero(String::new()),
            "",
            "a zero-width id must stay empty"
        );

        assert!(
            parse_traceparent(&format!("00-{}-{}-01", id_from(0, 0, 16), id_from(0, 0, 8)))
                .is_some(),
            "our own minted ids must survive our own parser"
        );
    }

    #[test]
    fn an_id_is_exactly_the_width_asked_for() {
        // Only 8 and 16 are used today. The others are here because
        // `id_from` is advertised as pure and adversarially testable, and
        // a width that is not a multiple of eight is what distinguishes
        // rounding up from truncating down.
        for bytes in [0, 1, 4, 8, 12, 16, 20] {
            let id = id_from(0xfeed, 7, bytes);
            assert_eq!(id.len(), bytes * 2, "wrong width for {bytes} bytes: {id:?}");
            assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn the_seed_reaches_a_span_id() {
        // The old fallback put the pid at bits 64..96 and then emitted the
        // low 8 bytes, so an 8-byte span id was identical across processes
        // that started in the same nanosecond.
        assert_ne!(
            id_from(1, 0, 8),
            id_from(2, 0, 8),
            "seed must reach a span id"
        );
        assert_ne!(id_from(1, 0, 16), id_from(2, 0, 16));
    }

    #[test]
    fn ids_do_not_repeat_or_run_in_sequence() {
        let ids: Vec<String> = (0..2048).map(|c| id_from(0xfeed, c, 8)).collect();
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "id collision within one process");
        // Avalanche. Consecutive counters must not yield ids differing in
        // only a few bits; any additive or lightly-mixed scheme fails this,
        // while "differs by exactly one" does not catch them.
        let mean: f64 = ids
            .windows(2)
            .map(|w| {
                let a = u64::from_str_radix(&w[0], 16).unwrap();
                let b = u64::from_str_radix(&w[1], 16).unwrap();
                f64::from((a ^ b).count_ones())
            })
            .sum::<f64>()
            / (ids.len() - 1) as f64;
        assert!(
            (28.0..36.0).contains(&mean),
            "consecutive ids differ in {mean:.1} of 64 bits; expected about half"
        );
    }

    #[test]
    fn no_record_names_a_parent_that_was_never_printed() {
        // At session detail a frame span is silent. A child of it must be
        // silent too, or it emits `parent=<id>` pointing at nothing.
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
        let root = t.span("lock.session");
        let frame = root.frame_child("frame.present");
        assert!(!emits(&frame), "frame span must be silent here");

        let orphan = frame.child("frame.subtask");
        assert!(
            !emits(&orphan),
            "a child of a silent span must not emit: it would name an absent parent"
        );
        assert!(!emits(&orphan));

        // Turn the parent on and the child comes back with it.
        let loud = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Frames).unwrap();
        let root = loud.span("lock.session");
        let frame = root.frame_child("frame.present");
        assert!(emits(&frame));
        assert!(emits(&frame.child("frame.subtask")));

        // And an ordinary child of an ordinary span is unaffected.
        assert!(emits(&root.child("flow.phase")));
    }

    #[test]
    fn a_span_can_be_stored_and_shared() {
        // `Cell<bool>` made Span !Sync, so a session span held in shared
        // state would not compile. The guard it provided is a plain bool.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Span>();
        assert_send_sync::<Trace>();
    }

    #[test]
    fn flush_runs_hooks_in_reverse_and_stays_armed() {
        // Reverse, because a hook registered later may depend on one
        // registered earlier still being usable. Still armed afterwards,
        // because draining meant a consumer's own flush() - a signal
        // handler, a test - silently disarmed the exit path, so the next
        // `exit` lost every open span. That is the failure the registry
        // exists to prevent, reintroduced by the registry.
        #[derive(Clone)]
        struct Note(&'static str, std::sync::Arc<Mutex<Vec<&'static str>>>);
        impl AtExit for Note {
            fn flush(&self) {
                self.1.lock().unwrap().push(self.0);
            }
        }
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        at_exit(Note("first", seen.clone()));
        at_exit(Note("second", seen.clone()));
        flush();
        assert_eq!(*seen.lock().unwrap(), ["second", "first"]);

        // A second flush runs them again: hooks are idempotent, and the
        // alternative is an exit path that quietly stopped working.
        flush();
        assert_eq!(
            *seen.lock().unwrap(),
            ["second", "first", "second", "first"],
            "flush must stay armed for a later exit()"
        );
    }

    #[test]
    fn a_hook_registered_during_flush_does_not_deadlock() {
        // Holding the registry lock across the hooks would deadlock here,
        // which is the sort of thing that only ever happens while the
        // process is trying to exit.
        struct Reentrant;
        impl AtExit for Reentrant {
            fn flush(&self) {
                struct Inner;
                impl AtExit for Inner {
                    fn flush(&self) {}
                }
                at_exit(Inner);
            }
        }
        at_exit(Reentrant);
        flush();
        // Reaching here at all is the assertion.
        flush();
    }

    #[test]
    fn a_span_emits_exactly_once_however_it_ends() {
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();

        drain();
        t.span("lock.session").end();
        assert_eq!(drain().len(), 1, "end() then Drop must write one record");

        drain();
        drop(t.span("lock.session"));
        assert_eq!(drain().len(), 1, "Drop alone must write one record");

        drain();
        {
            let mut span = t.span("lock.session");
            span.finish(None);
            span.finish(None);
        }
        assert_eq!(drain().len(), 1, "repeated finish must write one record");
    }

    #[test]
    fn end_emits_once_and_drop_does_not_repeat_it() {
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
        let mut span = t.span("lock.session");
        let first = span.finish(None).expect("first end must emit");
        assert!(first.starts_with("span=lock.session"));
        assert!(
            span.finish(None).is_none(),
            "a span must not emit twice; Drop runs after end()"
        );
    }

    #[test]
    fn a_span_unwound_by_a_panic_says_so() {
        assert_eq!(drop_status(true), Some("panic"));
        assert_eq!(drop_status(false), None);

        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
        let mut span = t.span("lock.session").attr("outcome", "Unlocked");
        let line = span.finish(drop_status(true)).unwrap();
        assert!(
            line.contains(" status=panic"),
            "a panicking span must be distinguishable: {line}"
        );

        // end() must reach the same decision as Drop, not a hardcoded None:
        // a wrapping type's Drop can call end() while the thread unwinds.
        // end() must reach the same decision as Drop, not a hardcoded
        // None: a wrapping type's Drop can call end() while unwinding.
        drain();
        let panicked = std::panic::catch_unwind(|| {
            struct EndsOnDrop(Option<Span>);
            impl Drop for EndsOnDrop {
                fn drop(&mut self) {
                    if let Some(span) = self.0.take() {
                        span.end();
                    }
                }
            }
            let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
            let _guard = EndsOnDrop(Some(t.span("lock.session")));
            std::panic::panic_any("boom");
        });
        assert!(panicked.is_err(), "the test's own panic must have fired");
        let written = drain();
        assert_eq!(
            written.len(),
            1,
            "the guard's span must emit once: {written:?}"
        );
        assert!(
            written[0].contains(" status=panic"),
            "end() during unwinding must record the panic: {}",
            written[0]
        );
    }

    #[test]
    fn a_span_is_an_interval_so_overlap_is_visible() {
        // This is the whole point of the crate. Spans emit on Drop, so
        // record order is *end* order; without a start time a reader cannot
        // tell "the transition happened during this frame" from "the
        // transition happened after it", which is precisely the question
        // being asked of vigil-lock.
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Frames).unwrap();
        let root = t.span("lock.session");

        // Overlapping: the transition fires while the frame is in flight.
        let frame = root.frame_child("frame.present").attr("output", "DP-1");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let event = event_of(
            &root,
            "flow.transition",
            &[("from", "Committing"), ("to", "Locked")],
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
        let frame = record(frame);

        let (start, end) = interval(&frame);
        let at = field(&event, "t_us");
        assert!(
            start < at && at < end,
            "transition at {at} should fall inside frame [{start}, {end})"
        );

        // Sequential: the frame finishes before the transition fires.
        let done = record(root.frame_child("frame.present"));
        std::thread::sleep(std::time::Duration::from_millis(2));
        let after = event_of(&root, "flow.transition", &[]);
        let (_, end) = interval(&done);
        assert!(
            end < field(&after, "t_us"),
            "a transition after the frame must read as after it"
        );
    }

    #[test]
    fn a_span_reads_the_clock_once() {
        // Two Instant::now() calls put a scheduler-jitter gap between the
        // duration's origin and the published t_us, so `t_us + dur_us`
        // overstated the real endpoint on every span. A reviewer measured
        // 1.8% of consecutive pairs straddling a microsecond and a worst
        // case of 32 us - small, but this crate exists to decide sub-
        // millisecond overlap, so it is exactly the wrong place to be
        // sloppy.
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
        let before = micros_since_origin(Instant::now());
        let mut span = t.span("lock.session");
        let after = micros_since_origin(Instant::now());
        let line = span.finish(None).unwrap();

        let start = field(&line, "t_us");
        assert!(
            (before..=after).contains(&start),
            "t_us {start} outside [{before}, {after}]"
        );
        // The published interval must not claim to end after the moment we
        // stopped measuring it.
        let end = start + field(&line, "dur_us");
        assert!(
            end <= micros_since_origin(Instant::now()),
            "interval ends in the future: {line}"
        );
    }

    #[test]
    fn seq_totally_orders_records_within_a_process() {
        // t_us can tie at microsecond resolution; seq is what keeps the
        // order total when it does.
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
        let root = t.span("lock.session");
        // Interleaved on purpose: a span record and an event record draw
        // from the same counter, or the order is only total within a kind.
        let mut seqs = Vec::new();
        for _ in 0..4 {
            seqs.push(field(&event_of(&root, "flow.transition", &[]), "seq"));
            seqs.push(field(&record(root.child("flow.phase")), "seq"));
        }
        assert!(
            seqs.windows(2).all(|w| w[0] < w[1]),
            "seq must strictly increase across spans and events alike: {seqs:?}"
        );
    }

    #[test]
    fn frame_work_is_not_reported_as_zero() {
        // dur_ms rounded every frame to 0: real frame work is well under a
        // millisecond against a 16 ms budget, so the field recorded a
        // constant and paid syscalls to do it.
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Frames).unwrap();
        let root = t.span("lock.session");
        let span = root.frame_child("frame.present");
        std::thread::sleep(std::time::Duration::from_micros(400));
        let line = record(span);
        assert!(
            field(&line, "dur_us") >= 300,
            "sub-millisecond work must be measurable: {line}"
        );
    }

    #[test]
    fn a_record_is_one_buffer_and_one_line() {
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
        let line = record(t.span("lock.session").attr("phase", "PreLock"));
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
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
        let mut span = t.span("lock.session");
        span.set("blob", "x".repeat(8000));
        let bytes = frame(&record(span));
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
    fn a_careless_literal_key_cannot_corrupt_a_record() {
        // `&'static str` keeps attacker text out of key position, which is
        // the security property. It does not make a literal well-formed:
        // `attr("bad key", "v")` compiles. Before keys were encoded it
        // emitted `bad key=v`, which a reader parses as a field `bad` that
        // is silently dropped plus a field `key=v`.
        let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
        let line = record(
            t.span("lock.session")
                .attr("bad key", "v")
                .attr("also=bad", "w"),
        );
        let fields = naive_read(&line);
        let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "span",
                "trace",
                "id",
                "parent",
                "seq",
                "t_us",
                "dur_us",
                "bad%20key",
                "also%3Dbad"
            ],
            "a malformed literal key must not add or drop fields: {line}"
        );
        assert_eq!(
            fields.iter().find(|(k, _)| k == "bad%20key").unwrap().1,
            "v"
        );
        assert_eq!(
            fields.iter().find(|(k, _)| k == "also%3Dbad").unwrap().1,
            "w"
        );

        // Events take their attributes by a different path, so they need
        // their own coverage: the first version of this test asserted only
        // on `attr`, and a mutant leaving event keys raw survived it.
        let root = t.span("lock.session");
        let event = event_of(
            &root,
            "flow.transition",
            &[("bad key", "v"), ("also=bad", "w")],
        );
        let keys: Vec<String> = naive_read(&event).into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            keys,
            [
                "event",
                "trace",
                "parent",
                "seq",
                "t_us",
                "bad%20key",
                "also%3Dbad"
            ],
            "an event's keys must be encoded too: {event}"
        );
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
            let t = Trace::adopt("4bf92f3577b34da6a3ce929d0e0e4736", Detail::Session).unwrap();
            let line = record(
                t.span("lock.session")
                    .attr("note", hostile)
                    .attr("after", "sentinel"),
            );

            assert_eq!(line.lines().count(), 1, "record split into two: {line:?}");
            let fields = naive_read(&line);
            let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
            assert_eq!(
                keys,
                ["span", "trace", "id", "parent", "seq", "t_us", "dur_us", "note", "after"],
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
