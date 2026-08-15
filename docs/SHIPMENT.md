# Shipment plan

hypr-DE stays **alpha** until the supporting tools are daily-driver quality for people other than the author.

## Rules

- Telemetry never leaves the machine. Users attach a local `doctor` dump to a bug report.
- Bugs go on that repo’s GitHub issues board.
- Not selling until it is sellable.
- Out of scope: dotfiles overlay, couch/game-mode, Hyprland `-git`, debug-bin drop-ins.

## Order

1. **Foundations** ([hypr-commons#3](https://github.com/MasonRhodesDev/hypr-commons/issues/3))
   - `hypr-paths` — fail-closed XDG config/data/runtime (no `/run/user/<uid>`, no `/tmp`, no `~`)
   - logind helper
   - Hyprland IPC helper
   - suite-doctor (local dump + shared issue template)
   - LMTT token publish onto appearance-profiles
2. **vigil**
3. **hyprstate**
4. **hyprstate-gui** (registered power surface; closes #3)
5. **lmtt**
6. **sni-watcher**, **logind-idle-control**, **hyprland-voice-dictation**
7. **hypr-DE** leaves alpha

`hypr-paths` lives in `crates/hypr-paths`. hypr-commons is private, so the crate must be published to crates.io before public consumers depend on it.
