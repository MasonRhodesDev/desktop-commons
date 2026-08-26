# ADR 0007: Probe dirtiness before acquiring a buffer

- Status: accepted
- Date: 2026-08-25

## Context

ADR 0002 was written from a measurement: vigil-lock burning ~59% of a core
while locked and idle, Hyprland ~49%. It installed `slint-idle-runtime` and
laid down a rule — "buffers are acquired only after an output has been
selected from the dirty set" — and the surface `slint-idle-runtime-v0`
repeats it: presenters "must acquire buffers only for outputs returned by
`DirtySet::take_all`".

The rule was then broken by a correct fix. `DirtySet` cannot see Slint's
redraw requests: `slint::Window` belongs to the inner `MinimalSoftwareWindow`,
so core calls `request_redraw` there and never on any wrapper a presenter
could observe. Gating presents on the dirty set therefore left real
animations and input-driven changes unpresented on metal, and vigil's
`d66a974` ("Slint dirtiness gates frames, not the DirtySet") replaced the
dirty-set sweep with an unconditional present of every configured surface —
noting in a comment that the DirtySet was now "advisory, not the render
gate".

That reintroduced the original symptom, at the same magnitude, for a
different reason (vigil#65): 61.6% of a core with **no frames presented at
all**. The mechanism was not cost-per-frame, which is what ADR 0002 recorded,
but a feedback loop:

1. Every loop iteration offers each output a present.
2. `present()` acquires a `wl_shm` buffer **before** asking whether anything
   changed, then drops it un-attached when nothing did.
3. Dropping it sends `wl_buffer.destroy`.
4. Hyprland answers each destroy with `wl_display.delete_id` within ~50-90 us.
5. That makes the Wayland fd readable, which defeats the timeout the loop
   just armed, so it iterates again — and acquires another buffer.

Measured with an isolated 60-line client doing only that churn: **24,333
iterations/s against Hyprland**, versus **one iteration per 60-second timeout
against wlroots**, which does not reply eagerly. Hyprland's own CPU went from
1.1% to 25.2% of a core from the churn alone, with nothing ever drawn — which
is why the compositor appeared to be a second, independent problem.

Two consequences of that asymmetry matter beyond this bug. First, buffer
acquisition is **protocol traffic**, not a local allocation, so "acquire it
and find out" is never a free way to test dirtiness. Second, the behaviour is
**compositor-dependent**: vigil's nested test suite runs under sway and
therefore could not observe this at any severity, and did not.

## Decision

A presenter must be able to ask "does this output owe a present?" **without a
buffer**, and must ask before acquiring one.

- The render backend exposes a target-free probe
  (`RenderBackend::scene_needs_present`). For the software path it performs
  the Slint partial-repaint into the persistent shadow — which needs no
  target — and reports whether pixels changed. A following render only copies
  out.
- The probe's default is conservative: a backend that cannot answer cheaply
  answers `true`. Over-presenting costs frames; skipping a present that was
  owed is a black screen.
- Forced presents are exempt and still acquire: an output that has not yet
  committed must receive a frame even over a quiescent scene (vigil#35/#37).
- `DirtySet` remains advisory. This ADR does not restore it as the gate —
  the reason it was demoted is real — it removes the need for it to be the
  gate, by making the true gate answerable without protocol traffic.

## Consequences

- A clean wake costs one shadow repaint and **zero protocol traffic**, so the
  loop reaches its armed timeout instead of being woken by its own churn.
- A dirty frame renders the scene twice: once for the probe, once inside the
  present. The second is a no-op partial-repaint over an already-drawn
  shadow. This is the price of the split and is bounded by the frame rate.
- Every presenter in the ecosystem inherits the obligation, not just the one
  that regressed: vigil's greeter runs its own present loop of the same
  shape, and any future Slint-on-Wayland surface must probe before acquiring.
- **Tests under wlroots cannot verify this.** A nested-sway suite will pass
  with the loop fully present. Verification must be either protocol-level
  (assert zero `wl_buffer` create/destroy pairs across a settled window) or
  run against a compositor that replies eagerly. Any claim that idle cost is
  fixed must name which of those it rests on.
- ADR 0002's rule is superseded in its mechanism, not its intent: the intent
  ("no work, no buffers, no wakes, when nothing changed") stands, and this is
  how it is now enforced.

## Rejected alternatives

- **Restore the DirtySet as the render gate.** This is what `d66a974`
  correctly abandoned; Slint's redraw requests are invisible to it, and the
  failure mode is unpresented frames on metal, which is worse than idle cost.
- **Predict dirtiness from a flag before rendering.** `needs_present` is set
  *during* the render, because Slint reports whether it drew. Reading it
  beforehand skips legitimate repaints — the same defect, reintroduced.
- **Retain and reuse one buffer per output** so no destroy is sent. Smaller,
  but it forfeits the slot pool's guarantee that a buffer being written is
  not still held by the compositor, and trades an idle-cost bug for a
  tearing/black-frame risk in the one area with the least margin.
- **Treat it as a compositor bug.** Hyprland's eager `delete_id` is
  permitted; a client that acquires and destroys a buffer per wake is the
  party doing something unreasonable.
