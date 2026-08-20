# hypr-slint-runtime

Shared event-driven idle mechanics for long-lived Slint applications.

The crate pins Slint exactly to `1.17.1`. It provides a coalescing wake handle,
per-output dirty tracking, frame/timer/indefinite scheduling, redraw adapter
glue, and idle metrics. Presentation remains application-owned.

```rust
use hypr_slint_runtime::{DirtySet, IdleScheduler, Metrics, WakeHandle};
use std::sync::Arc;

let dirty = Arc::new(DirtySet::<u32>::new());
let metrics = Arc::new(Metrics::new());
let wake = WakeHandle::new(|| event_loop_ping());
let redraw = wake.redraw_handle(dirty.clone(), metrics.clone(), 7);

// WindowAdapter::request_redraw delegates to this. It performs no rendering.
redraw.request_redraw();

// After the event loop wakes:
let outputs = dirty.take_all();
for output in outputs {
    metrics.record_buffer_acquire();
    render_and_commit(output);
    metrics.record_render();
    metrics.record_commit();
}

let decision = IdleScheduler::default().from_slint([&window]);
wait_for_event_or_deadline(decision);
# fn event_loop_ping() {}
# fn render_and_commit(_: u32) {}
# fn wait_for_event_or_deadline(_: hypr_slint_runtime::WaitDecision) {}
# let window: slint::Window = panic!();
```

Call `WakeHandle::acknowledge()` immediately after consuming the event-loop wake
edge and before draining application work. This prevents a concurrent request
from being coalesced into an edge that was already consumed. Buffer acquisition
belongs inside the dirty-output branch.
