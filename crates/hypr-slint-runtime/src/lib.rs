//! Event-driven idle mechanics for long-lived Slint applications.
//!
//! This crate deliberately does not own a Wayland, DRM, or layer-shell event
//! loop. It supplies the state and policy those loops need to sleep indefinitely
//! when a static scene has no deadline. `WindowAdapter::request_redraw()` should
//! delegate to [`RedrawHandle::request_redraw`].

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The exact Slint release shared consumers must use.
pub const SLINT_VERSION: &str = "1.17.1";

/// A wakeup decision made after events, timers, and rendering have been drained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitDecision {
    /// A visible animation needs another frame after this delay.
    Frame(Duration),
    /// No animation is active; wait for this real Slint timer deadline.
    Timer(Duration),
    /// No work or deadline exists. Block until an external event.
    Indefinite,
}

/// Pure idle policy. Event-loop integrations translate its result into their
/// native timer/event source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdleScheduler {
    frame_interval: Duration,
}

impl IdleScheduler {
    /// Build a scheduler for the display's active refresh interval.
    pub const fn new(frame_interval: Duration) -> Self {
        Self { frame_interval }
    }

    /// Decide from already-observed animation and timer state.
    pub fn decide(
        &self,
        has_visible_animation: bool,
        next_timer: Option<Duration>,
    ) -> WaitDecision {
        if has_visible_animation {
            WaitDecision::Frame(match next_timer {
                Some(timer) => timer.min(self.frame_interval),
                None => self.frame_interval,
            })
        } else if let Some(timer) = next_timer {
            WaitDecision::Timer(timer)
        } else {
            WaitDecision::Indefinite
        }
    }

    /// Query Slint after `update_timers_and_animations()` and rendering.
    ///
    /// Only visible windows should be supplied. Hidden windows with infinite
    /// animations must not keep the application frame clock alive.
    pub fn from_slint<'a>(
        &self,
        visible_windows: impl IntoIterator<Item = &'a slint::Window>,
    ) -> WaitDecision {
        let active = visible_windows
            .into_iter()
            .any(slint::Window::has_active_animations);
        self.decide(active, slint::platform::duration_until_next_timer_update())
    }
}

impl Default for IdleScheduler {
    fn default() -> Self {
        Self::new(Duration::from_nanos(16_666_667))
    }
}

struct WakeState {
    pending: AtomicBool,
    notify: Box<dyn Fn() + Send + Sync>,
}

/// Cloneable, thread-safe, edge-coalescing event-loop wake source.
///
/// The callback runs only on the transition from no pending wake to pending.
/// Event loops call [`acknowledge`](Self::acknowledge) immediately after
/// consuming the wake edge and before draining application events. This order
/// ensures a concurrent request creates a fresh edge instead of being lost.
#[derive(Clone)]
pub struct WakeHandle(Arc<WakeState>);

impl WakeHandle {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(WakeState {
            pending: AtomicBool::new(false),
            notify: Box::new(notify),
        }))
    }

    /// Request an event-loop wake. Returns true only for the coalesced edge.
    pub fn wake(&self) -> bool {
        if self
            .0
            .pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            (self.0.notify)();
            true
        } else {
            false
        }
    }

    /// Clear the pending edge immediately after consuming it, before draining
    /// application work. A request concurrent with the drain then wakes again.
    pub fn acknowledge(&self) -> bool {
        self.0.pending.swap(false, Ordering::AcqRel)
    }

    pub fn is_pending(&self) -> bool {
        self.0.pending.load(Ordering::Acquire)
    }

    /// Construct the glue an application's `WindowAdapter::request_redraw()`
    /// delegates to.
    pub fn redraw_handle<O>(
        &self,
        dirty: Arc<DirtySet<O>>,
        metrics: Arc<Metrics>,
        output: O,
    ) -> RedrawHandle<O>
    where
        O: Ord,
    {
        RedrawHandle {
            dirty,
            wake: self.clone(),
            metrics,
            output,
        }
    }
}

/// Precise, coalescing per-output redraw intent.
pub struct DirtySet<O>(Mutex<BTreeSet<O>>);

impl<O> DirtySet<O>
where
    O: Ord,
{
    pub fn new() -> Self {
        Self(Mutex::new(BTreeSet::new()))
    }

    /// Mark an output dirty. Returns true only when newly inserted.
    pub fn mark(&self, output: O) -> bool {
        self.0.lock().expect("dirty set poisoned").insert(output)
    }

    pub fn is_empty(&self) -> bool {
        self.0.lock().expect("dirty set poisoned").is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.lock().expect("dirty set poisoned").len()
    }

    /// Atomically drain all dirty outputs in deterministic order.
    pub fn take_all(&self) -> Vec<O> {
        std::mem::take(&mut *self.0.lock().expect("dirty set poisoned"))
            .into_iter()
            .collect()
    }
}

impl<O: Ord> Default for DirtySet<O> {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimal Slint adapter glue: dirties exactly one output, records the request,
/// and wakes the host loop. It never renders or acquires a buffer.
pub struct RedrawHandle<O> {
    dirty: Arc<DirtySet<O>>,
    wake: WakeHandle,
    metrics: Arc<Metrics>,
    output: O,
}

impl<O> RedrawHandle<O>
where
    O: Clone + Ord,
{
    pub fn request_redraw(&self) {
        self.metrics.record_redraw_request();
        self.dirty.mark(self.output.clone());
        self.wake.wake();
    }
}

/// Monotonic idle observability counters.
#[derive(Default)]
pub struct Metrics {
    wake_requests: AtomicU64,
    redraw_requests: AtomicU64,
    buffer_acquires: AtomicU64,
    renders: AtomicU64,
    commits: AtomicU64,
}

/// A consistent point-in-time copy of [`Metrics`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MetricsSnapshot {
    pub wake_requests: u64,
    pub redraw_requests: u64,
    pub buffer_acquires: u64,
    pub renders: u64,
    pub commits: u64,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_wake(&self) {
        self.wake_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_redraw_request(&self) {
        self.redraw_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_buffer_acquire(&self) {
        self.buffer_acquires.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_render(&self) {
        self.renders.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_commit(&self) {
        self.commits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            wake_requests: self.wake_requests.load(Ordering::Relaxed),
            redraw_requests: self.redraw_requests.load(Ordering::Relaxed),
            buffer_acquires: self.buffer_acquires.load(Ordering::Relaxed),
            renders: self.renders.load(Ordering::Relaxed),
            commits: self.commits.load(Ordering::Relaxed),
        }
    }
}

/// Deterministic elapsed-time source for event-loop tests.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FakeClock {
    now: Duration,
}

impl FakeClock {
    pub const fn new() -> Self {
        Self {
            now: Duration::ZERO,
        }
    }

    pub const fn now(&self) -> Duration {
        self.now
    }

    pub fn advance(&mut self, by: Duration) -> Duration {
        self.now = self.now.saturating_add(by);
        self.now
    }

    pub fn deadline_after(&self, delay: Duration) -> Duration {
        self.now.saturating_add(delay)
    }

    pub fn remaining_until(&self, deadline: Duration) -> Duration {
        deadline.saturating_sub(self.now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn static_idle_blocks_indefinitely_regardless_of_output_count() {
        let scheduler = IdleScheduler::default();
        for outputs in [0, 1, 3, 32] {
            let dirty = DirtySet::new();
            for output in 0..outputs {
                dirty.mark(output);
            }
            dirty.take_all();
            assert_eq!(scheduler.decide(false, None), WaitDecision::Indefinite);
            assert!(dirty.is_empty());
        }
    }

    #[test]
    fn timer_and_frame_decisions_use_real_deadlines() {
        let scheduler = IdleScheduler::new(Duration::from_millis(16));
        assert_eq!(
            scheduler.decide(false, Some(Duration::from_secs(60))),
            WaitDecision::Timer(Duration::from_secs(60))
        );
        assert_eq!(
            scheduler.decide(true, None),
            WaitDecision::Frame(Duration::from_millis(16))
        );
        assert_eq!(
            scheduler.decide(true, Some(Duration::from_millis(4))),
            WaitDecision::Frame(Duration::from_millis(4))
        );
    }

    #[test]
    fn wake_requests_coalesce_until_acknowledged() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let wake = WakeHandle::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        assert!(wake.wake());
        assert!(!wake.wake());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(wake.acknowledge());
        assert!(wake.wake());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn redraw_only_marks_its_output_and_wakes_once() {
        let dirty = Arc::new(DirtySet::new());
        let metrics = Arc::new(Metrics::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let wake = WakeHandle::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        let one = wake.redraw_handle(dirty.clone(), metrics.clone(), "one");
        let two = wake.redraw_handle(dirty.clone(), metrics.clone(), "two");
        one.request_redraw();
        one.request_redraw();
        two.request_redraw();
        assert_eq!(dirty.take_all(), vec!["one", "two"]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(metrics.snapshot().redraw_requests, 3);
        assert_eq!(metrics.snapshot().buffer_acquires, 0);
    }

    #[test]
    fn fake_clock_models_minute_boundary_without_polling() {
        let mut clock = FakeClock::new();
        let deadline = clock.deadline_after(Duration::from_secs(60));
        clock.advance(Duration::from_secs(59));
        assert_eq!(clock.remaining_until(deadline), Duration::from_secs(1));
        clock.advance(Duration::from_secs(1));
        assert_eq!(clock.remaining_until(deadline), Duration::ZERO);
    }

    #[test]
    fn metrics_distinguish_intent_from_expensive_work() {
        let metrics = Metrics::new();
        metrics.record_wake();
        metrics.record_redraw_request();
        metrics.record_buffer_acquire();
        metrics.record_render();
        metrics.record_commit();
        assert_eq!(
            metrics.snapshot(),
            MetricsSnapshot {
                wake_requests: 1,
                redraw_requests: 1,
                buffer_acquires: 1,
                renders: 1,
                commits: 1,
            }
        );
    }
}
