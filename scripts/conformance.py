#!/usr/bin/env python3
"""Evaluate barrier-linked assertions against GitHub or local repositories."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from dataclasses import asdict, dataclass
from datetime import date
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
REGISTRY = ROOT / "registry"


def load(name: str) -> dict:
    with (REGISTRY / name).open("rb") as handle:
        return tomllib.load(handle)


class ProviderError(RuntimeError):
    pass


class GitHubProvider:
    def __init__(self, repositories: dict[str, dict]) -> None:
        self.repositories = repositories
        self.content_cache: dict[tuple[str, str], str | None] = {}
        self.metadata_cache: dict[str, dict] = {}
        self.tree_cache: dict[str, list[str]] = {}

    def run(self, arguments: list[str]) -> str:
        process = subprocess.run(
            ["gh", "api", *arguments],
            check=False,
            capture_output=True,
            text=True,
        )
        if process.returncode == 0:
            return process.stdout
        if "HTTP 404" in process.stderr:
            raise FileNotFoundError(process.stderr.strip())
        raise ProviderError(process.stderr.strip() or "gh api failed")

    def github_name(self, repository: str) -> str:
        return self.repositories[repository]["github"]

    def metadata(self, repository: str) -> dict:
        if repository not in self.metadata_cache:
            output = self.run([f"repos/{self.github_name(repository)}"])
            self.metadata_cache[repository] = json.loads(output)
        return self.metadata_cache[repository]

    def content(self, repository: str, path: str) -> str | None:
        key = (repository, path)
        if key not in self.content_cache:
            try:
                self.content_cache[key] = self.run(
                    [
                        f"repos/{self.github_name(repository)}/contents/{path}",
                        "-H",
                        "Accept: application/vnd.github.raw+json",
                    ]
                )
            except FileNotFoundError:
                self.content_cache[key] = None
        return self.content_cache[key]

    def paths(self, repository: str) -> list[str]:
        if repository not in self.tree_cache:
            branch = self.metadata(repository)["default_branch"]
            output = self.run(
                [
                    f"repos/{self.github_name(repository)}/git/trees/{branch}?recursive=1",
                ]
            )
            document = json.loads(output)
            if document.get("truncated"):
                raise ProviderError(
                    f"{repository} recursive tree was truncated by the GitHub API"
                )
            tree = document.get("tree", [])
            self.tree_cache[repository] = [
                row["path"] for row in tree if row.get("type") == "blob"
            ]
        return self.tree_cache[repository]


class LocalProvider:
    def __init__(self, repositories: dict[str, dict], workspace_root: Path) -> None:
        self.repositories = repositories
        self.workspace_root = workspace_root

    def repository_path(self, repository: str) -> Path:
        row = self.repositories[repository]
        candidates = []
        if row.get("local"):
            candidates.append(self.workspace_root / row["local"])
        candidates.extend(
            [
                self.workspace_root / row["github"].split("/", 1)[1],
                self.workspace_root / repository,
            ]
        )
        for candidate in candidates:
            if candidate.name and candidate.is_dir():
                return candidate
        return candidates[1]

    def metadata(self, repository: str) -> dict:
        if not self.repository_path(repository).is_dir():
            raise ProviderError(f"local checkout for {repository} is missing")
        row = self.repositories[repository]
        return {
            "archived": row["lifecycle"] == "archived",
            "default_branch": "main",
        }

    def content(self, repository: str, path: str) -> str | None:
        target = self.repository_path(repository) / path
        if not target.is_file():
            return None
        return target.read_text(encoding="utf-8")

    def paths(self, repository: str) -> list[str]:
        base = self.repository_path(repository)
        if not base.is_dir():
            raise ProviderError(f"local checkout for {repository} is missing")
        return [
            str(path.relative_to(base))
            for path in base.rglob("*")
            if path.is_file() and ".git" not in path.parts
        ]


@dataclass
class Finding:
    assertion: str
    status: str
    severity: str
    message: str
    details: list[str]
    barriers: list[str]
    concerns: list[str]
    waiver_expires: str | None


def targets(assertion: dict) -> list[str]:
    if assertion.get("repositories"):
        return assertion["repositories"]
    return [assertion["repository"]]


def regex_matches(pattern: str, content: str) -> bool:
    return re.search(pattern, content) is not None


def evaluate_file(assertion: dict, provider: GitHubProvider | LocalProvider) -> list[str]:
    failures: list[str] = []
    for repository in targets(assertion):
        content = provider.content(repository, assertion["path"])
        if content is None:
            failures.append(f"{repository}:{assertion['path']} is missing")
            continue
        matched = regex_matches(assertion["pattern"], content)
        if assertion["kind"] == "file_contains" and not matched:
            failures.append(
                f"{repository}:{assertion['path']} does not match {assertion['pattern']!r}"
            )
        if assertion["kind"] == "file_not_contains" and matched:
            failures.append(
                f"{repository}:{assertion['path']} matches forbidden {assertion['pattern']!r}"
            )
    return failures


def evaluate_path(assertion: dict, provider: GitHubProvider | LocalProvider) -> list[str]:
    failures: list[str] = []
    for repository in targets(assertion):
        exists = assertion["path"] in provider.paths(repository)
        if assertion["kind"] == "path_exists" and not exists:
            failures.append(f"{repository}:{assertion['path']} is missing")
        if assertion["kind"] == "path_absent" and exists:
            failures.append(f"{repository}:{assertion['path']} must be absent")
    return failures


def evaluate_conditional(
    assertion: dict,
    provider: GitHubProvider | LocalProvider,
) -> list[str]:
    failures: list[str] = []
    repository = assertion["repository"]
    source = provider.content(repository, assertion["when_path"])
    if source is None:
        return [f"{repository}:{assertion['when_path']} is missing"]
    if not regex_matches(assertion["when_pattern"], source):
        return []
    target = provider.content(repository, assertion["path"])
    if target is None:
        return [f"{repository}:{assertion['path']} is missing"]
    if not regex_matches(assertion["pattern"], target):
        failures.append(
            f"{repository}:{assertion['when_path']} activates the condition, but "
            f"{assertion['path']} does not match {assertion['pattern']!r}"
        )
    return failures


def evaluate_archived(
    assertion: dict,
    provider: GitHubProvider | LocalProvider,
) -> list[str]:
    repository = assertion["repository"]
    actual = provider.metadata(repository).get("archived")
    if actual == assertion["expected"]:
        return []
    return [
        f"{repository} archived={actual!r}, expected {assertion['expected']!r}"
    ]


def workflow_references(content: str) -> list[tuple[int, str]]:
    references: list[tuple[int, str]] = []
    for number, line in enumerate(content.splitlines(), start=1):
        match = re.match(r"^\s*(?:-\s*)?uses:\s*([^\s#]+)", line)
        if match:
            references.append((number, match.group(1).strip("'\"")))
    return references


def evaluate_workflows(
    assertion: dict,
    provider: GitHubProvider | LocalProvider,
) -> list[str]:
    failures: list[str] = []
    for repository in targets(assertion):
        workflow_paths = [
            path
            for path in provider.paths(repository)
            if re.match(r"^\.github/workflows/.*\.ya?ml$", path)
        ]
        if not workflow_paths:
            failures.append(f"{repository} has no workflow files to inspect")
            continue
        for path in workflow_paths:
            content = provider.content(repository, path)
            if content is None:
                failures.append(f"{repository}:{path} disappeared during inspection")
                continue
            for number, reference in workflow_references(content):
                if reference.startswith("./") or reference.startswith("docker://"):
                    continue
                revision = reference.rsplit("@", 1)[-1] if "@" in reference else ""
                if not re.fullmatch(r"[0-9a-fA-F]{40}", revision):
                    failures.append(
                        f"{repository}:{path}:{number} uses mutable reference {reference}"
                    )
    return failures


def evaluate_assertion(
    assertion: dict,
    provider: GitHubProvider | LocalProvider,
) -> list[str]:
    kind = assertion["kind"]
    if kind in {"file_contains", "file_not_contains"}:
        return evaluate_file(assertion, provider)
    if kind in {"path_absent", "path_exists"}:
        return evaluate_path(assertion, provider)
    if kind == "conditional_contains":
        return evaluate_conditional(assertion, provider)
    if kind == "repository_archived":
        return evaluate_archived(assertion, provider)
    if kind == "workflow_references_pinned":
        return evaluate_workflows(assertion, provider)
    raise ValueError(f"unsupported assertion kind: {kind}")


def classify(assertion: dict, failures: list[str], today: date) -> Finding:
    waiver = assertion.get("waiver")
    waiver_expires = waiver.get("expires") if waiver else None
    if not failures:
        status = "pass-with-stale-waiver" if waiver else "pass"
        details = (
            ["The assertion passes; remove its now-unneeded waiver."]
            if waiver
            else []
        )
    elif not waiver:
        status = "violation"
        details = failures
    elif date.fromisoformat(waiver["expires"]) < today:
        status = "expired-waiver"
        details = failures + [f"Waiver expired {waiver['expires']}: {waiver['reason']}"]
    else:
        status = "waived"
        details = failures + [f"Waiver: {waiver['reason']}"]
    return Finding(
        assertion=assertion["id"],
        status=status,
        severity=assertion["severity"],
        message=assertion["message"],
        details=details,
        barriers=assertion["barriers"],
        concerns=assertion["concerns"],
        waiver_expires=waiver_expires,
    )


def evaluate_all(
    assertions: list[dict],
    provider: GitHubProvider | LocalProvider,
    today: date,
) -> list[Finding]:
    findings: list[Finding] = []
    for assertion in assertions:
        try:
            failures = evaluate_assertion(assertion, provider)
            findings.append(classify(assertion, failures, today))
        except (ProviderError, OSError, ValueError, json.JSONDecodeError) as error:
            findings.append(
                Finding(
                    assertion=assertion["id"],
                    status="error",
                    severity="error",
                    message=assertion["message"],
                    details=[str(error)],
                    barriers=assertion["barriers"],
                    concerns=assertion["concerns"],
                    waiver_expires=None,
                )
            )
    return findings


def print_text(findings: list[Finding]) -> None:
    for finding in findings:
        print(f"{finding.status}: {finding.assertion}: {finding.message}")
        for detail in finding.details:
            print(f"  - {detail}")


def print_github(findings: list[Finding]) -> None:
    for finding in findings:
        if finding.status == "pass":
            continue
        level = "error"
        if finding.status in {"waived", "pass-with-stale-waiver"}:
            level = "warning"
        details = " | ".join(finding.details)
        print(
            f"::{level} title={finding.assertion}::"
            f"{finding.status}: {finding.message} {details}"
        )


def print_markdown(findings: list[Finding], output=sys.stdout) -> None:
    counts = {}
    for finding in findings:
        counts[finding.status] = counts.get(finding.status, 0) + 1
    print("# Cross-repository conformance", file=output)
    print(file=output)
    print(
        " ".join(f"`{status}`: {count}" for status, count in sorted(counts.items())),
        file=output,
    )
    print(file=output)
    print("| Assertion | Status | Barriers | Concerns | Waiver |", file=output)
    print("|---|---|---|---|---|", file=output)
    for finding in findings:
        print(
            f"| {finding.assertion} | {finding.status} | "
            f"{', '.join(finding.barriers)} | {', '.join(finding.concerns)} | "
            f"{finding.waiver_expires or '—'} |",
            file=output,
        )


def has_failure(findings: list[Finding], strict: bool) -> bool:
    failing = {"error", "expired-waiver", "violation"}
    if strict:
        failing.add("waived")
    return any(
        finding.status in failing and finding.severity == "error"
        for finding in findings
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", choices=["github", "local"], default="github")
    parser.add_argument("--workspace-root", type=Path, default=ROOT.parent)
    parser.add_argument(
        "--format",
        choices=["text", "github", "markdown", "json"],
        default="text",
    )
    parser.add_argument("--strict", action="store_true")
    parser.add_argument("--today", type=date.fromisoformat, default=date.today())
    parser.add_argument("--summary", type=Path)
    args = parser.parse_args()

    repositories = {
        row["id"]: row for row in load("repositories.toml").get("repository", [])
    }
    assertions = load("assertions.toml").get("assertion", [])
    provider: GitHubProvider | LocalProvider
    if args.source == "github":
        provider = GitHubProvider(repositories)
    else:
        provider = LocalProvider(repositories, args.workspace_root)

    findings = evaluate_all(assertions, provider, args.today)
    if args.format == "github":
        print_github(findings)
    elif args.format == "markdown":
        print_markdown(findings)
    elif args.format == "json":
        print(json.dumps([asdict(finding) for finding in findings], indent=2))
    else:
        print_text(findings)
    if args.summary:
        with args.summary.open("a", encoding="utf-8") as output:
            print_markdown(findings, output)

    return 1 if has_failure(findings, args.strict) else 0


if __name__ == "__main__":
    raise SystemExit(main())
