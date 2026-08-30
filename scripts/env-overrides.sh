#!/bin/bash
# Report systemd user-manager environment variables whose live value differs
# from what environment.d would produce right now.
#
# The manager reads environment.d only when its generators run, and a
# `systemctl --user set-environment` override outranks them and never
# expires. Under Linger=yes the manager outlives every login, so a stale
# override can silently win for days: on 2026-08-30 a lingering manager
# held WALLPAPER_PATH at the package default for eight days while every
# persisted config said otherwise, and swaybg and vigil showed different
# wallpapers as a result. Nothing else surfaces this - the config files all
# look correct, because they are.
#
# Prints one line per drifted variable, `NAME live=... generated=...`, and
# exits 1 if any drifted, 0 if none. Read-only.
set -uo pipefail

# Overridable so a test can point at a fake generator and a fake `systemctl`.
gen=${ENV_GENERATOR:-/usr/lib/systemd/user-environment-generators/30-systemd-environment-d-generator}
if [ ! -x "$gen" ]; then
    echo "environment.d generator not found at $gen" >&2
    exit 2
fi

live=$(systemctl --user show-environment 2>/dev/null) || {
    echo "no systemd user manager reachable" >&2
    exit 2
}
# A clean environment, or the generator inherits this shell's PATH and
# reports it as drift. HOME and XDG_CONFIG_HOME are what it reads
# ~/.config/environment.d through; XDG_RUNTIME_DIR is expanded by some
# shipped files (60-hypr-de.conf's GNOME_KEYRING_CONTROL).
generated=$(env -i HOME="$HOME" USER="${USER:-}" \
    XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-}" XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-}" \
    "$gen" 2>/dev/null | grep -v '^PATH=')

drift=0
while IFS='=' read -r name value; do
    [ -n "$name" ] || continue
    current=$(printf '%s\n' "$live" | sed -n "s/^$name=//p" | head -n1)
    if [ "$current" != "$value" ]; then
        printf '%s live=%s generated=%s\n' "$name" "${current:-<unset>}" "$value"
        drift=1
    fi
done <<< "$generated"

exit "$drift"
