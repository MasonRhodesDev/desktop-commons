# ADR 0005: One settings window (dials) and the naming rule

- Status: accepted
- Date: 2026-08-22

## Context

Settings were fragmented: displays and power had a Slint GUI
(`hyprstate-gui`), themes had a schema-tui TUI, and audio, network,
Bluetooth, input, and defaults were reachable only from the bar or the
launcher. The registry recorded this as the `unified-settings` gap.

The candidate hubs outside the suite were surveyed. `lxqt-config` and
`xfce4-settings-manager` are desktop-entry launchers with a Qt/GTK stack
and their own design language; embedding (GtkSocket) is X11-only. elementary
Switchboard has a real plugin API but requires Vala/Granite plugs. GNOME,
COSMIC, Cinnamon, and Budgie compile their pages in. All of them would put a
second design system in front of `slint-kit` and give the suite an external
owner for its most visible surface.

At the same time the suite had drifted into `hypr` prefixes for components
that do not depend on Hyprland (`xdg-paths`, `logind-session`,
`slint-idle-runtime`), while `vigil` set the precedent of a neutral name for
a Wayland-targeted component.

## Decision

1. `hyprstate-gui` becomes **dials**, the suite's one settings window. It is
   a Slint application on `slint-kit`, registered as the `settings-surface`
   role and owner of `unified-settings`.
2. A dials page is one of three kinds:
   - **native** — handwritten Slint against a daemon's registered surface
     (Displays, Power, Help today);
   - **schema-generated** — rendered from the same schema a daemon already
     publishes for schema-tui (planned: Appearance/LMTT, Voice, Idle);
   - **external launch** — any XDG desktop entry with `Categories=Settings;`
     or `X-Dials-Section=` is listed and launched in its own window
     (surface `settings-entry-v1`).
   There is no plugin ABI and dials never embeds or imports another tool.
3. Audio, network, and Bluetooth remain hybrid concerns: dials launches
   the selected provider tool (pavucontrol, nm-connection-editor,
   Overskride); it does not re-implement them.
4. Naming rule (Principle 13): a `hypr`/`hyprland` name is reserved for
   components that only work with Hyprland. Wayland-, D-Bus-, logind-, or
   XDG-targeted components get neutral names. Applied in the same change,
   without compatibility aliases: `hypr-paths` → `xdg-paths` (0.2.0),
   `hypr-logind` → `logind-session` (0.2.0), `hypr-slint-runtime` →
   `slint-idle-runtime` (0.2.0), `hypr-singleton` → `singleton-guard`,
   `hypr-commons` → `desktop-commons`, `hyprland-voice-dictation` →
   `wayland-voice-dictation`. `hypr-ipc`, `hyprstate`, `hypr-de`, and
   `waybar-workspace-buttons` keep their names: they only work with Hyprland.

## Consequences

- Every suite GUI ships a desktop entry with `Categories=Settings;`; that
  is the whole registration, and it also lists the tool in LXQt/Xfce hubs.
- Displays and Power are still Hyprland-specific pages inside a
  compositor-agnostic window; the registry marks them as such rather than
  the application.
- Package consumers (`hypr-de`, `arch-repo`) depend on `dials`; the
  `hyprstate-gui` package is not aliased.
- A schema-to-Slint renderer is the next shared piece; it is extracted only
  once two daemons' pages use it (Principle 11, evidence before extraction).
- Consumers move to the new crate names in one sweep; the renamed crates
  publish first (xdg-paths, logind-session, slint-idle-runtime), then
  hypr-ipc 0.1.1, then applications. The 0.1.0 crates under the old names
  stay on crates.io unmaintained.
- New generic components must not take a `hypr` prefix.
