# Design principles

The constitution for the tool suite. Each principle names its current
exemplar and known violations — a violation isn't a sin, it's a queued
migration. Grounded in SURVEY-2026-06-12.md.

## 1. Spec first for anything stateful; adversarial review before code

Daemons with nontrivial state machines get a written spec that survives as
the behavioral contract (not a design diary). Run it through an adversarial
review pass (multi-lens panel) before implementing; fold verdicts back in.
The spec is what made the hyprstate Python→Rust port mechanically verifiable.

- Exemplars: hyprstate (POWER_SPEC/GPU_SPEC), couchcord (SPEC + panel docs).
- Violations: hyprnotice, voice-dictation (grew organically; retrofit only if
  a rewrite/major feature forces it).

## 2. Pure core, effector boundary

Decision logic is pure functions over plain inputs — no I/O, no clocks
(`Instant` passed in), no globals. Side effects live in a thin, idempotent
effector layer. Tests target the pure core; effects get mocked or seam-tested.
couchcord (cc-core, 56 tests) and hyprstate (pure/, 52 tests) converged on
this independently; every tool without the split has zero tests. Testability
is an architecture property, not a discipline property.

## 3. One event loop, one state owner

A single dispatcher task owns mutable state; sources send typed events over
channels. No `Arc<Mutex<State>>` webs (voice-dictation's pain point). The one
sanctioned exception is a `watch` channel for a value an in-flight operation
must observe (hyprstate's `locked`).

## 4. Daemons own their drift

Subscriptions lie by omission: signals get missed, sockets die, compositors
restart. Every long-running daemon needs all three legs:
- **Reconnect with rediscovery** — don't retry a dead endpoint forever;
  re-resolve it (hyprstate's socket2 instance rescan).
- **Periodic reconcile** against ground truth that *drives the state
  machine*, not just logs (hyprstate's ReconcileTick; its v1 bug — repairs
  that didn't transition — is the cautionary tale).
- **Liveness signal** where the tool matters: systemd watchdog
  (voice-dictation) or Type=dbus/notify so failure is visible.

## 5. Policy in user space; mechanism in narrow privileged processes; fail closed

Root components do mechanism only, behind the narrowest possible interface
(hyprstate powerd: 3 methods, hardcoded sysfs whitelist, hardened unit;
greetd_game_mode: separated users, exact-match sudoers, fail-closed approval
gate). Privileged code paths must be package-owned — root never executes
user-writable files (the lesson that forced hyprstate's libexec dance until
the RPM made it obsolete).

## 6. Native interfaces over subprocess parsing; isolate what can't be native

Speak the protocol (zbus proxies, unix sockets, sysfs reads) instead of
shelling to `hyprctl | jq | grep`. Where no protocol exists (hypridle's
inhibitor count), isolate the hack in one module with an explicit health
signal so format drift is loud (hyprstate's ParseHealth), never silent.

- Worst offender: waybar-workspace-buttons (7+ popen per update, sscanf JSON).

## 7. Effects are idempotent and read-before-write

Every effector checks current state before mutating (hyprstate set_edp/knob
writes; lmtt's managed `>>> <<<` blocks in foreign configs). Corollaries:
debounce bursty inputs; self-written changes must be recognized when they
echo back (hyprstate's self-write tracker / poller-echo no-ops); a tool never
revert-fights an external change — adopt it as intent.

## 8. Standard toolchain per language

- **Rust** (default for daemons/CLIs): tokio (current_thread unless proven
  otherwise), zbus 5, clap 4 derive, tracing + tracing-subscriber,
  anyhow at edges / thiserror or plain enums in cores, Cargo.lock committed.
- **C++** (only for hypr-ecosystem UI work where hyprtoolkit is the point):
  C++23, hyprutils/hyprlang/hyprtoolkit, sdbus-c++, CMake.
- Shell/Python: installers, hardware-locked one-offs (greetd-config,
  desk-controller). Not for daemons. The hyprstate rewrite is the precedent:
  long-lived state machines end up in Rust.

## 9. Config: TOML at ~/.config/<tool>/; restart-to-reconfigure is fine

TOML parsed once with serde defaults. Hot reload only when the tool's UX
demands it (voice-dictation's dictionary), via file watcher — not SIGHUP.
Tools with enough knobs ship a schema-tui schema instead of growing a custom
config UI. Directive comments (`#@ key = value`) only for embedding metadata
in *foreign-format* files (hyprstate profiles in Hyprland .conf), with the
key charset pinned per dialect.

## 10. Packaging: one tag → dnf + pacman; no new bespoke installers

The hyprstate pattern is the target for every distributable tool: `dist/`
(units, dbus policy under /usr/share, udev rules, presets) + `packaging/`
(spec with vendored cargo deps for COPR, PKGBUILD with the same payload,
build-srpm.sh). Presets auto-enable units on first install. Migration/cleanup
of old installs is a separate one-shot script, never %post. Symlink-into-
checkout installers are for *config repos* (greetd-config) only — never for
binaries or anything root executes.

- Migration queue: hyprnotice (symlink installer), logind-idle-control
  (has a spec — converge layout), voice-dictation (has a PKGBUILD — add RPM),
  lmtt, waybar-workspace-buttons, couchcord (when it ships).

## 11. Cross-tool contracts are registered and versioned

The suite is a web of small contracts. Each one is registered in
`registry/surfaces.toml` with an owner, producers, consumers, transport,
location, version, compatibility policy, and failure behavior. Provider
repositories own canonical schemas; desktop-commons owns the relationship and
barrier registry. New D-Bus names use a domain actually controlled by the
project and encode incompatible interface versions in the interface name.

## 12. Doctor over documentation

Every daemon ships a `doctor`/`status` subcommand that checks its real
preconditions (couchcord's de-risk gate, voice-dictation's diagnose,
hyprstate's status). A README troubleshooting section is the fallback, not
the mechanism.

## 13. Names: `hypr` only when the component only works with Hyprland

A component that depends on Hyprland IPC, its config dialect, or its
plugins is named for it (`hyprstate`, `hypr-ipc`, `hypr-de`,
`waybar-workspace-buttons`). Anything that talks Wayland protocols, D-Bus,
logind, XDG, or a neutral file contract gets a neutral name (`vigil`,
`dials`, `lmtt`, `sni-watcher`, `monitor-profiles`). Applies to repositories,
crates, binaries, desktop entries, D-Bus names, and desktop-entry keys
(`X-Dials-Section`, not `X-HyprDE-`). Existing generic crates under a
`hypr` name are tracked as debt in `concerns.toml` (`component-naming`) and
renamed at their next breaking release, not patched with aliases.
