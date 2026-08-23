# Shipment plan

hypr-DE stays **alpha** until the supporting tools are daily-driver quality for people other than the author. It is packaged Hyprland configuration plus those tools, not a greeter session; log into stock Hyprland (uwsm).

## Rules

- Telemetry never leaves the machine. Users attach a local `doctor` dump to a bug report.
- Bugs go on that repo’s GitHub issues board.
- Not selling until it is sellable.
- Out of scope: dotfiles overlay, couch/game-mode, Hyprland `-git`, debug-bin drop-ins.

## Order

1. **Foundations** ([desktop-commons#3](https://github.com/MasonRhodesDev/desktop-commons/issues/3))
   - `xdg-paths` — fail-closed XDG config/data/runtime (no `/run/user/<uid>`, no `/tmp`, no `~`)
   - `logind-session` — Inhibit RAII, GetSessionByPID / XDG_SESSION_ID / scored ListSessions, never the manager path
   - Hyprland IPC helper
   - suite-doctor (local dump + shared issue template)
   - LMTT tokens via `lmtt-core` / `lmtt tokens`; published beside appearance-profiles snapshots
2. **vigil**
3. **hyprstate**
4. **hyprstate-gui** (registered power surface; closes #3)
5. **lmtt**
6. **sni-watcher**, **logind-idle-control**, **wayland-voice-dictation**
7. **hypr-DE** leaves alpha

Shared crates are developed in `desktop-commons/crates/` and mirrored as public repos. Tagging the mirror publishes to crates.io from CI (`CARGO_REGISTRY_TOKEN` org secret). Do not `cargo publish` from a laptop.

- [MasonRhodesDev/xdg-paths](https://github.com/MasonRhodesDev/xdg-paths)
- [MasonRhodesDev/logind-session](https://github.com/MasonRhodesDev/logind-session) — adopted by logind-idle-control, voice-dictation, and hyprstate session/inhibit
- [MasonRhodesDev/hypr-ipc](https://github.com/MasonRhodesDev/hypr-ipc) — adopted by hyprstate and voice-dictation
- [MasonRhodesDev/slint-idle-runtime](https://github.com/MasonRhodesDev/slint-idle-runtime) — adopted by Vigil and voice-dictation

Tag `v0.1.0` on a crate mirror after the org secret `CARGO_REGISTRY_TOKEN` is set. CI publishes. Do not tag until that secret exists.
