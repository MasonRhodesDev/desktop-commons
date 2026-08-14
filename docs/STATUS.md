# Suite status — historical migration worklog

> Historical snapshot from 2026-06-12. It is retained as migration context,
> not current ecosystem truth. Current repository status, relationships,
> surfaces, ownership, external providers, and gaps live in `registry/` and
> the generated `ECOSYSTEM.md`, `SURFACES.md`, and `DESKTOP-COVERAGE.md`.

_Last updated: 2026-06-12 (from mason-work)._

## hyprstate v2 (Rust rewrite + packaging) — IN FLIGHT

Plan: Rust port → RPM (COPR) + Arch PKGBUILD from one tag; Python v1 deleted
only after soak. Specs (POWER_SPEC/GPU_SPEC in-repo) are the behavioral
contract.

**Branches (pushed to github.com/MasonRhodesDev/hyprstate):**
- `power-management` — Python v1 + review fixes (sleep-hook privilege path,
  AC-axis bug, reconciler-drift transitions, profile-switch staleness,
  53-test pure-layer suite). Production baseline.
- `v2-rust` (off power-management) — the Rust port, complete: pure layer
  (57 tests), gpu/power/profile/status CLIs, powerd, sleep-hook, daemon
  (shadow mode + live), `profile save` capture, dist/ + packaging/ (RPM spec
  build-verified locally, PKGBUILD untested until a tag exists).
- `main` is still pre-review-fixes; merge power-management → main before
  tagging.

**Live deployment is mason-work ONLY** (other machines still run the Python
symlink install until the RPM lands):
- Binary: root-owned `/usr/local/libexec/hyprstate-v2`.
- powerd: drop-in `/etc/systemd/system/hyprstate-powerd.service.d/v2-rust.conf`.
- user daemon: drop-in `~/.config/systemd/user/hyprstate.service.d/v2-rust.conf`.
- sleep hook: root-owned copy at `/usr/lib/systemd/system-sleep/hyprstate`.
- uwsm gpu-select still calls the Python symlink (byte-identical output;
  switches at RPM cutover).
- Revert any seam: remove its drop-in/hook, daemon-reload, restart.

**Verification done:** all CLIs byte-diffed against Python on live hardware;
powerd per-profile ApplyProfile rows byte-identical under root; daemon shadow
parity (identical decisions at identical timestamps); RPM built end-to-end
locally from a `--head` srpm.

**Gates before tag v2.0.0:**
1. Soak (started 2026-06-12): physical checklist on mason-work — lid
   close/open ×{docked,undocked,inhibited}, 30s suspend with lock,
   reopen-cancel, suspend/resume (powerd re-apply + USB wake), AC settle +
   brightness edges, `power set` override stamp/expiry. Watch
   `journalctl --user -u hyprstate` for unexplained reconciler-drift WARNs.
2. FAS account + COPR token → `copr-cli create hyprstate --chroot
   fedora-42-x86_64 --chroot fedora-43-x86_64`; AUR account only if
   publishing there (local makepkg needs nothing).
3. Cutover commit: delete hyprstate.py/install.sh/test_hyprstate.py/root
   unit copies, README rewrite, merge to main, tag.
4. Run `packaging/migrate-from-devinstall.sh` per machine, then dnf/pacman
   install. Chezmoi: rewrite run_once_after_30-install-hyprstate.sh (now
   needs distro detection: dnf copr vs paru/makepkg), packages.toml
   `verify = "rpm -q hyprstate"` (or pacman equivalent), uwsm env-hyprland
   `_gpu_sel=$(command -v hyprstate || true)`.

## Suite decisions & queues

- **hyprdm archived** (2026-06-12): local-only repo, tombstone commit on
  mason-work; editor folded into hyprstate as `profile save`. See
  PATTERNS.md §Decision record.
- **Packaging migration queue** (PRINCIPLES §10): hyprnotice,
  logind-idle-control, voice-dictation, lmtt, waybar-workspace-buttons,
  couchcord — converge on the hyprstate dist/+packaging pattern during
  normal maintenance.
- **Crates extraction queue** (PATTERNS.md): hypr-ipc → logind-util →
  directive-conf; extract on second consumer.
- lmtt is on zbus 4 — bump to 5 when touched.

## Resuming on a fresh machine

1. Clone this repo; read PRINCIPLES.md / PATTERNS.md / this file.
2. `gh repo clone MasonRhodesDev/hyprstate ~/repos/hyprstate && git checkout
   v2-rust` — everything above is in-tree (specs, plan context in commit
   messages, packaging).
3. Don't deploy v2 on another machine by hand — wait for the RPM/PKGBUILD
   path (gate 2-4 above); the drop-in seams on mason-work are a migration
   artifact, not the install method.
