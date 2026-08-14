#!/usr/bin/env python3
"""Validate architecture registries and generate human-facing documentation."""

from __future__ import annotations

import argparse
import sys
import tomllib
from collections import Counter, defaultdict
from datetime import date
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
REGISTRY = ROOT / "registry"


def load(name: str) -> dict:
    with (REGISTRY / name).open("rb") as handle:
        return tomllib.load(handle)


def duplicate_ids(rows: list[dict], kind: str, errors: list[str]) -> None:
    counts = Counter(row.get("id", "") for row in rows)
    for value, count in counts.items():
        if not value:
            errors.append(f"{kind} has an empty id")
        elif count > 1:
            errors.append(f"duplicate {kind} id: {value}")


def check_refs(
    values: list[str],
    valid: set[str],
    context: str,
    errors: list[str],
    *,
    external_allowed: bool = False,
) -> None:
    for value in values:
        if external_allowed and value.startswith("external:"):
            continue
        if value not in valid:
            errors.append(f"{context} references unknown repository: {value}")


def find_dependency_cycles(repositories: list[dict], errors: list[str]) -> None:
    graph = {row["id"]: row.get("depends_on", []) for row in repositories}
    visiting: set[str] = set()
    visited: set[str] = set()

    def visit(node: str, path: list[str]) -> None:
        if node in visiting:
            start = path.index(node)
            errors.append(f"runtime dependency cycle: {' -> '.join(path[start:] + [node])}")
            return
        if node in visited:
            return
        visiting.add(node)
        for dependency in graph[node]:
            visit(dependency, path + [node])
        visiting.remove(node)
        visited.add(node)

    for repository in graph:
        visit(repository, [])


def validate() -> tuple[list[str], list[str], dict[str, list[dict]]]:
    repositories = load("repositories.toml").get("repository", [])
    surfaces = load("surfaces.toml").get("surface", [])
    concern_doc = load("concerns.toml")
    concerns = concern_doc.get("concern", [])
    barrier_doc = load("barriers.toml")
    barriers = barrier_doc.get("barrier", [])
    assertions = load("assertions.toml").get("assertion", [])

    errors: list[str] = []
    warnings: list[str] = []

    duplicate_ids(repositories, "repository", errors)
    duplicate_ids(surfaces, "surface", errors)
    duplicate_ids(concerns, "concern", errors)
    duplicate_ids(barriers, "barrier", errors)
    duplicate_ids(assertions, "assertion", errors)

    repository_ids = {row["id"] for row in repositories}
    barrier_ids = {row["id"] for row in barriers}
    concern_ids = {row["id"] for row in concerns}
    statuses = set(concern_doc.get("statuses", []))
    risks = set(concern_doc.get("risks", []))
    priorities = {"P0", "P1", "P2", "P3"}

    for repository in repositories:
        check_refs(
            repository.get("depends_on", []),
            repository_ids,
            f"repository {repository['id']}",
            errors,
        )
    find_dependency_cycles(repositories, errors)

    required_surface_fields = {
        "id",
        "owner",
        "producers",
        "consumers",
        "transport",
        "location",
        "version",
        "maturity",
        "compatibility",
        "failure",
        "barriers",
    }
    for surface in surfaces:
        missing = sorted(required_surface_fields - surface.keys())
        if missing:
            errors.append(f"surface {surface.get('id', '<unknown>')} missing: {', '.join(missing)}")
            continue
        check_refs(
            [surface["owner"]],
            repository_ids,
            f"surface {surface['id']} owner",
            errors,
            external_allowed=True,
        )
        check_refs(
            surface["producers"],
            repository_ids,
            f"surface {surface['id']} producer",
            errors,
            external_allowed=True,
        )
        check_refs(
            surface["consumers"],
            repository_ids,
            f"surface {surface['id']} consumer",
            errors,
            external_allowed=True,
        )
        for barrier in surface["barriers"]:
            if barrier not in barrier_ids:
                errors.append(f"surface {surface['id']} references unknown barrier: {barrier}")
        if surface["version"].strip().lower() in {"", "none", "unversioned"}:
            warnings.append(f"surface {surface['id']} has no usable version")
        if not surface["failure"].strip():
            errors.append(f"surface {surface['id']} has no failure behavior")

    owner_required = {"owned", "hybrid", "planned", "legacy"}
    for concern in concerns:
        concern_id = concern["id"]
        if concern.get("status") not in statuses:
            errors.append(f"concern {concern_id} has unknown status: {concern.get('status')}")
        if concern.get("risk") not in risks:
            errors.append(f"concern {concern_id} has unknown risk: {concern.get('risk')}")
        if concern.get("priority") and concern["priority"] not in priorities:
            errors.append(
                f"concern {concern_id} has unknown priority: {concern['priority']}"
            )
        if concern.get("status") in owner_required and not concern.get("owner"):
            errors.append(f"concern {concern_id} requires an owner")
        if concern.get("owner") and concern["owner"] not in repository_ids:
            errors.append(f"concern {concern_id} references unknown owner: {concern['owner']}")
        check_refs(
            concern.get("internal", []),
            repository_ids,
            f"concern {concern_id}",
            errors,
        )
        for barrier in concern.get("barriers", []):
            if barrier not in barrier_ids:
                errors.append(f"concern {concern_id} references unknown barrier: {barrier}")
        if concern.get("status") == "external" and not concern.get("external"):
            errors.append(f"external concern {concern_id} names no provider")
        if concern.get("status") == "gap" and concern.get("owner"):
            errors.append(f"gap concern {concern_id} must not have an owner")

    assertion_kinds = {
        "conditional_contains",
        "file_contains",
        "file_not_contains",
        "path_absent",
        "path_exists",
        "repository_archived",
        "workflow_references_pinned",
    }
    for assertion in assertions:
        assertion_id = assertion["id"]
        if assertion.get("kind") not in assertion_kinds:
            errors.append(
                f"assertion {assertion_id} has unknown kind: {assertion.get('kind')}"
            )
        targets = []
        if assertion.get("repository"):
            targets.append(assertion["repository"])
        targets.extend(assertion.get("repositories", []))
        if not targets:
            errors.append(f"assertion {assertion_id} has no repository target")
        check_refs(targets, repository_ids, f"assertion {assertion_id}", errors)
        if not assertion.get("barriers"):
            errors.append(f"assertion {assertion_id} has no barrier")
        for barrier in assertion.get("barriers", []):
            if barrier not in barrier_ids:
                errors.append(
                    f"assertion {assertion_id} references unknown barrier: {barrier}"
                )
        if not assertion.get("concerns"):
            errors.append(f"assertion {assertion_id} has no concern")
        for concern in assertion.get("concerns", []):
            if concern not in concern_ids:
                errors.append(
                    f"assertion {assertion_id} references unknown concern: {concern}"
                )
        if assertion.get("severity") not in {"error", "warning"}:
            errors.append(
                f"assertion {assertion_id} has unknown severity: {assertion.get('severity')}"
            )
        kind = assertion.get("kind")
        if kind in {"file_contains", "file_not_contains"}:
            for field in ("path", "pattern"):
                if not assertion.get(field):
                    errors.append(f"assertion {assertion_id} missing {field}")
        elif kind in {"path_absent", "path_exists"}:
            if not assertion.get("path"):
                errors.append(f"assertion {assertion_id} missing path")
        elif kind == "conditional_contains":
            for field in ("when_path", "when_pattern", "path", "pattern"):
                if not assertion.get(field):
                    errors.append(f"assertion {assertion_id} missing {field}")
        elif kind == "repository_archived" and not isinstance(
            assertion.get("expected"), bool
        ):
            errors.append(f"assertion {assertion_id} missing boolean expected")
        waiver = assertion.get("waiver")
        if waiver:
            if not waiver.get("reason"):
                errors.append(f"assertion {assertion_id} waiver has no reason")
            try:
                date.fromisoformat(waiver.get("expires", ""))
            except ValueError:
                errors.append(
                    f"assertion {assertion_id} waiver has invalid expiry: "
                    f"{waiver.get('expires')}"
                )

    participation: set[str] = set()
    for concern in concerns:
        participation.update(concern.get("internal", []))
        if concern.get("owner"):
            participation.add(concern["owner"])
    for surface in surfaces:
        participation.update(value for value in surface["producers"] if not value.startswith("external:"))
        participation.update(value for value in surface["consumers"] if not value.startswith("external:"))
        if not surface["owner"].startswith("external:"):
            participation.add(surface["owner"])
    for repository in repositories:
        if repository["id"] not in participation:
            warnings.append(f"repository {repository['id']} has no concern or surface participation")

    data = {
        "repositories": repositories,
        "surfaces": surfaces,
        "concerns": concerns,
        "barriers": barriers,
        "assertions": assertions,
    }
    return errors, warnings, data


def mermaid_id(value: str) -> str:
    return "repo_" + value.replace("-", "_")


def render_ecosystem(data: dict[str, list[dict]]) -> str:
    repositories = data["repositories"]
    role_groups: dict[str, list[dict]] = defaultdict(list)
    for repository in repositories:
        role_groups[repository["role"]].append(repository)

    lines = [
        "# Ecosystem",
        "",
        "> Generated by `scripts/architecture.py`; edit `registry/*.toml`, not this file.",
        "",
        f"{len(repositories)} registered repositories. Arrows point from a dependency to its consumer.",
        "",
        "```mermaid",
        "flowchart LR",
    ]
    for role in sorted(role_groups):
        lines.append(f'    subgraph role_{role.replace("-", "_")}["{role}"]')
        for repository in sorted(role_groups[role], key=lambda row: row["id"]):
            label = repository["id"]
            if repository["lifecycle"] != "active":
                label += f" ({repository['lifecycle']})"
            lines.append(f'        {mermaid_id(repository["id"])}["{label}"]')
        lines.append("    end")
    for repository in sorted(repositories, key=lambda row: row["id"]):
        for dependency in sorted(repository.get("depends_on", [])):
            lines.append(
                f'    {mermaid_id(dependency)} --> {mermaid_id(repository["id"])}'
            )
    lines.extend(["```", "", "## Repository registry", ""])
    lines.append("| Repository | Role | Lifecycle | Responsibilities |")
    lines.append("|---|---|---|---|")
    for repository in sorted(repositories, key=lambda row: row["id"]):
        url = f"https://github.com/{repository['github']}"
        responsibilities = "; ".join(repository["responsibilities"])
        lines.append(
            f"| [{repository['id']}]({url}) | {repository['role']} | "
            f"{repository['lifecycle']} | {responsibilities} |"
        )
    lines.append("")
    return "\n".join(lines)


def concern_priority(concern: dict) -> str:
    if concern.get("priority"):
        return concern["priority"]
    return {"high": "P1", "medium": "P2", "low": "P3"}[concern["risk"]]


def render_coverage(data: dict[str, list[dict]]) -> str:
    concerns = data["concerns"]
    status_counts = Counter(row["status"] for row in concerns)
    lines = [
        "# Desktop capability coverage",
        "",
        "> Generated by `scripts/architecture.py`; edit `registry/concerns.toml`, not this file.",
        "",
        "## Summary",
        "",
        "| Status | Count |",
        "|---|---:|",
    ]
    for status in sorted(status_counts):
        lines.append(f"| {status} | {status_counts[status]} |")

    lines.extend(
        [
            "",
            "## Gaps and high-risk boundaries",
            "",
            "| Priority | Concern | Area | Status | Owner | External provider | Risk |",
            "|---|---|---|---|---|---|---|",
        ]
    )
    priority = [
        row
        for row in concerns
        if row["status"] in {"gap", "planned"} or row["risk"] == "high"
    ]
    for concern in sorted(
        priority,
        key=lambda row: (concern_priority(row), row["area"], row["id"]),
    ):
        lines.append(
            f"| {concern_priority(concern)} | {concern['id']} | {concern['area']} | {concern['status']} | "
            f"{concern['owner'] or '—'} | {', '.join(concern['external']) or '—'} | "
            f"{concern['risk']} |"
        )

    lines.extend(
        [
            "",
            "## Complete matrix",
            "",
            "| Priority | Concern | Area | Status | Owner | External provider | Risk | Notes |",
            "|---|---|---|---|---|---|---|---|",
        ]
    )
    for concern in sorted(concerns, key=lambda row: (row["area"], row["id"])):
        lines.append(
            f"| {concern_priority(concern)} | {concern['id']} | {concern['area']} | {concern['status']} | "
            f"{concern['owner'] or '—'} | {', '.join(concern['external']) or '—'} | "
            f"{concern['risk']} | {concern['notes']} |"
        )
    lines.append("")
    return "\n".join(lines)


def render_surfaces(data: dict[str, list[dict]]) -> str:
    surfaces = data["surfaces"]
    debt = [
        surface
        for surface in surfaces
        if surface["maturity"] in {"provisional", "legacy"}
    ]
    lines = [
        "# Cross-repository surfaces",
        "",
        "> Generated by `scripts/architecture.py`; edit `registry/surfaces.toml`, not this file.",
        "",
        "A surface is the narrow contract at a repository boundary. Shared implementation is not implied.",
        "",
        "## Architecture debt",
        "",
        "| Surface | Maturity | Reason to revisit |",
        "|---|---|---|",
    ]
    for surface in sorted(debt, key=lambda row: row["id"]):
        lines.append(
            f"| {surface['id']} | {surface['maturity']} | {surface['compatibility']} |"
        )
    lines.extend(
        [
            "",
            "## Complete surface registry",
            "",
        "| Surface | Owner | Producers | Consumers | Transport | Version | Maturity |",
        "|---|---|---|---|---|---|---|",
        ]
    )
    for surface in sorted(surfaces, key=lambda row: row["id"]):
        lines.append(
            f"| {surface['id']} | {surface['owner']} | {', '.join(surface['producers'])} | "
            f"{', '.join(surface['consumers'])} | {surface['transport']} | "
            f"{surface['version']} | {surface['maturity']} |"
        )
    for surface in sorted(surfaces, key=lambda row: row["id"]):
        lines.extend(
            [
                "",
                f"## {surface['id']}",
                "",
                f"- Location: `{surface['location']}`",
                f"- Compatibility: {surface['compatibility']}",
                f"- Failure behavior: {surface['failure']}",
                f"- Barriers: {', '.join(surface['barriers'])}",
            ]
        )
    lines.append("")
    return "\n".join(lines)


def render_barriers(data: dict[str, list[dict]]) -> str:
    barriers = data["barriers"]
    assertions = data["assertions"]
    assertion_counts = Counter(
        barrier
        for assertion in assertions
        for barrier in assertion.get("barriers", [])
    )
    lines = [
        "# Architecture barriers",
        "",
        "> Generated by `scripts/architecture.py`; edit `registry/barriers.toml`, not this file.",
        "",
        "Barriers are review and CI constraints. They prevent ownership drift while allowing implementations to evolve independently.",
        "",
        "| ID | Name | Severity | Rule |",
        "|---|---|---|---|",
    ]
    for barrier in sorted(barriers, key=lambda row: row["id"]):
        lines.append(
            f"| {barrier['id']} | {barrier['name']} | {barrier['severity']} | {barrier['rule']} |"
        )
    lines.extend(
        [
            "",
            "## Automated coverage",
            "",
            "| Barrier | Assertions |",
            "|---|---:|",
        ]
    )
    for barrier in sorted(barriers, key=lambda row: row["id"]):
        lines.append(f"| {barrier['id']} | {assertion_counts[barrier['id']]} |")
    lines.extend(
        [
            "",
            "Assertions with zero counts remain review-only barriers until an executable check is added.",
            "",
            "## Assertions",
            "",
            "| Assertion | Kind | Repository | Barriers | Concerns | Waiver expires |",
            "|---|---|---|---|---|---|",
        ]
    )
    for assertion in sorted(assertions, key=lambda row: row["id"]):
        targets = assertion.get("repositories") or [assertion["repository"]]
        waiver = assertion.get("waiver", {})
        lines.append(
            f"| {assertion['id']} | {assertion['kind']} | {', '.join(targets)} | "
            f"{', '.join(assertion['barriers'])} | {', '.join(assertion['concerns'])} | "
            f"{waiver.get('expires', '—')} |"
        )
    lines.append("")
    return "\n".join(lines)


def generated_documents(data: dict[str, list[dict]]) -> dict[Path, str]:
    return {
        ROOT / "docs" / "ECOSYSTEM.md": render_ecosystem(data),
        ROOT / "docs" / "DESKTOP-COVERAGE.md": render_coverage(data),
        ROOT / "docs" / "SURFACES.md": render_surfaces(data),
        ROOT / "docs" / "BARRIERS.md": render_barriers(data),
    }


def write_documents(documents: dict[Path, str]) -> None:
    for path, content in documents.items():
        path.write_text(content, encoding="utf-8")
        print(f"generated {path.relative_to(ROOT)}")


def check_documents(documents: dict[Path, str], errors: list[str]) -> None:
    for path, expected in documents.items():
        if not path.exists():
            errors.append(f"generated document missing: {path.relative_to(ROOT)}")
            continue
        actual = path.read_text(encoding="utf-8")
        if actual != expected:
            errors.append(
                f"generated document is stale: {path.relative_to(ROOT)} "
                "(run scripts/architecture.py generate)"
            )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["validate", "generate", "check"], nargs="?", default="validate")
    args = parser.parse_args()

    errors, warnings, data = validate()
    for warning in warnings:
        print(f"warning: {warning}", file=sys.stderr)

    documents = generated_documents(data)
    if args.command == "generate" and not errors:
        write_documents(documents)
    elif args.command == "check":
        check_documents(documents, errors)

    for error in errors:
        print(f"error: {error}", file=sys.stderr)
    if errors:
        return 1

    print(
        f"ok: {len(data['repositories'])} repositories, "
        f"{len(data['surfaces'])} surfaces, "
        f"{len(data['concerns'])} concerns, "
        f"{len(data['barriers'])} barriers, "
        f"{len(data['assertions'])} assertions"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
