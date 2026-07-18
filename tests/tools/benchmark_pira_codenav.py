#!/usr/bin/env python3
"""Benchmark every pira_codenav subcommand and compare overlapping baselines.

Run the full comparison in an isolated environment after provisioning all
three tools, or use --pira-only for a native pira_codenav measurement.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile
import time


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser()
    result.add_argument("--pira", type=Path, required=True)
    result.add_argument("--ast-outline", type=Path)
    result.add_argument("--grove", type=Path)
    result.add_argument(
        "--pira-only",
        action="store_true",
        help="benchmark only pira_codenav, for example on the native host",
    )
    result.add_argument("--data", type=Path, required=True)
    result.add_argument("--runs", type=int, default=40)
    result.add_argument("--warmups", type=int, default=5)
    result.add_argument(
        "--task",
        action="append",
        dest="tasks",
        help="run only this named task; repeat to select multiple tasks",
    )
    result.add_argument("--output", type=Path)
    return result


def run(command: list[str], cwd: Path) -> tuple[bytes, bytes]:
    completed = subprocess.run(
        command,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {command}\n"
            f"stdout={completed.stdout.decode(errors='replace')}\n"
            f"stderr={completed.stderr.decode(errors='replace')}"
        )
    return completed.stdout, completed.stderr


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, round((len(ordered) - 1) * fraction)))
    return ordered[index]


def peak_rss_kib(command: list[str], cwd: Path) -> int | None:
    if sys.platform != "linux":
        return None
    time_binary = Path("/usr/bin/time")
    if not time_binary.is_file():
        return None
    with tempfile.NamedTemporaryFile() as measurement:
        completed = subprocess.run(
            [str(time_binary), "-f", "%M", "-o", measurement.name, *command],
            cwd=cwd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        if completed.returncode:
            raise RuntimeError(f"RSS command failed ({completed.returncode}): {command}")
        measurement.seek(0)
        return int(measurement.read().strip())


def benchmark(
    command: list[str], cwd: Path, runs: int, warmups: int, expected: bytes
) -> dict[str, object]:
    for _ in range(warmups):
        stdout, _ = run(command, cwd)
        if expected not in stdout:
            raise RuntimeError(f"semantic check failed for {command}: missing {expected!r}")
    samples_ms: list[float] = []
    stdout = b""
    stderr = b""
    for _ in range(runs):
        started = time.perf_counter_ns()
        stdout, stderr = run(command, cwd)
        if expected not in stdout:
            raise RuntimeError(f"semantic check failed for {command}: missing {expected!r}")
        samples_ms.append((time.perf_counter_ns() - started) / 1_000_000)
    return {
        "command": command,
        "runs": runs,
        "median_ms": round(statistics.median(samples_ms), 3),
        "p95_ms": round(percentile(samples_ms, 0.95), 3),
        "min_ms": round(min(samples_ms), 3),
        "stdout_bytes": len(stdout),
        "stderr_bytes": len(stderr),
        "peak_rss_kib": peak_rss_kib(command, cwd),
    }


def main() -> int:
    args = parser().parse_args()
    if args.runs < 3 or args.warmups < 0:
        raise SystemExit("--runs must be at least 3 and --warmups non-negative")
    if not args.pira_only and (args.ast_outline is None or args.grove is None):
        raise SystemExit("--ast-outline and --grove are required unless --pira-only is used")
    tools = {
        "pira_codenav": str(args.pira.resolve()),
        "ast_outline": str(args.ast_outline.resolve()) if args.ast_outline else "",
        "grove": str(args.grove.resolve()) if args.grove else "",
    }
    fake_lsp = Path(__file__).with_name("fake_lsp_server.py").resolve()
    python = Path(sys.executable).resolve()
    if not fake_lsp.is_file():
        raise SystemExit(f"missing semantic benchmark fixture: {fake_lsp}")
    for name, path in tools.items():
        if args.pira_only and name != "pira_codenav":
            continue
        if not Path(path).is_file():
            raise SystemExit(f"missing {name}: {path}")

    tasks: dict[str, dict[str, list[str]]] = {
        "python_outline": {
            "pira_codenav": [tools["pira_codenav"], "outline", "real/python_click/decorators.py"],
            "ast_outline": [tools["ast_outline"], "real/python_click/decorators.py"],
            "grove": [tools["grove"], "outline", "real/python_click/decorators.py"],
        },
        "rust_outline": {
            "pira_codenav": [tools["pira_codenav"], "outline", "real/rust_ripgrep/gitignore.rs"],
            "ast_outline": [tools["ast_outline"], "real/rust_ripgrep/gitignore.rs"],
            "grove": [tools["grove"], "outline", "real/rust_ripgrep/gitignore.rs"],
        },
        "python_show": {
            "pira_codenav": [
                tools["pira_codenav"],
                "show",
                "real/python_click/decorators.py:168",
            ],
            "ast_outline": [
                tools["ast_outline"],
                "show",
                "real/python_click/decorators.py",
                "command",
            ],
            "grove": [
                tools["grove"],
                "source",
                "real/python_click/decorators.py",
                "command",
            ],
        },
        "python_map": {
            "pira_codenav": [tools["pira_codenav"], "python", "map", "synthetic/python_project"],
            "ast_outline": [tools["ast_outline"], "digest", "synthetic/python_project"],
            "grove": [tools["grove"], "map", "synthetic/python_project"],
        },
        "rust_map": {
            "pira_codenav": [tools["pira_codenav"], "rust", "map", "synthetic/rust_project"],
            "ast_outline": [tools["ast_outline"], "digest", "synthetic/rust_project"],
            "grove": [tools["grove"], "map", "synthetic/rust_project"],
        },
        "python_find": {
            "pira_codenav": [
                tools["pira_codenav"],
                "python",
                "find",
                "synthetic/python_project",
                "Client.fetch",
                "--exact",
            ],
        },
        "python_search": {
            "pira_codenav": [
                tools["pira_codenav"],
                "python",
                "search",
                "real/python_click",
                "def command",
                "return decorator",
                "--context",
                "1",
                "--max-items",
                "10",
            ],
        },
        "python_imports": {
            "pira_codenav": [
                tools["pira_codenav"],
                "imports",
                "synthetic/python_project/package/api.py",
            ],
        },
        "python_dependents": {
            "pira_codenav": [
                tools["pira_codenav"],
                "dependents",
                "package/models.py",
                "--root",
                "synthetic/python_project",
            ],
        },
        "python_deps": {
            "pira_codenav": [
                tools["pira_codenav"],
                "deps",
                "package/api.py",
                "--direction",
                "both",
                "--depth",
                "2",
                "--root",
                "synthetic/python_project",
                "--max-items",
                "20",
            ],
        },
        "languages": {
            "pira_codenav": [tools["pira_codenav"], "languages"],
        },
        "python_definition": {
            "pira_codenav": [
                tools["pira_codenav"],
                "definition",
                "real/python_click/decorators.py:168:5",
                "--lsp",
                str(python),
                "--lsp-arg",
                str(fake_lsp),
            ],
        },
        "python_implementation": {
            "pira_codenav": [
                tools["pira_codenav"],
                "implementation",
                "real/python_click/decorators.py:168:5",
                "--lsp",
                str(python),
                "--lsp-arg",
                str(fake_lsp),
            ],
        },
        "python_type_definition": {
            "pira_codenav": [
                tools["pira_codenav"],
                "type-definition",
                "real/python_click/decorators.py:168:5",
                "--lsp",
                str(python),
                "--lsp-arg",
                str(fake_lsp),
            ],
        },
        "python_references": {
            "pira_codenav": [
                tools["pira_codenav"],
                "references",
                "real/python_click/decorators.py:168:5",
                "--max-items",
                "20",
                "--lsp",
                str(python),
                "--lsp-arg",
                str(fake_lsp),
            ],
        },
        "python_hover": {
            "pira_codenav": [
                tools["pira_codenav"],
                "hover",
                "real/python_click/decorators.py:168:5",
                "--lsp",
                str(python),
                "--lsp-arg",
                str(fake_lsp),
            ],
        },
        "python_callers": {
            "pira_codenav": [
                tools["pira_codenav"],
                "callers",
                "real/python_click/decorators.py:168:5",
                "--lsp",
                str(python),
                "--lsp-arg",
                str(fake_lsp),
            ],
        },
        "python_callees": {
            "pira_codenav": [
                tools["pira_codenav"],
                "callees",
                "real/python_click/decorators.py:168:5",
                "--lsp",
                str(python),
                "--lsp-arg",
                str(fake_lsp),
            ],
        },
        "java_outline": {
            "pira_codenav": [tools["pira_codenav"], "outline", "real/java_junit/StringUtils.java"],
            "ast_outline": [tools["ast_outline"], "real/java_junit/StringUtils.java"],
            "grove": [tools["grove"], "outline", "real/java_junit/StringUtils.java"],
        },
        "c_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "synthetic/c_project/src/app.c",
            ],
            "grove": [tools["grove"], "outline", "synthetic/c_project/src/app.c"],
        },
        "cpp_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "synthetic/cpp_project/include/widget.hpp",
            ],
            "ast_outline": [
                tools["ast_outline"],
                "synthetic/cpp_project/include/widget.hpp",
            ],
            "grove": [
                tools["grove"],
                "outline",
                "synthetic/cpp_project/include/widget.hpp",
            ],
        },
        "bash_outline": {
            "pira_codenav": [tools["pira_codenav"], "outline", "real/bash_bats/bats.sh"],
        },
        "cuda_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "synthetic/cuda_project/src/kernel.cu",
            ],
        },
        "go_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "synthetic/go_project/main.go",
            ],
        },
        "javascript_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "synthetic/javascript_project/lib/model.js",
            ],
        },
        "typescript_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "synthetic/typescript_project/app.ts",
            ],
        },
        "csharp_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "synthetic/csharp_project/Program.cs",
            ],
        },
        "powershell_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "real/powershell_powershell/ResxGen.psm1",
            ],
        },
        "php_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "real/php_laravel/Collection.php",
            ],
        },
        "kotlin_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "real/kotlin_coroutines/CoroutineDispatcher.kt",
            ],
        },
        "lua_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "real/lua_neovim/lsp.lua",
            ],
        },
        "hcl_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "real/hcl_terraform/root.tf",
            ],
        },
        "r_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "real/r_dplyr/mutate.R",
            ],
        },
        "ruby_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "real/ruby_rails/base.rb",
            ],
        },
        "swift_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "real/swift_argument_parser/ParsableCommand.swift",
            ],
        },
        "scala_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "real/scala_cats_effect/IO.scala",
            ],
        },
        "dart_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "real/dart_http/base_client.dart",
            ],
        },
        "elixir_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "real/elixir_elixir/access.ex",
            ],
        },
        "julia_outline": {
            "pira_codenav": [
                tools["pira_codenav"],
                "outline",
                "real/julia_http/HTTP.jl",
            ],
        },
    }
    expected = {
        "python_outline": b"command",
        "rust_outline": b"Gitignore",
        "python_show": b"def command",
        "python_map": b"api.py",
        "rust_map": b"parser.rs",
        "python_find": b'name="Client.fetch"',
        "python_search": b"def command",
        "python_imports": b"package/models.py resolution=structural",
        "python_dependents": b"dependent=package/api.py",
        "python_deps": b"edge depth=1 direction=import",
        "languages": b"\nr\n",
        "python_definition": b"location file=real/python_click/decorators.py range=L138:5-138:12",
        "python_implementation": b"location file=real/python_click/decorators.py range=L138:5-138:12",
        "python_type_definition": b"location file=real/python_click/decorators.py range=L138:5-138:12",
        "python_references": b"references target=",
        "python_hover": b"**command**",
        "python_callers": b'name="caller_of_command"',
        "python_callees": b'name="callee_of_command"',
        "java_outline": b"StringUtils",
        "c_outline": b"main",
        "cpp_outline": b"Widget",
        "bash_outline": b"bats_tee",
        "cuda_outline": b"scale_kernel",
        "go_outline": b"function main",
        "javascript_outline": b"normalizeName",
        "typescript_outline": b"register.validate",
        "csharp_outline": b"Program.Main",
        "powershell_outline": b"EventMessage",
        "php_outline": b"Illuminate\\Support\\Collection",
        "kotlin_outline": b"CoroutineDispatcher",
        "lua_outline": b"lsp.start",
        "hcl_outline": b"resource.test_thing.source",
        "r_outline": b"mutate.data.frame",
        "ruby_outline": b"ActiveRecord::Base",
        "swift_outline": b"ParsableCommand",
        "scala_outline": b"class IO",
        "dart_outline": b"BaseClient",
        "elixir_outline": b"module Access",
        "julia_outline": b"module HTTP",
    }

    selected_tasks = list(tasks)
    if args.tasks:
        unknown = sorted(set(args.tasks) - set(tasks))
        if unknown:
            raise SystemExit(f"unknown --task value(s): {', '.join(unknown)}")
        requested = set(args.tasks)
        selected_tasks = [name for name in tasks if name in requested]

    results: dict[str, object] = {
        "schema": 1,
        "data": str(args.data.resolve()),
        "runs": args.runs,
        "warmups": args.warmups,
        "tasks": {},
    }
    cwd = args.data.resolve()
    for task in selected_tasks:
        commands = tasks[task]
        task_results: dict[str, object] = {}
        for tool, command in commands.items():
            if args.pira_only and tool != "pira_codenav":
                continue
            task_results[tool] = benchmark(
                command, cwd, args.runs, args.warmups, expected[task]
            )
        results["tasks"][task] = task_results  # type: ignore[index]

    rendered = json.dumps(results, indent=2) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
