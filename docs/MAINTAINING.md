# Maintaining the ecosystem registry

## Repository changes

Register a repository before another component takes a runtime dependency on
it. Give it one primary role and a short list of responsibilities. A
repository is not an architecture boundary merely because it has its own Git
history; responsibilities must remain non-overlapping.

When replacing a repository, keep both entries during migration, mark the old
one `legacy`, and record one active path plus the retirement condition.

## Surface changes

Register a surface when one repository relies on another repository's:

- file or directory;
- executable or CLI output;
- D-Bus name, object, signal, or method;
- Unix socket or message schema;
- library API;
- service ordering or well-known runtime identity;
- release asset or package metadata.

The provider owns the canonical schema or interface. The registry records:
owner, producers, consumers, transport, location, version, compatibility,
failure behavior, maturity, and applicable barriers.

Do not register an implementation detail that consumers cannot rely upon.
Do not create a shared crate solely to remove similar-looking code. First
stabilize the surface and test vectors; extract implementation only after two
real consumers adopt the same behavior.

## Concern changes

Every desktop capability is one of:

- `owned`: one internal repository is authoritative;
- `external`: intentionally delegated to a named provider;
- `hybrid`: an internal policy/workflow composes external mechanisms;
- `planned`: ownership is chosen but implementation is incomplete;
- `gap`: no owner or selected provider;
- `legacy`: retained only during migration;
- `non-goal`: deliberately outside the desktop product.

An external concern is not a lesser concern. It still needs a selected
provider, integration seam, failure behavior, package source, and replacement
policy. Use `risk` to prioritize boundaries whose failure can prevent login,
lose user intent, expose secrets, or make the desktop unusable.

Optional concern priorities are:

- `P0`: active security, integrity, or single-writer violation;
- `P1`: release-blocking architecture debt or high-risk missing composition;
- `P2`: meaningful capability or resilience gap;
- `P3`: low-risk completeness or polish.

## Conformance assertions

Add an assertion when a barrier can be checked from repository metadata or
versioned files. Every assertion names its barrier, concern, target
repository, severity, and actionable failure message.

The default scan fails on new violations, scanner errors, and expired
waivers. A known violation may carry a reason and ISO expiry date. Do not add
a waiver to hide a regression; use it only to baseline confirmed debt while a
fix is scheduled. A passing assertion with a waiver is reported so the waiver
can be removed. `--strict` treats active waivers as failures.

GitHub mode reads default branches through the GitHub API. Local mode reads
sibling checkouts and is useful while changing several repositories together.
The architecture workflow runs GitHub mode on registry changes and daily,
then writes the result to its job summary.

## Review cadence

- Update the registry in the same change that creates or changes a surface.
- Run a full GitHub/package/service audit before major desktop releases.
- Revisit all `gap`, `planned`, `legacy`, and high-risk rows quarterly.
- Compare package manifests and systemd units against repository
  responsibilities after dependency changes.

## Commands

```sh
python3 scripts/architecture.py validate
python3 scripts/architecture.py generate
python3 scripts/architecture.py check
python3 scripts/conformance.py --source github
python3 scripts/conformance.py --source local --workspace-root ..
```
