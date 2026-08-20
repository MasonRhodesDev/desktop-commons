# ADR 0004: Capture-free, interruptible lock warning

- Status: accepted
- Date: 2026-08-20

## Context

An idle-triggered lock should warn the user without abruptly replacing their
desktop. The desired visual is live frosted glass followed by the lock
wallpaper, while any user activity before commitment cancels the lock.

Capturing a desktop image would create privacy, permission, synchronization,
and multi-output failure modes. Hyprland-specific renderer hooks or layer rules
would make Vigil's client behavior compositor-specific. A client-side blur
would still require obtaining the pixels it is forbidden to capture.

## Decision

Vigil uses the staging `ext-background-effect-v1` Wayland protocol on one
transparent ARGB layer-shell surface per output. It requests a full-surface
blur region and lets the compositor blur the live content behind that surface.
Vigil animates only its own tint and wallpaper opacity; the protocol
intentionally leaves blur algorithm and strength to compositor policy.

The warning state is pre-lock and cancelable. Key, button, configured pointer
motion, or output hotplug tears down every warning surface and exits 3. At the
opaque-wallpaper boundary Vigil requests `ext-session-lock-v1`; warning
surfaces remain mapped until every lock surface has committed, preventing a
desktop reveal during handoff. Authentication starts only after the compositor
confirms the lock.

If background effects or blur capability are unavailable, the same state
machine runs with tint only. Vigil does not bind a capture protocol and does
not contain a production CPU/GPU desktop-blur path. Its safe windowed simulator
may blur its generated fake desktop solely to preview the compositor-owned
effect.

Idle policy is the only path that uses the warning. Manual lock and
before-sleep paths commit immediately. A second locker joins the warning over
an unprivileged runtime socket, requests immediate commitment, and reports
success only after compositor lock confirmation.

Callers that must establish readiness use `vigil-lock --wait`. Vigil detaches
the long-lived locker and returns zero only after compositor confirmation;
warning cancellation returns 3 and pre-confirmation failure remains nonzero.
This is the boundary used by idle and before-sleep policy.

Development and automation use the separately linked `vigil-sim`, which has
no PAM, logind, greetd, DRM, input-device, power, or session-lock dependencies.
Its control socket acknowledges `lock --wait` and frame exports only after the
corresponding frame is presented. Headless scenario fixtures run the real
theme/UI renderer with an injected clock and emit deterministic state, trace,
and frame fingerprints without affecting the host session.

## Consequences

- Live desktop changes remain visible beneath frost without a captured frame.
- Unsupported compositors retain a functional, cancelable tint transition.
- Portable configuration cannot promise a blur radius or animate blur
  strength.
- Wallpaper assets must be ready before their fade begins; late assets extend
  the warning instead of producing a blank handoff.
- Output topology changes before commitment cancel rather than risking partial
  coverage.
- The simulator and production backend deliberately use different blur
  implementations; the dependency boundary prevents simulator code from
  entering the lock binary.
- Readiness means compositor-confirmed lock, never merely process startup or a
  request to commit.

## Rejected alternatives

**Screencopy or portal capture.** This exposes desktop pixels to the locker,
adds permission and stale-frame behavior, and violates the privacy boundary.

**Client-side production blur.** A client cannot blur pixels it does not own
without first capturing them.

**Hyprland blur layer rules.** They are compositor-specific external policy and
cannot provide a portable client contract or capability fallback.

**Acquire session-lock before warning.** Once locked, input can no longer
cancel back to the live session; it changes the product from an idle warning
into a lock-screen animation.
