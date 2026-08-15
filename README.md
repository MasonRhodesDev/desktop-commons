# hypr-commons

Architecture registry, design principles, recurring patterns, and eventually
shared code for the Mason Hyprland desktop ecosystem.

The registries are the source of truth for repository responsibilities,
cross-repository surfaces, desktop capability ownership, external providers,
gaps, and architecture barriers. Human-facing tables and diagrams are
generated from them.

## Layout

```
registry/
  repositories.toml  # repository role, lifecycle, responsibilities, dependencies
  surfaces.toml      # versioned producer/consumer boundary contracts
  concerns.toml      # desktop capability ownership, external coverage, and gaps
  barriers.toml      # enforceable ownership and dependency constraints
  assertions.toml    # executable barrier checks and expiring known-debt waivers
docs/
  ECOSYSTEM.md       # generated repository dependency graph
  DESKTOP-COVERAGE.md # generated owned/external/hybrid/gap capability matrix
  SURFACES.md        # generated thin-surface catalog
  BARRIERS.md        # generated architecture constraints
  PRINCIPLES.md     # cross-tool design principles (the "constitution")
  PATTERNS.md       # named, reusable implementation patterns with per-tool
                    # references (where each is done well / where it's missing)
  STATUS.md         # historical 2026-06 migration worklog
  SURVEY-<date>.md  # point-in-time survey of the suite that grounded the above
scripts/
  architecture.py   # registry validation and documentation generation
  conformance.py    # live GitHub or sibling-checkout barrier scanner
crates/             # shared Rust utilities, extracted as tools converge
                    # hypr-paths is first (public mirror: MasonRhodesDev/hypr-paths).
                    # Planned next: hyprland socket2/instance discovery, logind
                    # zbus proxies, #@ directive-file parsing, sysfs helpers
```

## Working with the registry

```sh
python3 scripts/architecture.py validate
python3 scripts/architecture.py generate
python3 scripts/architecture.py check
python3 scripts/conformance.py --source github
python3 scripts/conformance.py --source local --workspace-root ..
```

Change registry TOML, regenerate, and commit both. CI rejects invalid
references, duplicate IDs, runtime dependency cycles, missing surface
compatibility/failure contracts, stale generated documentation, new
cross-repository violations, and expired waivers. Active waivers keep known
debt visible without making the default branch permanently red; `--strict`
also fails on waived violations.

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
