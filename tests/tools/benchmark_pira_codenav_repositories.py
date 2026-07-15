#!/usr/bin/env python3
"""Measure pira_codenav map behavior on pinned real repositories."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import statistics
import subprocess
import tempfile
import time


SUPPORTED_SUFFIXES = {
    ".py",
    ".pyi",
    ".pyw",
    ".rs",
    ".java",
    ".c",
    ".cc",
    ".cpp",
    ".cxx",
    ".hpp",
    ".hh",
    ".hxx",
    ".cu",
    ".cuh",
    ".sh",
    ".bash",
    ".go",
    ".js",
    ".jsx",
    ".mjs",
    ".cjs",
    ".ts",
    ".tsx",
    ".mts",
    ".cts",
    ".cs",
}
HEADER_FIELD = re.compile(r"\b([a-z_]+)=(\d+)")


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
    command = [str(binary), "map", ".", "--max-items", "1000000"]
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

    lines = completed.stdout.decode(errors="replace").splitlines()
    fields = {name: int(value) for name, value in HEADER_FIELD.findall(lines[0])}
    supported_files = [
        path
        for path in root.rglob("*")
        if path.is_file()
        and ".git" not in path.parts
        and path.suffix.lower() in SUPPORTED_SUFFIXES
    ]
    source_bytes = sum(path.stat().st_size for path in supported_files)
    output_bytes = len(completed.stdout) + len(completed.stderr)
    symbol_files = sum(
        bool(line.rsplit(" symbols=", 1)[-1]) for line in lines[1:] if " symbols=" in line
    )
    return {
        "commit": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=root, text=True
        ).strip(),
        "checked_out_files": sum(
            1 for path in root.rglob("*") if path.is_file() and ".git" not in path.parts
        ),
        "supported_source_files": len(supported_files),
        "supported_source_bytes": source_bytes,
        "map_output_bytes": output_bytes,
        "context_reduction_pct": round((1 - output_bytes / source_bytes) * 100, 1),
        "parsed_files": fields.get("parsed", 0),
        "tree_sitter_files": fields.get("tree_sitter", 0),
        "lsp_files": fields.get("lsp", 0),
        "files_with_symbols": symbol_files,
        "failed_files": fields.get("failed", 0),
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
