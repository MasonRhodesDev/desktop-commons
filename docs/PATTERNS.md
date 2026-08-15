# Patterns

Named, reusable implementation patterns with their reference implementations.
When building or refactoring a tool, link the pattern instead of re-deriving
it. Companion to PRINCIPLES.md (the *why*); this is the *how*.

## Architecture

**Layered FSM daemon** — pure transition maps (state, event, inputs) →
on-enter composition → idempotent effectors; dispatcher owns Context, sources
send typed events. Reference: hyprstate `src/pure/fsm.rs` +
`src/daemon/dispatcher.rs`. Sibling: couchcord cc-core/cc-menu.

**Effector worker queue** — fire-and-forget subprocess effects serialized on
their own task so a hung external command can never stall event dispatch;
effects whose results feed state are awaited inline. Reference: hyprstate
`src/daemon/effectors.rs` (Cmd enum + worker).

**Reconciler as snapshot task** — a poller gathers ground truth into a
snapshot event; the dispatcher diffs, repairs, and routes repairs back
through the normal transition path (repairs must *drive* the machine).
Reference: hyprstate `reconcile_snapshot_task` + `handle_reconcile_tick`.

**Boundary traits + composition root** — domain crates depend only on a core
crate; I/O implementations injected at the composition root; mock the traits
in tests. Reference: couchcord (InputSource/Renderer, couchcordd).

**Module trait + inventory registration** — plugin-style extensibility for
one-shot tools; priority classes decide sequential vs concurrent execution.
Reference: lmtt ThemeModule + `inventory!`.

## Robustness

**Endpoint rediscovery** — on repeated connect failure, re-resolve the
endpoint (scan `$XDG_RUNTIME_DIR/hypr/*/hyprland.lock` for a live PID) rather
than retrying a stale path. Reference: hyprstate `sources::socket2_path`.
Anti-reference: waybar-workspace-buttons raw retry loop.

**Isolated fragile parser with health signal** — a hack with no protocol
alternative lives in one module returning `(value, ParseHealth)`; warn on
health *transitions*; surface in `status`. Reference: hyprstate
`sysio/hypridle_log.rs`.

**Self-write suppression** — register expected values (full fallback chains)
*before* writing; consume matches one-shot; suppression window after any
write; everything else is external intent to adopt, never revert. Reference:
hyprstate SelfWriteTracker + adopt_power_override.

**Coalescing request slot** — latest-request slot + serialization lock;
superseded waiters return immediately with a marker; only first and latest
execute. Reference: hyprstate powerd ApplyProfile.

**Debounce-then-reconcile** — bursty events (monitor hotplug negotiation, AC
plug jiggle) restart a settle timer; consumers react to the settled event
only. Reference: hyprstate profile/power debounce.

**Watchdog + meaningful exit codes** — Type=notify with WatchdogSec; reserved
exit codes trigger targeted restarts (RestartForceExitStatus). Reference:
voice-dictation unit.

**Fail-closed privileged gate** — blocking, synchronous approval over a
permission-locked unix socket; every error path lands in the denied state.
Reference: greetd_game_mode approval gate.

**Inhibitor RAII** — logind Inhibit fd held in a struct; drop = release.
References: logind-idle-control InhibitorLock, hyprstate lid inhibitor.

## Migration & verification

**Seam-based migration** — replace a system one seam at a time, each
independently revertible: D-Bus interface as the seam (run new server under
old client), shadow mode for decision parity (effects logged, not fired; no
exclusive resources taken), systemd drop-ins for the cutover. Reference: the
hyprstate v2 port (M2–M5).

**Byte-diff parity testing** — port CLIs by diffing stdout against the old
implementation across an input matrix; keep quirky output formats verbatim
until cutover so diffs stay meaningful. Reference: hyprstate M2/M4 logs.

**Doctor subcommand** — check real preconditions (sockets, atoms, devices,
permissions) before first run and on demand. References: couchcord doctor,
voice-dictation diagnose.

## Config & integration

**Managed blocks in foreign files** — `# >>> tool managed >>>` markers for
idempotent inject/remove in configs the tool doesn't own. Reference: lmtt.

**Directive comments** — `#@ key = value` headers embedding metadata in
foreign-format files; per-dialect key charset pinned (no hyphens in profile
keys vs hyphens in power.conf keys). Reference: hyprstate profiles.

**Schema-driven config UI** — ship a JSON schema; schema-tui renders the
editor. References: lmtt, voice-dictation (consumers); candidate: hyprdm's
profile editor, hyprstate power.conf.

**Symlink edit-in-place installer** — for *config repos* only: symlink repo
files into /etc so edits apply immediately. Reference: greetd-config.
Explicitly banned for binaries/root-executed code (PRINCIPLES §5, §10).

## Contracts (the implicit web, made explicit)

Cross-repository contracts are machine-readable entries in
`registry/surfaces.toml`; `docs/SURFACES.md` is generated from that source.
Each entry records ownership, both sides of the relationship, transport,
location, compatibility, failure behavior, and applicable barriers. Add the
surface when the dependency is introduced, rather than documenting it after
an integration breaks.

## Consolidation queue (crates/)

Extract only when a second consumer adopts; order by duplication count:

1. **hypr-ipc** — socket2 event stream + instance discovery + typed hyprctl
   JSON queries. Today ×4 (hyprstate ✦best, waybar-workspace-buttons, hyprdm,
   voice-dictation).
2. **hypr-logind** — extracted. Public mirror MasonRhodesDev/hypr-logind.
   idle-control and voice-dictation adopted. hyprstate still has local proxies.
3. **directive-conf** — `#@` parser with pluggable key charset. Today ×2
   dialects.
4. **packaging templates** — dist/+packaging skeleton (spec, PKGBUILD,
   build-srpm.sh) copied per tool until it stabilizes, then a generator.

## Decision record

**2026-06-12 — hyprdm archived; editor folded into hyprstate.** hyprdm's v0
scaffold duplicated hyprstate's production monitor-profile subsystem. Its
editor goal landed as `hyprstate profile save <name>` (capture the live
layout into a profile, conventions lifted from the hand-written profiles:
externals matched by desc, eDP excluded from matches when externals exist,
implicit match-count priority). hyprdm is tombstoned locally with a pointer;
its hyprtoolkit GUI scaffold remains the starting point if a graphical
editor is ever wanted — emitting hyprstate profiles, never its own daemon.
