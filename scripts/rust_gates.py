#!/usr/bin/env python3
"""Vigil-style gates for the Rust crates that opt into the workspace lints.

fence       unsafe fence, workspace lint levels, crate-root attributes and the
            clippy bans; counts unsafe and lint-allow sites per file against
            a baseline that only shrinks
canary      clippy must reject crates/panic-exit/examples/lint_canary.rs for
            every ban
advisories  each cargo-deny advisory ignore carries a dated expiry
pin         a consumer outside this repository takes the gated crates by git,
            pinned to the full 40-character commit SHA, and runs them
"""

import argparse
import collections
import datetime
import json
import os
import re
import subprocess
import sys
import tempfile
import tomllib

BASELINE = "scripts/rust-gates-baseline.tsv"
COUNTED = ("unsafe", "lint-allow")
UNSAFE_WORD = re.compile(r"\bunsafe\b")
ALLOW_CALL = re.compile(r"\b(allow|expect|warn)\s*\(")
WEAKENS = re.compile(r"\bunsafe|\bclippy\b|\bwarnings\b")
ROOT_LINES = {"forbid": "#![forbid(unsafe_code)]", "deny": "#![deny(unsafe_code)]"}
FORBIDDEN_LINTS = {"rust": ["unsafe_op_in_unsafe_fn"]}
REQUIRED_LINTS = {
    "rust": ["unsafe_code", "unsafe_op_in_unsafe_fn"],
    "clippy": [
        "undocumented_unsafe_blocks",
        "multiple_unsafe_ops_per_block",
        "disallowed_macros",
        "disallowed_methods",
        "disallowed_types",
    ],
}
CLIPPY_BANS = {
    "disallowed-macros": ["std::eprintln", "std::eprint", "std::println", "std::print", "std::dbg"],
    "disallowed-methods": ["std::panic::set_hook", "std::panic::take_hook", "std::io::stderr", "std::io::stdout"],
    "disallowed-types": ["std::io::Stderr", "std::io::StderrLock", "std::io::Stdout", "std::io::StdoutLock"],
}
CANARY = "crates/panic-exit/examples/lint_canary.rs"
CANARY_CMD = ["cargo", "clippy", "--quiet", "-p", "panic-exit", "--example", "lint_canary",
              "--features", "lint-canary", "--message-format=json"]
CANARY_CODES = [
    "unsafe_code",
    "E0133",
    "clippy::undocumented_unsafe_blocks",
    "clippy::multiple_unsafe_ops_per_block",
    "clippy::disallowed_macros",
    "clippy::disallowed_methods",
    "clippy::disallowed_types",
]
ADVISORY_ID = re.compile(r"RUSTSEC-\d{4}-\d{4}")
ADVISORY_REASON = re.compile(r"expires (\d{4}-\d{2}-\d{2}): \S")
HORIZON_DAYS = 183
FULL_SHA = re.compile(r"[0-9a-f]{40}")
REPOSITORY = "https://github.com/MasonRhodesDev/desktop-commons"
PINNED = ("vigil-log", "panic-exit")


class Findings:
    def __init__(self):
        self.counts = collections.Counter()
        self.details = collections.defaultdict(list)

    def add(self, path, rule, detail, weight=1):
        self.counts[path, rule] += weight
        self.details[path, rule].append(detail)


def load_toml(path):
    with open(path, "rb") as f:
        return tomllib.load(f)


def gated_crates():
    members = load_toml("Cargo.toml")["workspace"]["members"]
    return [m for m in members if load_toml(f"{m}/Cargo.toml").get("lints", {}).get("workspace") is True]


def crate_roots(crate):
    roots = [f"{crate}/src/{name}" for name in ("lib.rs", "main.rs") if os.path.exists(f"{crate}/src/{name}")]
    for sub in ("src/bin", "tests", "examples", "benches"):
        folder = f"{crate}/{sub}"
        if os.path.isdir(folder):
            roots += sorted(f"{folder}/{n}" for n in os.listdir(folder) if n.endswith(".rs"))
    return roots


def rust_files(crate):
    for top, dirs, files in os.walk(crate):
        dirs.sort()
        for name in sorted(files):
            if name.endswith(".rs"):
                yield os.path.join(top, name)


def skip_literal(text, i):
    """The index just past a comment, string or char literal at i, or i if none starts there."""
    if text.startswith("//", i):
        end = text.find("\n", i)
        return len(text) if end < 0 else end
    if text.startswith("/*", i):
        end = text.find("*/", i + 2)
        return len(text) if end < 0 else end + 2
    raw = re.match(r'b?r(#*)"', text[i:i + 260])
    if raw and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
        end = text.find('"' + raw.group(1), i + raw.end())
        return len(text) if end < 0 else end + 1 + len(raw.group(1))
    if text[i] == '"':
        k = i + 1
        while k < len(text) and text[k] != '"':
            k += 2 if text[k] == "\\" else 1
        return k + 1
    char = re.match(r"'(\\.[^']*|[^\\'])'", text[i:i + 12])
    return i + char.end() if char else i


def attributes(text):
    """Every #[...] and #![...] in text, with brackets balanced and comments and literals skipped."""
    i = 0
    while i < len(text):
        if (past := skip_literal(text, i)) != i:
            i = past
            continue
        if not (text[i] == "#" and (text.startswith("[", i + 1) or text.startswith("![", i + 1))):
            i += 1
            continue
        start, depth, k = i, 0, i
        while k < len(text):
            if (past := skip_literal(text, k)) != k:
                k = past
                continue
            depth += {"[": 1, "]": -1}.get(text[k], 0)
            k += 1
            if text[k - 1] == "]" and depth == 0:
                break
        yield text[start:k]
        i = k


def lint_allows(text):
    """Attributes that allow, expect or warn a lint naming unsafe, clippy or warnings."""
    count = 0
    for attribute in attributes(text):
        for call in ALLOW_CALL.finditer(attribute):
            depth, k = 1, call.end()
            while k < len(attribute) and depth:
                depth += {"(": 1, ")": -1}.get(attribute[k], 0)
                k += 1
            if WEAKENS.search(attribute[call.end():k]):
                count += 1
    return count


def unsafe_words(text):
    return len(UNSAFE_WORD.findall(text))


def lint_level(setting):
    return setting.get("level") if isinstance(setting, dict) else setting


def check_workspace(found, lints):
    for tool, table in lints.items():
        for name, setting in table.items():
            if lint_level(setting) not in ("deny", "forbid"):
                found.add("Cargo.toml", "workspace-lints", f"Cargo.toml: [workspace.lints.{tool}] {name} is {lint_level(setting)}; only deny or forbid")
    for tool, names in REQUIRED_LINTS.items():
        for name in names:
            if lint_level(lints.get(tool, {}).get(name)) not in ("deny", "forbid"):
                found.add("Cargo.toml", "workspace-lints", f"Cargo.toml: [workspace.lints.{tool}] {name} must be deny or forbid")
    for tool, names in FORBIDDEN_LINTS.items():
        for name in names:
            if lint_level(lints.get(tool, {}).get(name)) != "forbid":
                found.add("Cargo.toml", "workspace-lints", f"Cargo.toml: [workspace.lints.{tool}] {name} must be forbid, since deny yields to an allow")


def check_clippy_toml(found, crate):
    path = f"{crate}/clippy.toml"
    try:
        config = load_toml(path)
    except (OSError, tomllib.TOMLDecodeError) as e:
        found.add(path, "clippy-bans", f"{path}: {e}")
        return
    for key, paths in CLIPPY_BANS.items():
        have = {entry.get("path") if isinstance(entry, dict) else entry for entry in config.get(key, [])}
        for missing in sorted(set(paths) - have):
            found.add(path, "clippy-bans", f"{path}: {key} no longer bans {missing}")


def check_crate(found, crate):
    for extra in ("build.rs", ".cargo"):
        if os.path.exists(f"{crate}/{extra}"):
            found.add(f"{crate}/{extra}", "no-build", f"{crate}/{extra}: gated crates have no build script and no .cargo")
    if "build" in load_toml(f"{crate}/Cargo.toml").get("package", {}):
        found.add(f"{crate}/Cargo.toml", "no-build", f"{crate}/Cargo.toml: package.build is not allowed")
    check_clippy_toml(found, crate)
    texts = {path: open(path, encoding="utf-8").read() for path in rust_files(crate)}
    library_unsafe = any(unsafe_words(t) for p, t in texts.items() if p.startswith(f"{crate}/src/") and "/src/bin/" not in p)
    for root in crate_roots(crate):
        first = texts[root].split("\n", 1)[0].strip()
        fenced = library_unsafe if root.endswith("src/lib.rs") else unsafe_words(texts[root]) > 0
        allowed = {ROOT_LINES["forbid"], ROOT_LINES["deny"]} if fenced else {ROOT_LINES["forbid"]}
        if first not in allowed:
            want = " or ".join(sorted(allowed))
            found.add(root, "root-attribute", f"{root}: line 1 must be {want}, not {first!r}")
    for path, text in texts.items():
        if n := unsafe_words(text):
            found.add(path, "unsafe", f"{path}: {n} unsafe", weight=n)
        if n := lint_allows(text):
            found.add(path, "lint-allow", f"{path}: {n} allow, expect or warn naming unsafe, clippy or warnings", weight=n)


def gate_fence(found, _args):
    lints = load_toml("Cargo.toml")["workspace"].get("lints", {})
    check_workspace(found, lints)
    crates = gated_crates()
    if not crates:
        found.add("Cargo.toml", "gated-crates", "Cargo.toml: no member sets [lints] workspace = true")
    for crate in crates:
        check_crate(found, crate)
    print(f"fence: {', '.join(crates)}")


def gate_canary(found, _args):
    result = subprocess.run(CANARY_CMD, capture_output=True, text=True)
    seen = set()
    for line in result.stdout.splitlines():
        try:
            message = json.loads(line)
        except ValueError:
            continue
        diagnostic = message.get("message") or {}
        code = (diagnostic.get("code") or {}).get("code")
        if message.get("reason") == "compiler-message" and diagnostic.get("level") == "error" and code:
            seen.add(code)
    if result.returncode == 0:
        found.add(CANARY, "canary", "the canary built cleanly, so the lint configuration no longer rejects it")
    for code in CANARY_CODES:
        if code not in seen:
            found.add(CANARY, "canary", f"clippy no longer rejects the canary's {code}: {result.stderr.strip()[-400:]}")


def advisory_problems(config, today):
    ignores = config.get("advisories", {}).get("ignore", [])
    if not isinstance(ignores, list):
        return ["advisories.ignore must be a list"]
    problems = []
    for entry in ignores:
        if not isinstance(entry, dict) or set(entry) != {"id", "reason"}:
            problems.append(f"{entry!r} must be a table with exactly id and reason")
            continue
        name, reason = entry["id"], entry["reason"]
        dated = isinstance(reason, str) and ADVISORY_REASON.match(reason)
        if not (isinstance(name, str) and ADVISORY_ID.fullmatch(name)) or not dated:
            problems.append(f"{name}: an ignore needs an advisory id and reason = \"expires YYYY-MM-DD: why\"")
            continue
        try:
            expires = datetime.date.fromisoformat(dated.group(1))
        except ValueError:
            problems.append(f"{name}: {dated.group(1)} is not a date")
            continue
        left = (expires - today).days
        if left < 0:
            problems.append(f"{name}: expired on {expires}; fix the dependency or re-justify it with a new date")
        elif left > HORIZON_DAYS:
            problems.append(f"{name}: expiry {expires} is more than {HORIZON_DAYS} days out")
    return problems


def gate_advisories(found, _args):
    today = datetime.date.fromisoformat(os.environ.get("GATE_TODAY") or datetime.datetime.now(datetime.UTC).date().isoformat())
    try:
        config = load_toml("deny.toml")
    except (OSError, tomllib.TOMLDecodeError) as e:
        found.add("deny.toml", "advisory-ignore", f"deny.toml: {e}")
        return
    for problem in advisory_problems(config, today):
        found.add("deny.toml", "advisory-ignore", f"deny.toml: {problem}")


def pin_problems(lock_text, url, names=PINNED):
    """What is wrong with how Cargo.lock pins each named crate from url.

    Cargo writes the rev a manifest asked for into the source's ?rev= query and
    the resolved commit after #, so a short or symbolic rev shows up in the
    query even though the fragment is always the full SHA. cargo-deny checks
    neither."""
    packages = load_lock(lock_text)
    problems = []
    for name in names:
        found = [p for p in packages if p.get("name") == name]
        if len(found) != 1:
            problems.append(f"{name}: {len(found)} entries in Cargo.lock, want 1")
            continue
        source = found[0].get("source", "")
        match = re.fullmatch(r"git\+(.+)\?rev=([^#]+)#([0-9a-f]+)", source)
        if not match:
            problems.append(f"{name}: source {source!r} is not git pinned with rev")
            continue
        where, rev, commit = match.groups()
        if where != url:
            problems.append(f"{name}: comes from {where}, not {url}")
        if not FULL_SHA.fullmatch(rev):
            problems.append(f"{name}: rev = {rev!r} is not a full 40-character commit SHA")
        elif rev != commit:
            problems.append(f"{name}: rev {rev} resolved to {commit}")
    return problems


def load_lock(lock_text):
    return tomllib.loads(lock_text).get("package", [])


CONSUMER_MAIN = """#![forbid(unsafe_code)]

fn main() {
    panic_exit::exit_on_panic();
    vigil_log::say!("pinned");
    if std::env::args_os().nth(1).is_some() {
        panic!("asked to");
    }
}
"""


def gate_pin(found, args):
    rev = args.rev or subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
    url = args.url or f"file://{os.getcwd()}"
    deps = "".join(f'{name} = {{ git = "{url}", rev = "{rev}" }}\n' for name in PINNED)
    env = dict(os.environ, CARGO_NET_GIT_FETCH_WITH_CLI="true")
    with tempfile.TemporaryDirectory(prefix="pin-consumer-") as consumer:
        os.mkdir(f"{consumer}/src")
        with open(f"{consumer}/Cargo.toml", "w", encoding="utf-8") as f:
            f.write(f'[package]\nname = "pin-consumer"\nversion = "0.0.0"\nedition = "2024"\npublish = false\n\n[workspace]\n\n[dependencies]\n{deps}')
        with open(f"{consumer}/src/main.rs", "w", encoding="utf-8") as f:
            f.write(CONSUMER_MAIN)
        build = subprocess.run(["cargo", "build", "--quiet", "--message-format=json"], cwd=consumer, env=env, capture_output=True, text=True)
        if build.returncode != 0:
            found.add("pin", "pin", f"a consumer pinned to {rev} did not build: {build.stderr.strip()[-800:]}")
            return
        with open(f"{consumer}/Cargo.lock", encoding="utf-8") as f:
            for problem in pin_problems(f.read(), url):
                found.add("pin", "pin", f"pin: {problem}")
        binary = next(m["executable"] for m in map(json.loads, build.stdout.splitlines())
                      if m.get("reason") == "compiler-artifact" and m.get("executable"))
        calm = subprocess.run([binary], capture_output=True, text=True, timeout=30)
        if (calm.returncode, calm.stderr) != (0, "pinned\n"):
            found.add("pin", "pin", f"pin: the consumer exited {calm.returncode} with {calm.stderr!r}, want 0 and 'pinned'")
        hurt = subprocess.run([binary, "panic"], capture_output=True, text=True, timeout=30)
        if hurt.returncode != 101 or not re.fullmatch(r"pinned\npanic at src/main\.rs:\d+; exiting\n", hurt.stderr):
            found.add("pin", "pin", f"pin: the panicking consumer exited {hurt.returncode} with {hurt.stderr!r}, want 101 and its place")
    print(f"pin: vigil-log and panic-exit from {url} at rev {rev}")


def parse_baseline(text):
    entries = {}
    for line in text.splitlines():
        gate, path, rule, count = line.split("\t")
        entries[gate, path, rule] = int(count)
    return entries


GATES = {"fence": gate_fence, "advisories": gate_advisories, "canary": gate_canary, "pin": gate_pin}


def judge(gate, found, baseline, args):
    allowed = {key[1:]: count for key, count in baseline.items() if key[0] == gate and key[2] in COUNTED}
    failures = 0
    for key in sorted(set(found.counts) | set(allowed)):
        have, limit = found.counts[key], allowed.get(key, 0)
        if have == limit:
            continue
        counted = key[1] in COUNTED
        if args.update and counted and (have < limit or args.allow_growth):
            baseline[gate, *key] = have
            continue
        failures += 1
        if have > limit:
            print("\n".join(found.details[key]))
            if counted:
                print(f"FAIL {gate}: {key[0]} {key[1]}: {have} found, the baseline allows {limit}")
            else:
                print(f"FAIL {gate}: {key[0]} {key[1]}: never allowed")
        else:
            print(f"FAIL {gate}: {key[0]} {key[1]}: the baseline allows {limit} but only {have} remain; lower it with --update")
    if not failures:
        print(f"ok   {gate}")
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("gates", nargs="+", choices=[*GATES, "all"])
    parser.add_argument("--root", default=os.path.dirname(os.path.dirname(os.path.abspath(__file__))), help="checkout to judge; a base-branch copy of this script judges a pull request this way")
    parser.add_argument("--update", action="store_true", help="lower the baseline to what is found now")
    parser.add_argument("--allow-growth", action="store_true", help="with --update, also record growth, which the pull request must justify")
    parser.add_argument("--url", help="pin: the git URL the consumer takes the crates from (default: this checkout as file://)")
    parser.add_argument("--rev", help="pin: the commit the consumer pins (default: HEAD)")
    args = parser.parse_args()
    os.chdir(args.root)
    baseline = parse_baseline(open(BASELINE, encoding="utf-8").read()) if os.path.exists(BASELINE) else {}
    failures = 0
    for gate in GATES if "all" in args.gates else args.gates:
        found = Findings()
        GATES[gate](found, args)
        failures += judge(gate, found, baseline, args)
    if args.update and not failures:
        rows = sorted((*key, count) for key, count in baseline.items() if count)
        with open(BASELINE, "w", encoding="utf-8") as f:
            f.writelines("\t".join(map(str, row)) + "\n" for row in rows)
        print(f"wrote {BASELINE}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
