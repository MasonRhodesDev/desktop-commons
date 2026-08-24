# ADR 0004: Capture-free, interruptible lock warning

- Status: accepted (amended 2026-08-23: manual-lock transition, reveal,
  frost opacity lever — vigil #52; bounded wallpaper hold — vigil #56)
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

Idle policy is the only path that uses the cancelable warning. Manual lock
and before-sleep paths run a short **non-cancelable transition** (default
400 ms, each ramp clamped to 2 s) on the same overlay machinery before
requesting `ext-session-lock-v1`: input is ignored, output hotplug commits
immediately rather than cancelling, and the wallpaper never holds the commit.
`vigil-lock --immediate` restores the instant commit. A second locker joins
either pre-lock phase over an unprivileged runtime socket, requests immediate
commitment, and reports success only after compositor lock confirmation.

On unlock, after authorization, Vigil maps a pointer-transparent,
keyboard-inert **reveal** overlay per output while still locked, then sends
`unlock_and_destroy`, then fades wallpaper and frost out and exits. The wait
for the overlays to map and the fade itself are both bounded; the desktop is
never shown before `unlock_and_destroy`, and `unlock_and_destroy` is never
sent before authorization.

**Frost strength.** `ext-background-effect-v1` only toggles a blur region;
it has no strength parameter. Vigil therefore drives frost as a
whole-surface opacity: `hyprland-surface-v1` `set_opacity` (documented to
multiply "blur behind the surface in addition to the surface's content",
and applied in Hyprland's blur pass regardless of
`decoration:blur:ignore_opacity`), else `wp-alpha-modifier-v1` (portable;
ramps blur wherever a compositor ties blur to surface alpha), else a
per-pixel tint ramp over a constant blur. These are optional client-protocol
capability tiers, not external compositor configuration, which keeps the
rejected-alternatives reasoning below intact.

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
- Portable configuration cannot promise a blur radius. Blur *strength*
  animates only through the surface-opacity tiers above; the tint ramp is
  the portable guarantee.
- `--wait` returns ~400 ms later on manual/before-sleep locks; hypridle's
  sleep inhibitor covers it.
- Wallpaper assets must be ready before their fade begins; late assets extend
  the warning instead of producing a blank handoff — but only up to
  `lock.warning.wallpaper_hold_max_ms` (default 5 s, clamped to 30 s). Past
  that the lock commits with the scene as-is and journals that it did: an
  asset pipeline that never finishes must not leave the machine unlocked,
  and a lock over a plain background beats an unlocked screen. Setting the
  key to 0 restores the unbounded wait.
- Output topology changes before commitment cancel the warning rather than
  risk partial coverage; the non-cancelable transition commits instead, and
  the new output gets a lock surface like any hotplug-while-locked.
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
