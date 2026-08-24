# ADR 0006: A lock request fails closed, and its policy is not session-writable

- Status: accepted
- Date: 2026-08-24

## Context

The 2026-08-24 security review of the shipped defaults found that a lock
request could end with the session unlocked and nobody the wiser.
`lock-cmd.sh` returned non-zero, hypridle logged it and moved on, and
`dpms-off-if-unlocked.sh` then blanked the outputs — because `LockedHint` was
never set. The user walked away from a dark screen indistinguishable from a
locked one, over a live desktop one keypress away.

Every lock path funnels through that wrapper: `SUPER+L` is
`loginctl lock-session`, which logind turns into hypridle's `lock_cmd`. So the
wrapper is the single point where "the session was asked to lock" either
becomes true or silently does not.

Two questions had to be answered together. What should happen when no locker
can be started? And who is allowed to change that answer?

`BAR-016` (secure-transition) already says a protected transition cannot
"silently continue fail-open" on timeout. `BAR-017`
(no-unprivileged-security-bypass) says a protected mode cannot be disabled
"through a file, environment value, or control path writable by the protected
principal". The first decides the behavior; the second decides where the knob
lives.

## Decision

**A lock request fails closed.** When the locker cannot take the session, the
wrapper escalates rather than returning:

1. Retry outside the transient systemd scope — a non-zero exit can be the
   scope failing to start rather than the locker refusing.
2. Try any other installed locker, restricted to invocations that return once
   the screen is locked (hypridle waits on `lock_cmd` before suspending, so a
   locker that blocks until unlock would hold off the suspend it was called
   for).
3. Terminate the session.

`logind`'s `LockedHint` — the same signal the blanking listener trusts — is
consulted before escalating, so a locker that succeeds while exiting non-zero
never gets the session killed out from under it. Exit code 3 from
`vigil-lock`, an idle lock cancelled by user activity, is excluded from every
rung: a nudged mouse must not escalate into a lock, let alone a logout.

Where the failsafe is not reached, it must still not lie. While a lock attempt
has failed, the blanking listener leaves the outputs lit. An obviously
unlocked screen is the honest failure; a dark one is the dangerous one.

**The policy is operator-owned, not session-owned.** `failsafe`, `fallbacks`
and `verify_tries` are read from `/etc/hypr-de/lock.conf`, honoured only when
root owns the file and nobody else can write it, and parsed rather than
sourced. The environment is not consulted at all.

## Consequences

- A total lock failure costs unsaved work. That is the accepted price: the
  user asked for the session to be protected, and the alternative is an
  exposed desktop that looks protected.
- Suite lockers gain a contract they must keep. `vigil-lock`'s exit codes are
  now load-bearing — 0 locked, 3 cancelled, anything else escalates — and are
  recorded in `vigil-lock-protocol-v1`.
- Relaxing the failsafe requires root. An operator who wants a machine to stay
  up writes one line in `/etc`; a process running as the user cannot.
- The lock-failure marker is a new input to DPMS arbitration and is recorded
  in `desktop-dpms-arbitration-v0`.

## Scope of the BAR-017 claim

This does not make lock policy proof against in-session code execution.
Anything running as the user can drop a `~/.config/systemd/user` override,
kill the locker, or replace the binaries it calls. What the root-owned policy
file buys is narrower and still worth having: the failsafe cannot be switched
off by *writing one durable file* — no `~/.config/environment.d` line, no
`~/.local/bin` shadow, nothing that survives a reboot and quietly persists.
The threat this closes is a planted, durable weakening; the threat it does not
close is live code execution, which no user-space lock policy can.

The same reasoning applies to every executable the lock and consent paths
name: `hypridle`, the share picker, and `busctl` are searched over root-owned
directories rather than `$PATH`, for the same reason.

## Rejected alternatives

**Log and continue (the previous behavior).** It is what most desktops do, and
it is the bug: the failure is invisible precisely when it matters, because
DPMS makes it look like success.

**Fall back to a VT switch instead of terminating.** It needs privileges the
session may not have, and leaves a live session on another VT — protected only
by whatever locks that VT, which is the thing that just failed.

**Keep an environment override for convenience.** `~/.config/environment.d`
makes it durable, which turns a convenience into a persistent, one-file
disarm of the failsafe — exactly the shape BAR-017 names.

**Make the fallback locker a package dependency.** ADR 0003 keeps the base
session narrow; the fallbacks are used only if the user already has one
installed, and none of them are dependencies.
