#!/bin/bash
# Local suite health dump. Attach the file to a GitHub issue. Nothing leaves
# the machine.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHECKLIST="$ROOT/docs/health-checklist.txt"
OUT="${XDG_RUNTIME_DIR:-}/hypr-suite-doctor.txt"

while [ $# -gt 0 ]; do
    case "$1" in
        --out)
            OUT="${2:?--out needs a path}"
            shift 2
            ;;
        *)
            echo "usage: suite-doctor.sh [--out PATH]" >&2
            exit 2
            ;;
    esac
done

if [ -z "$OUT" ]; then
    echo "XDG_RUNTIME_DIR is unset and --out was not given" >&2
    exit 1
fi

if [ ! -f "$CHECKLIST" ]; then
    echo "missing $CHECKLIST; run scripts/architecture.py generate" >&2
    exit 1
fi

mkdir -p "$(dirname "$OUT")"
{
    echo "# hypr suite doctor"
    echo "date: $(date --iso-8601=seconds)"
    echo "host: $(hostname)"
    echo

    while IFS=$'\t' read -r kind a b; do
        [ -n "$kind" ] || continue
        case "$kind" in
            unit)
                echo "## unit $b ($a)"
                if [ "$a" = user ]; then
                    echo "enabled: $(systemctl --user is-enabled "$b" 2>&1 || true)"
                    echo "active: $(systemctl --user is-active "$b" 2>&1 || true)"
                    journalctl --user -u "$b" -n 20 --no-pager 2>&1 || true
                else
                    echo "enabled: $(systemctl is-enabled "$b" 2>&1 || true)"
                    echo "active: $(systemctl is-active "$b" 2>&1 || true)"
                    journalctl -u "$b" -n 20 --no-pager 2>&1 || true
                fi
                echo
                ;;
            package)
                echo "## package $a"
                if command -v pacman >/dev/null 2>&1; then
                    pacman -Q "$a" 2>&1 || true
                elif command -v rpm >/dev/null 2>&1; then
                    rpm -q "$a" 2>&1 || true
                else
                    echo "no pacman or rpm"
                fi
                echo
                ;;
            portal)
                echo "## portal $a"
                if command -v busctl >/dev/null 2>&1; then
                    busctl --user list 2>/dev/null | grep -F "$a" || echo "not on session bus"
                else
                    echo "busctl missing"
                fi
                echo
                ;;
            command)
                echo "## $a"
                # shellcheck disable=SC2086
                $a 2>&1 || true
                echo
                ;;
        esac
    done < "$CHECKLIST"

    # Not in the checklist because it is not a unit, package or portal: it
    # is the user manager's environment itself. A `set-environment` override
    # outranks environment.d and never expires, and under Linger=yes the
    # manager outlives every login - so a stale value can win for days
    # while every config file looks correct. That is exactly what the
    # config-file checks above cannot see.
    echo "## environment overrides (live vs environment.d)"
    if "$ROOT/scripts/env-overrides.sh" 2>&1; then
        echo "none"
    fi
    echo
} > "$OUT"

echo "$OUT"
