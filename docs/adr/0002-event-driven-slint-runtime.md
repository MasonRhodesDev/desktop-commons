# ADR 0002: Event-driven Slint runtime and frozen idle clocks

- Status: accepted
- Date: 2026-08-20

## Context

Several long-lived Hypr-DE applications use Slint on fullscreen or multi-output
surfaces. Vigil-lock was observed consuming about 59% of one CPU core while the
machine was locked and otherwise idle, with Hyprland consuming about 49%.
Unlocking immediately removed the load.

The failure was architectural rather than a single expensive frame. Custom
event loops woke every 16 ms, visited every output, and in the lock path
acquired fullscreen SHM buffers before determining whether Slint had anything
dirty to draw. Work therefore scaled with monitor count and kept the compositor
active for the entire locked interval.

Other applications had related fixed-rate state synchronization. Voice
Dictation used a permanent 16 ms timer even while hidden. Hyprstate GUI used
50 ms, 500 ms, and 1 second polling timers despite running on Slint's native
event loop.

Slint already exposes the required scheduling signals:
`WindowAdapter::request_redraw()`, `Window::has_active_animations()`,
`platform::duration_until_next_timer_update()`, and
`platform::update_timers_and_animations()`.

## Decision

Long-lived Slint applications in the suite use event/deadline-driven rendering.

When no visible window is dirty, no visible animation is active, and no timer
is due, the UI thread blocks indefinitely. There is no dormant fallback frame
rate. Idle work must remain zero as output count increases.

A static visible surface retains its last committed buffer. “Visible” does not
mean “continuously rendering.” Security surfaces such as a lock screen must
commit an initial covering buffer, then may sleep while the compositor or
display controller continues scanning it out.

For custom platform/event-loop integrations:

- `request_redraw()` marks exactly the affected output dirty and wakes the
  loop; it does not render or acquire a buffer.
- External producers wake through a coalescing, thread-safe edge.
- Buffers are acquired only after an output has been selected from the dirty
  set.
- Visible animations use a vblank or bounded frame deadline.
- Real Slint timers use their next reported deadline.
- Input, authentication, output topology, VT/resume, IPC, configuration, and
  asset-ready events defrost the loop.

The shared scheduling primitives live in `hypr-slint-runtime`, canonically
maintained in `hypr-commons` and shipped through its public mirror. The crate
owns wake coalescing, per-output dirtiness, wait decisions, metrics, and test
clocks. Applications retain ownership of Wayland, DRM/GBM, layer-shell,
authentication, and presentation details.

Applications using Slint's native winit backend do not add the shared runtime
merely for consistency. They retain the native scheduler and remove repeated
application timers in favor of event-loop wakes and bounded one-shot deadlines.

All suite consumers align on exact Slint `1.17.1` until a deliberate suite-wide
upgrade is validated. Released Git dependencies must be commit-locked, allowed
explicitly by supply-chain policy, and included in offline RPM/Arch vendor
metadata.

The normative implementation and verification contract is
[`SLINT_IDLE_RUNTIME.md`](../SLINT_IDLE_RUNTIME.md).

## Consequences

- Hidden and static applications can remain resident for days without a render
  clock or per-monitor CPU tax.
- Defrost latency is bounded by event delivery and one display refresh, not a
  polling interval.
- Infinite animations, blinking cursors, repeated timers, and retry loops must
  be explicitly bounded or disabled while dormant.
- Presenters require precise dirty tracking and cannot use buffer acquisition as
  a dirtiness probe.
- Cross-thread producers must signal after publishing state and wake requests
  must safely coalesce without losing a concurrent edge.
- Tests must measure wake requests, buffer acquisitions, renders, and commits;
  a successful visual frame alone is insufficient.
- Packaging must preserve non-registry dependency mappings for offline builds.

## Rejected alternatives

**Reduce the fixed cadence.** A 100 ms or one-second poll still consumes power,
adds latency, and multiplies by output count. It does not establish a correct
idle state.

**Put all presentation backends in the commons crate.** Wayland session lock,
direct DRM/GBM, and layer-shell have different safety and lifecycle contracts.
Sharing those implementations would enlarge the failure domain without being
necessary to share scheduling policy.

**Require every Slint application to depend on the shared runtime.** Native
winit applications already implement event/deadline waiting correctly. Their
problem is repeated application timers, so adding a second scheduler would be
redundant and potentially conflicting.

**Install local builds while publication catches up.** Production verification
must exercise released package artifacts and their offline dependency closure.
Local builds may validate source behavior but are not the installed end state.

