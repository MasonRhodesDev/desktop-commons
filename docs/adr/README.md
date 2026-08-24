# Architecture decision records

This directory is the durable decision log for the Hypr-DE service suite.
The machine-readable registry describes the current architecture; ADRs explain
why consequential boundaries were chosen and what would be required to change
them.

## Records

| ADR | Status | Decision |
|---|---|---|
| [0001](0001-architecture-registry.md) | accepted | `desktop-commons` owns the federated architecture registry and review barriers |
| [0002](0002-event-driven-slint-runtime.md) | accepted | Long-lived Slint applications freeze their render clock when no work is due |
| [0003](0003-suite-package-boundaries.md) | accepted | Hypr-DE installs only the core session; suite applications remain independently installable |
| [0004](0004-capture-free-lock-warning.md) | accepted | Vigil warnings use compositor background effects and never capture the desktop |
| [0005](0005-settings-window-and-naming.md) | accepted | `dials` is the one settings window (native, schema-generated, or launched pages); `hypr` names only for Hyprland-only components |
| [0006](0006-fail-closed-lock-and-session-owned-policy.md) | accepted | A lock request that no locker satisfies terminates the session, and its failsafe is settable only from a root-owned file |

## When an ADR is required

Add an ADR when a change establishes or revises any of these suite-wide
contracts:

- ownership or service boundaries;
- shared state, protocol, schema, or library surfaces;
- privilege, authentication, or trust boundaries;
- process lifecycle, idle, suspend, lock, or display behavior;
- release, package, dependency, or supply-chain policy;
- compatibility policy spanning multiple repositories.

Implementation details contained within one repository do not need a suite ADR
unless they constrain another service. ADRs are append-only: supersede an old
record with a new one instead of rewriting the historical decision.

## Backfill queue

Existing accepted suite contracts that should receive focused ADRs as their
current sources are audited:

1. LMTT ownership of theme state, resolved assets, and appearance caching.
2. User/shared desktop-state layering and per-user override resolution.
3. Hyprstate ownership of monitor profiles and origin-monitor focus.
4. Released-package-only production installation and shared supply-chain gates.
