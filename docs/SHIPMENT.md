# Shipment plan

hypr-DE stays **alpha** until the supporting tools are daily-driver quality for people other than the author. It is packaged Hyprland configuration plus those tools, not a greeter session; log into stock Hyprland (uwsm).

## Rules

- Telemetry never leaves the machine. Users attach a local `doctor` dump to a bug report.
- Bugs go on that repo’s GitHub issues board.
- Not selling until it is sellable.
- Out of scope: dotfiles overlay, couch/game-mode, Hyprland `-git`, debug-bin drop-ins.

## Order

1. **Foundations** ([hypr-commons#3](https://github.com/MasonRhodesDev/hypr-commons/issues/3))
   - `hypr-paths` — fail-closed XDG config/data/runtime (no `/run/user/<uid>`, no `/tmp`, no `~`)
   - `hypr-logind` — Inhibit RAII, GetSessionByPID / XDG_SESSION_ID / scored ListSessions, never the manager path
   - Hyprland IPC helper
   - suite-doctor (local dump + shared issue template)
   - LMTT tokens via `lmtt-core` / `lmtt tokens`; published beside appearance-profiles snapshots
2. **vigil**
3. **hyprstate**
4. **hyprstate-gui** (registered power surface; closes #3)
5. **lmtt**
6. **sni-watcher**, **logind-idle-control**, **hyprland-voice-dictation**
7. **hypr-DE** leaves alpha

Shared crates are developed in `hypr-commons/crates/` and mirrored as public repos so packaged tools can git-depend on them. crates.io publish waits on a registry token.

- [MasonRhodesDev/hypr-paths](https://github.com/MasonRhodesDev/hypr-paths)
- [MasonRhodesDev/hypr-logind](https://github.com/MasonRhodesDev/hypr-logind) (`8c331f285724e671beb71595d1dc8e07a5fcde73`) — adopted by logind-idle-control and voice-dictation. hyprstate still uses local proxies.
