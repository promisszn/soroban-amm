#!/usr/bin/env python3
"""Run curated, one-at-a-time guard mutations against a Rust crate's unit tests.

The check first runs the crate's unmutated library tests (the baseline). If the
baseline does not build or fails, the check stops with an error instead of
reporting every mutation as skipped. It then applies one curated mutation at a
time to the crate's production sources (every module reachable from
``src/lib.rs`` that is not gated by ``#[cfg(test)]``), reruns the tests, and
restores the file afterwards, including on timeout and Ctrl-C.
"""

import argparse
import bisect
import hashlib
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time
import tomllib
from dataclasses import dataclass


OPERATORS = ("auth", "error", "comparison")
PRIVILEGED_AUTH = (
    "admin",
    "current_admin",
    "new_admin",
    "governance",
    "keeper",
    "stored_admin",
)
ERROR_VARIANTS = (
    "DeadlineExceeded",
    "DeadlineExpired",
    "SlippageExceeded",
    "Unauthorized",
    "Paused",
    "ZeroAmount",
)
COMPARISON_FLIPS = {
    "<": ">=",
    "<=": ">",
    ">": "<=",
    ">=": "<",
    "==": "!=",
    "!=": "==",
}

# A comparison operator surrounded by whitespace on both sides. rustfmt (which
# CI enforces) always spaces binary comparisons, while `->`, `=>`, `<<`, `>>=`
# and generic brackets such as `Result<i128, ()>` or `Vec<u8>` never have
# whitespace on both sides of the bracket, so they are not matched.
COMPARISON_PATTERN = re.compile(r"(?<=\s)(?:<=|>=|==|!=|<|>)(?=\s)")
# Lines eligible for comparison flips. The keywords must start an identifier
# segment (`min_out`, `amount_min_out`, `order.deadline`) and are case
# sensitive, so `admin_`, `DeadlineExceeded` and `MIN_LIQUIDITY` do not count.
GUARD_LINE_PATTERN = re.compile(r"(?<![A-Za-z0-9])(?:deadline|min_|expires_at)")

STATUSES = ("killed", "survived", "timed out", "skipped (compile error)")


class BaselineError(RuntimeError):
    """The unmutated crate does not build or its tests fail."""


@dataclass(frozen=True)
class Mutation:
    identifier: str
    operator: str
    path: str
    function: str
    line: int
    original: str
    mutated: str
    start: int
    end: int

    def apply(self, source):
        return source[: self.start] + self.mutated + source[self.end :]

    @property
    def location(self):
        return f"{self.path}:{self.line}"


@dataclass(frozen=True)
class Result:
    mutation: Mutation
    status: str


# ── Source scanning ──────────────────────────────────────────────────────────

_RAW_STRING = re.compile(r'b?r(#*)"')


def _noncode_spans(source):
    """Return sorted (start, end) spans of comments and string/char literals."""
    spans = []
    i = 0
    length = len(source)
    while i < length:
        char = source[i]
        nxt = source[i + 1] if i + 1 < length else ""
        if char == "/" and nxt == "/":
            end = source.find("\n", i)
            end = length if end == -1 else end
            spans.append((i, end))
            i = end
        elif char == "/" and nxt == "*":
            depth = 1
            j = i + 2
            while j < length and depth:
                if source.startswith("/*", j):
                    depth += 1
                    j += 2
                elif source.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            spans.append((i, j))
            i = j
        elif char in "rb" and (i == 0 or not (source[i - 1].isalnum() or source[i - 1] == "_")):
            raw = _RAW_STRING.match(source, i)
            if raw:
                terminator = '"' + raw.group(1)
                end = source.find(terminator, raw.end())
                end = length if end == -1 else end + len(terminator)
                spans.append((i, end))
                i = end
            else:
                # `b"..."` / `b'.'` are handled when the quote is reached.
                i += 1
        elif char == '"':
            j = i + 1
            while j < length and source[j] != '"':
                j += 2 if source[j] == "\\" else 1
            spans.append((i, j + 1))
            i = j + 1
        elif char == "'":
            if nxt == "\\":
                end = source.find("'", i + 2)
                end = length if end == -1 else end + 1
                spans.append((i, end))
                i = end
            elif i + 2 < length and source[i + 2] == "'":
                spans.append((i, i + 3))
                i += 3
            else:
                i += 1  # a lifetime such as `'a`
        else:
            i += 1
    return spans


class _Spans:
    """Lookup over possibly overlapping (start, end) spans."""

    def __init__(self, spans):
        merged = []
        for start, end in sorted(spans):
            if merged and start <= merged[-1][1]:
                merged[-1] = (merged[-1][0], max(merged[-1][1], end))
            else:
                merged.append((start, end))
        self.spans = merged
        self.starts = [start for start, _ in merged]

    def end_of(self, position):
        """Return the end of the span containing ``position``, else None."""
        index = bisect.bisect_right(self.starts, position) - 1
        if index >= 0 and self.spans[index][0] <= position < self.spans[index][1]:
            return self.spans[index][1]
        return None

    def contains(self, position):
        return self.end_of(position) is not None


_STATEMENT_ITEMS = ("use", "const", "static", "type", "let", "extern")
_ITEM_KEYWORD = re.compile(
    r"\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:unsafe\s+|async\s+)*([A-Za-z_]\w*)"
)


def _balanced_end(source, position, noncode, closer):
    """Scan from ``position`` to the end of an item or attribute.

    Ends at a top-level `;`, or at a ``closer`` bracket (`}` for items, `]`
    for attributes, None for statement-like items) that returns to the top
    level.
    """
    depth = 0
    i = position
    length = len(source)
    while i < length:
        skip = noncode.end_of(i)
        if skip is not None:
            i = skip
            continue
        char = source[i]
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
            if depth == 0 and char == closer:
                return i + 1
        elif char == ";" and depth == 0:
            return i + 1
        i += 1
    return length


def cfg_test_spans(source, noncode=None):
    """Return spans of items gated by an exact `#[cfg(test)]` attribute.

    `#[cfg(test)] mod guard_tests;` excludes only that declaration, while
    `#[cfg(test)] mod tests { ... }` (or a gated fn/impl/use) excludes the
    whole item. Code after a gated item is still production code.
    """
    if noncode is None:
        noncode = _Spans(_noncode_spans(source))
    spans = []
    for match in re.finditer(r"#\[\s*cfg\s*\(\s*test\s*\)\s*\]", source):
        if noncode.contains(match.start()):
            continue
        position = match.end()
        # Skip further attributes and comments between the cfg and the item.
        while True:
            position = re.compile(r"\s*").match(source, position).end()
            skip = noncode.end_of(position)
            if skip is not None:
                position = skip
                continue
            if source.startswith("#[", position):
                position = _balanced_end(source, position + 1, noncode, closer="]")
                continue
            break
        keyword = _ITEM_KEYWORD.match(source, position)
        statement = bool(keyword) and keyword.group(1) in _STATEMENT_ITEMS
        closer = None if statement else "}"
        spans.append((match.start(), _balanced_end(source, position, noncode, closer)))
    return spans


def production_source(source):
    """Return the source with `#[cfg(test)]` items blanked out.

    Offsets are preserved (gated text is replaced by spaces, newlines kept),
    so positions in the result are valid positions in the original file.
    """
    chars = list(source)
    for start, end in cfg_test_spans(source):
        for index in range(start, end):
            if chars[index] != "\n":
                chars[index] = " "
    return "".join(chars)


def _excluded_spans(source):
    noncode_list = _noncode_spans(source)
    return _Spans(noncode_list + cfg_test_spans(source, _Spans(noncode_list)))


def _normalize(text):
    return " ".join(text.split())


def find_mutations(source, operators=OPERATORS, path="src/lib.rs", crate=""):
    """Return the curated mutations for one source file.

    Mutation ids are content based: a hash of the crate, file, enclosing
    function, the normalized text of the mutated line(s), the operator, the
    original and mutated text, and the occurrence index among otherwise
    identical candidates. Editing other lines (even above it) does not change
    a mutation's id; editing the line itself, renaming its function, or moving
    it to another file does.
    """
    excluded = _excluded_spans(source)
    functions = [
        (match.start(), match.group(1))
        for match in re.finditer(r"\bfn\s+([A-Za-z_]\w*)", source)
        if not excluded.contains(match.start())
    ]
    fn_positions = [position for position, _ in functions]
    candidates = []

    def add(operator, start, end, mutated):
        if not excluded.contains(start):
            candidates.append((start, operator, end, mutated))

    if "auth" in operators:
        privileged_roles = "|".join(PRIVILEGED_AUTH)
        auth_pattern = re.compile(
            r"(?:"
            rf"\b(?:{privileged_roles})\s*\.\s*require_auth\s*\(\s*\)"
            r"|Self\s*::\s*get_keeper\s*\([^;\n]*?\)\s*\?\s*"
            r"\.\s*require_auth\s*\(\s*\)"
            r")\s*;"
        )
        for match in auth_pattern.finditer(source):
            add("auth", match.start(), match.end(), "")

    if "error" in operators:
        variants = "|".join(ERROR_VARIANTS)
        error_pattern = re.compile(
            rf"return\s+Err\s*\(\s*(?:[A-Za-z_]\w*::)?"
            rf"(?:{variants})\s*\)\s*;"
        )
        for match in error_pattern.finditer(source):
            add("error", match.start(), match.end(), "")

    if "comparison" in operators:
        offset = 0
        for raw_line in source.splitlines(keepends=True):
            searchable = raw_line.rstrip("\r\n").split("//", 1)[0]
            if GUARD_LINE_PATTERN.search(searchable):
                for match in COMPARISON_PATTERN.finditer(searchable):
                    add(
                        "comparison",
                        offset + match.start(),
                        offset + match.end(),
                        COMPARISON_FLIPS[match.group()],
                    )
            offset += len(raw_line)

    candidates.sort()
    seen = {}
    found = []
    for start, operator, end, mutated in candidates:
        line_start = source.rfind("\n", 0, start) + 1
        line_end = source.find("\n", end)
        line_end = len(source) if line_end == -1 else line_end
        context = _normalize(source[line_start:line_end])
        fn_index = bisect.bisect_right(fn_positions, start) - 1
        function = functions[fn_index][1] if fn_index >= 0 else "<module>"
        original = source[start:end]
        key = (crate, path, function, context, operator, original, mutated)
        occurrence = seen.get(key, 0)
        seen[key] = occurrence + 1
        digest = hashlib.sha1("\x1f".join((*key, str(occurrence))).encode("utf-8"))
        found.append(
            Mutation(
                identifier=f"{operator}:{function}:{digest.hexdigest()[:10]}",
                operator=operator,
                path=path,
                function=function,
                line=source.count("\n", 0, start) + 1,
                original=original,
                mutated=mutated,
                start=start,
                end=end,
            )
        )
    return found


_MOD_DECLARATION = re.compile(
    r"(?m)^[ \t]*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_]\w*)\s*;"
)


def production_files(src_dir):
    """Return the crate's non-test source files, following `mod x;` from lib.rs.

    Modules declared under `#[cfg(test)]` (and everything below them) are not
    followed, so test-only files are never mutated.
    """
    src_dir = Path(src_dir)
    files = []
    pending = [src_dir / "lib.rs"]
    seen = set()
    while pending:
        path = pending.pop(0)
        if path in seen or not path.is_file():
            continue
        seen.add(path)
        files.append(path)
        source = path.read_text(encoding="utf-8")
        excluded = _excluded_spans(source)
        if path.name in ("lib.rs", "main.rs", "mod.rs"):
            child_dir = path.parent
        else:
            child_dir = path.parent / path.stem
        for match in _MOD_DECLARATION.finditer(source):
            if excluded.contains(match.start(1)):
                continue
            name = match.group(1)
            for candidate in (child_dir / f"{name}.rs", child_dir / name / "mod.rs"):
                if candidate.is_file():
                    pending.append(candidate)
                    break
    return files


# ── Execution ────────────────────────────────────────────────────────────────


def _atomic_write(path, content, mode):
    path = Path(path)
    temporary_path = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb", dir=path.parent, prefix=f".{path.name}.", delete=False
        ) as stream:
            temporary_path = Path(stream.name)
            stream.write(content)
        os.chmod(temporary_path, mode)
        os.replace(temporary_path, path)
    finally:
        if temporary_path is not None and temporary_path.exists():
            temporary_path.unlink()


def _kill_group(process):
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        pass


def run_command(command, cwd, timeout):
    """Run ``command`` in its own process group and kill the group on timeout.

    `subprocess.run(timeout=...)` only kills the direct child (cargo), which
    can leave the test binary running. A new session puts cargo and every
    process it spawns in one group, so the whole tree is killed.
    """
    process = subprocess.Popen(
        command,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        _kill_group(process)
        stdout, stderr = process.communicate()
        raise subprocess.TimeoutExpired(command, timeout, output=stdout, stderr=stderr)
    except BaseException:
        _kill_group(process)
        process.wait()
        raise
    return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)


def _tail(text, lines=40):
    return "\n".join((text or "").rstrip().splitlines()[-lines:])


def load_accepted_survivors(config_path, crate):
    with open(config_path, "rb") as stream:
        config = tomllib.load(stream)
    crate_config = config.get("crates", {}).get(crate, {})
    accepted = {}
    for entry in crate_config.get("accepted_survivors", []):
        identifier = entry["id"]
        justification = entry["justification"].strip()
        if not justification:
            raise ValueError(f"{crate}: accepted survivor {identifier!r} has no justification")
        accepted[identifier] = justification
    return accepted


def classify(completed):
    output = (completed.stdout or "") + "\n" + (completed.stderr or "")
    if completed.returncode == 0:
        return "survived"
    if re.search(r"error\[E\d+\]:|could not compile", output, re.IGNORECASE):
        return "skipped (compile error)"
    if re.search(r"test result: FAILED|^test .+ \.\.\. FAILED$", output, re.MULTILINE):
        return "killed"
    raise RuntimeError(
        "cargo test failed without reporting a compile error or failed test:\n" + _tail(output)
    )


class MutationCheck:
    def __init__(
        self,
        root,
        crate,
        timeout=300,
        operators=OPERATORS,
        accepted=None,
        command_runner=run_command,
        log=None,
    ):
        self.root = Path(root)
        self.crate = crate
        self.timeout = timeout
        self.operators = tuple(operators)
        self.accepted = accepted or {}
        self.command_runner = command_runner
        self.src_dir = self.root / "contracts" / crate / "src"
        self.log = log or (lambda message: None)

    @property
    def command(self):
        return ["cargo", "test", "-p", self.crate, "--lib"]

    def _run_tests(self):
        return self.command_runner(self.command, cwd=self.root, timeout=self.timeout)

    def discover(self):
        mutations = []
        for path in production_files(self.src_dir):
            source = path.read_text(encoding="utf-8")
            relative = path.relative_to(self.root).as_posix()
            mutations.extend(find_mutations(source, self.operators, relative, self.crate))
        return mutations

    def baseline(self):
        """Run the unmutated tests; raise BaselineError unless they pass."""
        command = " ".join(self.command)
        started = time.monotonic()
        try:
            completed = self._run_tests()
        except subprocess.TimeoutExpired as error:
            raise BaselineError(
                f"baseline `{command}` timed out after {self.timeout}s on the unmutated "
                "source; raise --timeout or fix the test suite before checking mutations"
            ) from error
        if completed.returncode != 0:
            output = (completed.stdout or "") + "\n" + (completed.stderr or "")
            raise BaselineError(
                f"baseline `{command}` failed on the unmutated source (exit "
                f"{completed.returncode}). The crate must build and its tests must pass "
                "before mutation results mean anything; contract tests also need the "
                "WASM fixtures from `bash scripts/build_workspace.sh`.\n" + _tail(output)
            )
        return time.monotonic() - started

    def run(self, mutations=None):
        if mutations is None:
            mutations = self.discover()
        duration = self.baseline()
        self.log(f"baseline passed in {duration:.0f}s; running {len(mutations)} mutation(s)")
        results = []
        originals = {}
        for index, mutation in enumerate(mutations, 1):
            path = self.root / mutation.path
            if mutation.path not in originals:
                originals[mutation.path] = (path.read_bytes(), path.stat().st_mode)
            original_bytes, mode = originals[mutation.path]
            mutated_source = mutation.apply(original_bytes.decode("utf-8"))
            try:
                _atomic_write(path, mutated_source.encode("utf-8"), mode)
                try:
                    completed = self._run_tests()
                except subprocess.TimeoutExpired:
                    status = "timed out"
                else:
                    status = classify(completed)
            finally:
                _atomic_write(path, original_bytes, mode)
            self.log(f"[{index}/{len(mutations)}] {status}: {mutation.location} {mutation.identifier}")
            results.append(Result(mutation, status))
        return results


# ── Reporting ────────────────────────────────────────────────────────────────


def format_report(crate, results, accepted, discovered=None):
    """Return (report text, unaccepted survivor count, viable mutation count)."""
    lines = [f"Mutation check: {crate}"]
    counts = {status: 0 for status in STATUSES}
    unaccepted = []
    for result in results:
        counts[result.status] += 1
        if result.status in ("survived", "timed out"):
            mutation = result.mutation
            tag = f"[{mutation.operator}; fn {mutation.function}; id={mutation.identifier}]"
            if mutation.identifier in accepted:
                lines.append(
                    f"ACCEPTED {result.status}: {mutation.location} {tag} "
                    f"({accepted[mutation.identifier]})"
                )
            else:
                unaccepted.append(result)
                lines.append(f"UNACCEPTED {result.status}: {mutation.location} {tag}")
            lines.append(f"  original: {mutation.original.strip()}")
            lines.append(f"  mutated:  {mutation.mutated.strip() or '(removed)'}")

    if discovered is not None:
        known = {mutation.identifier for mutation in discovered}
        for identifier in sorted(set(accepted) - known):
            lines.append(f"STALE accepted survivor (matches no current mutation): {identifier}")

    compile_errors = counts["skipped (compile error)"]
    viable = len(results) - compile_errors
    lines.append(
        "Summary: " + ", ".join(f"{status}={count}" for status, count in counts.items())
    )
    lines.append(f"Mutations: run={len(results)}, viable={viable}, compile_errors={compile_errors}")
    lines.append(f"Unaccepted survivors: {len(unaccepted)}")
    return "\n".join(lines) + "\n", len(unaccepted), viable


def select(mutations, shard=None, limit=None):
    """Pick shard K of N (round-robin, so each shard samples every file) and cap it."""
    if shard is not None:
        index, total = shard
        mutations = [m for position, m in enumerate(mutations) if position % total == index - 1]
    if limit is not None:
        mutations = mutations[:limit]
    return mutations


def _shard(value):
    match = re.fullmatch(r"(\d+)/(\d+)", value)
    if not match or not 1 <= int(match.group(1)) <= int(match.group(2)):
        raise argparse.ArgumentTypeError("expected K/N with 1 <= K <= N, e.g. 1/3")
    return int(match.group(1)), int(match.group(2))


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--crate", required=True, help="Cargo package name under contracts/")
    parser.add_argument(
        "--operators",
        nargs="+",
        choices=OPERATORS,
        default=list(OPERATORS),
        help="mutation operators to run (default: all)",
    )
    parser.add_argument("--timeout", type=int, default=300, help="per-run test timeout in seconds")
    parser.add_argument(
        "--config",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "mutations.toml",
        help="accepted-survivor TOML file",
    )
    parser.add_argument("--report", type=Path, help="write the text report to this file")
    parser.add_argument(
        "--list",
        action="store_true",
        help="print each mutation's location and id without running cargo",
    )
    parser.add_argument(
        "--shard",
        type=_shard,
        help="run only shard K of N (round-robin over the discovered mutations), e.g. 2/3",
    )
    parser.add_argument("--limit", type=int, help="run at most this many mutations (smoke runs)")
    return parser.parse_args(argv)


def _write_report(path, report):
    if path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(report, encoding="utf-8")


def _fail(args, message):
    text = f"ERROR: {message}\n"
    _write_report(args.report, f"Mutation check: {args.crate}\n{text}")
    print(text, file=sys.stderr, end="")
    return 2


def main(argv=None, root=None, command_runner=run_command):
    args = parse_args(argv)
    if args.timeout <= 0:
        print("ERROR: --timeout must be positive", file=sys.stderr)
        return 2
    if args.limit is not None and args.limit <= 0:
        print("ERROR: --limit must be positive", file=sys.stderr)
        return 2
    root = Path(root) if root is not None else Path(__file__).resolve().parent.parent
    source_path = root / "contracts" / args.crate / "src" / "lib.rs"
    if not source_path.is_file():
        print(f"ERROR: crate source not found: {source_path}", file=sys.stderr)
        return 2

    try:
        accepted = load_accepted_survivors(args.config, args.crate)
        checker = MutationCheck(
            root,
            args.crate,
            timeout=args.timeout,
            operators=args.operators,
            accepted=accepted,
            command_runner=command_runner,
            log=lambda message: print(message, file=sys.stderr, flush=True),
        )
        discovered = checker.discover()
        if args.list:
            for mutation in discovered:
                print(
                    f"{mutation.location}\t{mutation.identifier}\t"
                    f"{_normalize(mutation.original)} -> {mutation.mutated or '(removed)'}"
                )
            return 0
        if not discovered:
            return _fail(
                args,
                f"no mutations found for {args.crate} with operators "
                f"{', '.join(args.operators)}; the check would prove nothing",
            )
        try:
            results = checker.run(select(discovered, args.shard, args.limit))
        except BaselineError as error:
            return _fail(args, str(error))
        report, unaccepted_count, viable = format_report(
            args.crate, results, accepted, discovered
        )
        no_viable = bool(results) and viable == 0
        if no_viable:
            report += "ERROR: every mutation failed to compile; no viable mutation was tested\n"
        _write_report(args.report, report)
        print(report, end="")
        if unaccepted_count:
            return 1
        return 2 if no_viable else 0
    except KeyboardInterrupt:
        print("Mutation check interrupted; original source restored.", file=sys.stderr)
        return 130
    except (OSError, RuntimeError, ValueError, tomllib.TOMLDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
