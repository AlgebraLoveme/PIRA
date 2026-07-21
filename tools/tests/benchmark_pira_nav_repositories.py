#!/usr/bin/env python3
"""Measure pira_nav map behavior on pinned real repositories."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import statistics
import subprocess
import sys
import tempfile
import time


HEADER_FIELD = re.compile(r"\b([a-z_]+)=(\d+)")
FILE_FIELD = re.compile(r'(?:^| )file=("(?:\\.|[^"\\])*")')
SYMBOLS_FIELD = re.compile(r' symbols=("(?:\\.|[^"\\])*")')


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pira", type=Path, required=True)
    parser.add_argument(
        "--repo",
        action="append",
        required=True,
        metavar="NAME=PATH",
        help="repeat for each pinned repository",
    )
    parser.add_argument("--runs", type=int, default=20)
    parser.add_argument("--warmups", type=int, default=3)
    parser.add_argument("--lsp", action="append", default=[])
    parser.add_argument("--lsp-arg", action="append", default=[])
    parser.add_argument("--lsp-root")
    parser.add_argument("--output", type=Path)
    return parser.parse_args()


def run(command: list[str], cwd: Path) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        command,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    index = min(len(ordered) - 1, round((len(ordered) - 1) * fraction))
    return ordered[index]


def peak_rss_kib(command: list[str], cwd: Path) -> int | None:
    if not Path("/usr/bin/time").is_file():
        return None
    if sys.platform == "darwin":
        completed = subprocess.run(
            ["/usr/bin/time", "-l", *command], cwd=cwd,
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False,
        )
        if completed.returncode:
            raise RuntimeError(f"RSS measurement failed: {command}")
        line = next(
            (line for line in completed.stderr.decode(errors="replace").splitlines()
             if "maximum resident set size" in line),
            None,
        )
        return int(line.strip().split()[0]) // 1024 if line else None
    if sys.platform != "linux":
        return None
    with tempfile.NamedTemporaryFile() as measurement:
        completed = subprocess.run(
            ["/usr/bin/time", "-f", "%M", "-o", measurement.name, *command],
            cwd=cwd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        if completed.returncode:
            raise RuntimeError(f"RSS measurement failed: {command}")
        measurement.seek(0)
        return int(measurement.read().strip())


def repository_result(
    binary: Path,
    root: Path,
    runs: int,
    warmups: int,
    lsp: list[str],
    lsp_args: list[str],
    lsp_root: str | None,
) -> dict[str, object]:
    command = [str(binary), "map", "."]
    if not lsp:
        command.append("--native")
    for server in lsp:
        command.extend(("--lsp", server))
    for argument in lsp_args:
        command.extend(("--lsp-arg", argument))
    if lsp_root:
        command.extend(("--lsp-root", lsp_root))
    for _ in range(warmups):
        completed = run(command, root)
        if completed.returncode:
            raise RuntimeError(completed.stderr.decode(errors="replace"))

    samples = []
    completed = None
    for _ in range(runs):
        started = time.perf_counter_ns()
        completed = run(command, root)
        samples.append((time.perf_counter_ns() - started) / 1_000_000)
        if completed.returncode:
            raise RuntimeError(completed.stderr.decode(errors="replace"))
    assert completed is not None

    inventory_command = [*command, "--max-items", "1000000"]
    inventory = run(inventory_command, root)
    if inventory.returncode:
        raise RuntimeError(inventory.stderr.decode(errors="replace"))
    lines = inventory.stdout.decode(errors="replace").splitlines()
    fields = {name: int(value) for name, value in HEADER_FIELD.findall(lines[0])}
    # `--max-items 1000000` emits one quoted `file=` field for every parsed or
    # failed navigable file. Deriving the denominator from those rows exactly
    # matches pira_nav's hidden/ignore/symlink/language discovery policy.
    navigable_paths = set()
    for line in lines[1:]:
        if (line.startswith("file=") or line.startswith("error file=")) and (
            match := FILE_FIELD.search(line)
        ):
            navigable_paths.add(root / json.loads(match.group(1)))
    navigable_bytes = sum(path.stat().st_size for path in navigable_paths)
    output_bytes = len(completed.stdout) + len(completed.stderr)
    expanded_output_bytes = len(inventory.stdout) + len(inventory.stderr)
    file_count = fields.get("source_files", 0) + fields.get("document_files", 0)
    failed_files = fields.get("failed", 0)
    parsed_files = fields.get("parsed", file_count - failed_files)
    lsp_files = fields.get("lsp", 0)
    symbol_files = sum(
        bool(json.loads(match.group(1)))
        for line in lines[1:]
        if (match := SYMBOLS_FIELD.search(line))
    )
    return {
        "commit": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=root, text=True
        ).strip(),
        "eligible_files": fields.get("files", 0),
        "navigable_files": len(navigable_paths),
        "navigable_bytes": navigable_bytes,
        "map_output_bytes": output_bytes,
        "expanded_map_output_bytes": expanded_output_bytes,
        "context_reduction_pct": (
            round((1 - output_bytes / navigable_bytes) * 100, 1)
            if navigable_bytes
            else None
        ),
        "parsed_files": parsed_files,
        "native_files": parsed_files - lsp_files,
        "lsp_files": lsp_files,
        "files_with_symbols": symbol_files,
        "failed_files": failed_files,
        "ambiguous_files": fields.get("ambiguous", 0),
        "median_ms": round(statistics.median(samples), 3),
        "p95_ms": round(percentile(samples, 0.95), 3),
        "peak_rss_kib": peak_rss_kib(command, root),
    }


def main() -> int:
    args = arguments()
    binary = args.pira.resolve()
    if not binary.is_file() or args.runs < 3 or args.warmups < 0:
        raise SystemExit("invalid binary, run count, or warmup count")
    repositories = {}
    for spec in args.repo:
        name, separator, path = spec.partition("=")
        root = Path(path).resolve()
        if not separator or not name or not root.is_dir():
            raise SystemExit(f"invalid --repo {spec!r}; expected NAME=PATH")
        repositories[name] = repository_result(
            binary,
            root,
            args.runs,
            args.warmups,
            args.lsp,
            args.lsp_arg,
            args.lsp_root,
        )
    report = {
        "schema": 1,
        "runs": args.runs,
        "warmups": args.warmups,
        "repositories": repositories,
    }
    rendered = json.dumps(report, indent=2) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
