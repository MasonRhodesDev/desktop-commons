# desktop-commons

Architecture registry, design principles, recurring patterns, and eventually
shared code for the Mason Wayland desktop ecosystem (Hyprland session today).

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
                    # xdg-paths (MasonRhodesDev/xdg-paths), logind-session
                    # (MasonRhodesDev/logind-session). Planned next: hyprland
                    # socket2/instance discovery, slint-idle-runtime,
                    # #@ directive-file parsing, sysfs helpers.
                    # vigil-log and panic-exit carry the gates below
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

## Rust crates under the vigil-style gates

`vigil-log` (moved from vigil) and `panic-exit` are held to the same gates
as vigil's crates. Any other crate joins by setting `[lints] workspace = true`
in its `Cargo.toml`.

- `vigil-log`: `say!` writes one line to stderr. It does not panic or
  allocate, though a `Display` impl it formats can. It drops the line rather
  than wait on a stalled pipe, socket or tty, except in one race on a pipe
  `/proc` cannot reopen; a regular file can still wait on its disk. A tty
  `/proc` cannot reopen, such as the one `su` leaves on fd 2 when the account
  does not own it, is reached through `/dev/tty` when it is the controlling
  terminal, and otherwise loses the line, as under `su -c`. `out!` writes to
  stdout through std, ignores every error and never panics, but can block.
- `panic-exit`: `exit_on_panic()` is the first call in every `main`. A panic
  on any thread then writes `panic at <file>:<line>; exiting` through
  `vigil_log::say!` and ends the process with status 101 through `_exit`, so a
  stalled stdout or stderr cannot hold up the exit. The panic message is never
  printed, since it can quote input.

| Gate | Where |
|------|-------|
| Unsafe fence | `#![forbid(unsafe_code)]` on line 1 of every crate root, or `#![deny(unsafe_code)]` where the crate needs `libc`; `[workspace.lints]` in `Cargo.toml`; no `build.rs` or `.cargo`. `unsafe` and lint allows are counted per file in `scripts/rust-gates-baseline.tsv`, which only shrinks. |
| Clippy bans | Each crate's `clippy.toml`: `println!`, `eprintln!`, `dbg!`, `std::io::stdout`, `std::io::stderr`, `std::panic::set_hook` and `take_hook`. The lint canary `crates/panic-exit/examples/lint_canary.rs` must fail to build. |
| cargo-deny | `deny.toml`, run per crate: yanked crates denied, MIT and Apache-2.0 licences only, one version of each crate, every crate in the resolved closure listed by name, crates.io the only source. Each advisory ignore needs `reason = "expires YYYY-MM-DD: why"` no more than 183 days out. |
| Panic policy | `panic-exit` tests panic in a child process with stderr readable, on `/dev/full`, on a pipe with no reader and on a full pipe, and with a half line buffered on a full stdout. |
| Pin | A consumer outside the repository takes both crates by git at the full commit SHA, builds, and must log a line and exit 101 on a panic. |

```sh
cargo fmt -p vigil-log -p panic-exit --check
cargo clippy --locked -p vigil-log -p panic-exit --all-targets -- -D warnings
cargo test --locked -p vigil-log -p panic-exit
cargo deny --all-features --manifest-path crates/panic-exit/Cargo.toml check
python3 -I scripts/rust_gates.py all
```

`python3 -I scripts/rust_gates.py fence --update` lowers the baseline after
`unsafe` or an allow goes away. Growth needs `--allow-growth` and a reason in
the pull request. On a pull request, CI also runs the base branch's copy of the
script, so a change cannot weaken the gate that judges it.

## Pinning these crates from another repository

desktop-commons is public and MIT, and the crates are not on crates.io. A
consumer such as desktop-boot takes them by git, pinned to a full
40-character commit SHA from `main`:

```toml
[dependencies]
vigil-log = { git = "https://github.com/MasonRhodesDev/desktop-commons", rev = "<40-character SHA>" }
panic-exit = { git = "https://github.com/MasonRhodesDev/desktop-commons", rev = "<40-character SHA>" }
```

and allows that one git source in its `deny.toml`:

```toml
[sources]
unknown-git = "deny"
required-git-spec = "rev"
allow-git = ["https://github.com/MasonRhodesDev/desktop-commons"]
```

`required-git-spec = "rev"` refuses a `branch` or a `tag`, but cargo-deny
accepts a short `rev`, so the consumer enforces the full SHA itself, in a test
that reads its `Cargo.lock`. Cargo records the rev the manifest asked for in the source's
`?rev=` query and the resolved commit after `#`:

```
source = "git+https://github.com/MasonRhodesDev/desktop-commons?rev=<40 hex>#<the same 40 hex>"
```

The fragment is the full commit even for a short rev, so the check reads the
query: it must be exactly 40 lowercase hex characters and equal the fragment,
and each crate must appear once. `pin_problems` in `scripts/rust_gates.py` is
that check, and `python3 -I scripts/rust_gates.py pin` builds such a consumer
against this checkout. Add `--url https://github.com/MasonRhodesDev/desktop-commons --rev <sha>`
to fetch from GitHub instead.

To move a pin, take the new merge commit on `main`, change both `rev`
values, and run `cargo update -p vigil-log -p panic-exit`.

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
