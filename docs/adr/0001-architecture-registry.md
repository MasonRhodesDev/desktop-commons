# ADR 0001: Federated architecture registry

- Status: accepted
- Date: 2026-08-14

## Context

The desktop is composed from many small repositories and external systems.
Relationships previously lived in READMEs, package dependencies, service
ordering, hard-coded names, and operator memory. A prose-only architecture
repository would repeat those sources and become stale.

The ecosystem also needs consistency without becoming a monorepo or a large
shared utility crate. Most repository boundaries should remain protocol,
schema, executable, or file surfaces.

## Decision

`hypr-commons` is the architecture and governance repository.

It owns machine-readable registries for:

- repository responsibilities and directed dependencies;
- thin producer/consumer surfaces;
- desktop concerns, internal ownership, external providers, and gaps;
- architecture barriers enforced during review and CI.

Human-facing matrices and Mermaid diagrams are generated from those
registries. Provider repositories continue to own canonical protocol and
schema definitions. The registry records their locations and compatibility
expectations rather than copying their implementations.

Shared code is extracted only after at least two real consumers have
converged on behavior. Contracts and test vectors come before convenience
libraries.

## Consequences

- Ownership collisions and dependency cycles become detectable.
- External dependencies become explicit architecture, not invisible gaps.
- New repositories must declare responsibilities and consumed surfaces.
- Cross-repository changes require updating the relevant surface entry.
- Generated documents must not be edited directly.
- Registry accuracy still requires periodic audit against provider
  repositories; CI can validate structure, not the truth of every claim.
