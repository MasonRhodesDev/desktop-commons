# hypr-commons

Design principles, recurring patterns, and (eventually) shared code for my
Hyprland-adjacent tool suite: hyprstate, hyprnotice, hyprland-voice-dictation,
logind-idle-control, linux-multi-theme-toggle, waybar-workspace-buttons,
couchcord, greetd_game_mode, greetd-config, and friends.

## Layout

```
docs/
  PRINCIPLES.md     # cross-tool design principles (the "constitution")
  PATTERNS.md       # named, reusable implementation patterns with per-tool
                    # references (where each is done well / where it's missing)
  STATUS.md         # living worklog — the cross-machine source of truth
  SURVEY-<date>.md  # point-in-time survey of the suite that grounded the above
crates/             # shared Rust utilities, extracted as tools converge
                    # (planned: hyprland socket2/instance discovery, logind
                    # zbus proxies, #@ directive-file parsing, sysfs helpers)
```

## Why this exists

Each tool solved the same problems independently — talking to Hyprland's
socket2, holding logind inhibitors, parsing config, installing itself,
surviving compositor restarts — with different levels of rigor. hyprstate v2
(the Rust rewrite, 2026-06) established the strongest versions of these
patterns; this repo records them so new tools start from the patterns instead
of rediscovering them, and so the existing tools can converge during normal
maintenance.

Docs first, code second: a `crates/` extraction only happens when two or more
tools actually adopt the shared version.
