#!/usr/bin/env python3
"""Audit pira_codenav symbol coverage and exact identity round trips."""

from __future__ import annotations

import argparse
from collections import Counter
import json
from pathlib import Path
import re
import subprocess


OUTLINE_RE = re.compile(
    r"^\s*(\S+)\s+(\S+)\s+L(\d+):(\d+)-\d+:\d+\s+selector=(pira://\S+)$"
)
SHOW_RE = re.compile(
    r"item=(\S+) kind=(\S+) range=L(\d+):(\d+)-\d+:\d+ bytes=\d+ hash=([0-9a-f]+)"
)
SUPPORTED_SUFFIXES = {
    ".py",
    ".rs",
    ".java",
    ".c",
    ".h",
    ".cc",
    ".cpp",
    ".hpp",
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
    ".ps1",
    ".psm1",
    ".psd1",
    ".php",
    ".phtml",
    ".kt",
    ".kts",
    ".lua",
    ".hcl",
    ".tf",
    ".tfvars",
    ".r",
}
EXPLICIT_LANGUAGE = {
    "synthetic/c_project/include/model.h": "c",
    "synthetic/extensionless_python": "python",
    "synthetic/extensionless_bash": "bash",
}
CURATED = {
    "python": {
        ("synthetic/python_project/package/api.py", "class", "Client"),
        ("synthetic/python_project/package/api.py", "method", "Client.fetch"),
        ("synthetic/python_project/package/api.py", "function", "parse_payload"),
        ("synthetic/python_project/package/models.py", "class", "User"),
        ("synthetic/python_project/package/models.py", "function", "normalize_name"),
    },
    "rust": {
        ("synthetic/rust_project/src/parser.rs", "enum", "ParseError"),
        ("synthetic/rust_project/src/parser.rs", "struct", "Parser"),
        ("synthetic/rust_project/src/parser.rs", "method", "Parser::parse"),
    },
    "java": {
        ("synthetic/java_project/src/com/example/App.java", "class", "App"),
        ("synthetic/java_project/src/com/example/App.java", "method", "App.names"),
    },
    "c": {
        ("synthetic/c_project/include/model.h", "struct", "Model"),
        ("synthetic/c_project/include/model.h", "field", "Model::value"),
        ("synthetic/c_project/src/app.c", "function", "main"),
    },
    "cpp": {
        ("synthetic/cpp_project/include/widget.hpp", "class", "demo::Widget"),
        ("synthetic/cpp_project/include/widget.hpp", "method", "demo::Widget::name"),
    },
    "cuda": {
        ("synthetic/cuda_project/include/kernel.cuh", "struct", "ScaleConfig"),
        ("synthetic/cuda_project/src/kernel.cu", "function", "kernels::scale_kernel"),
        ("synthetic/cuda_project/src/kernel.cu", "function", "kernels::launch_scale"),
        (
            "synthetic/cuda_project/src/macro_kernel.cu",
            "function",
            "annotated_kernel",
        ),
    },
    "bash": {
        ("synthetic/bash_project/app.sh", "function", "main"),
    },
    "go": {
        ("synthetic/go_project/model/user.go", "interface", "Labeler"),
        ("synthetic/go_project/model/user.go", "struct", "User"),
        ("synthetic/go_project/model/user.go", "function", "NewUser"),
        ("synthetic/go_project/model/user.go", "method", "User.Label"),
    },
    "javascript": {
        ("synthetic/javascript_project/lib/model.js", "class", "User"),
        ("synthetic/javascript_project/lib/model.js", "method", "User.label"),
        ("synthetic/javascript_project/lib/model.js", "function", "normalizeName"),
    },
    "typescript": {
        ("synthetic/typescript_project/model.ts", "type", "UserId"),
        ("synthetic/typescript_project/model.ts", "variant", "Status.Disabled"),
        ("synthetic/typescript_project/model.ts", "interface", "User"),
        ("synthetic/typescript_project/model.ts", "method", "Store.add"),
        ("synthetic/typescript_project/view.tsx", "function", "UserName"),
        ("synthetic/typescript_project/app.ts", "function", "register.validate"),
        ("synthetic/typescript_project/app.ts", "function", "register.normalized"),
    },
    "csharp": {
        ("synthetic/csharp_project/Program.cs", "class", "Pira.App.Program"),
        ("synthetic/csharp_project/Program.cs", "method", "Pira.App.Program.Main"),
        ("synthetic/csharp_project/Models/User.cs", "record", "Pira.Models.User"),
        ("synthetic/csharp_project/Models/User.cs", "property", "Pira.Models.User.Label"),
    },
    "powershell": {
        ("synthetic/powershell_project/module.psm1", "enum", "Mode"),
        ("synthetic/powershell_project/module.psm1", "class", "Widget"),
        ("synthetic/powershell_project/module.psm1", "method", "Widget::Label"),
        ("synthetic/powershell_project/module.psm1", "function", "Get-Widget"),
    },
    "php": {
        ("synthetic/php_project/src/Model.php", "trait", "App\\Named"),
        ("synthetic/php_project/src/Model.php", "interface", "App\\Labelled"),
        ("synthetic/php_project/src/Model.php", "enum", "App\\State"),
        ("synthetic/php_project/src/Model.php", "class", "App\\Model"),
        ("synthetic/php_project/src/Model.php", "method", "App\\Model::label"),
        ("synthetic/php_project/src/Model.php", "function", "App\\normalize"),
    },
    "kotlin": {
        ("synthetic/kotlin_project/src/main/kotlin/example/Model.kt", "enum", "State"),
        ("synthetic/kotlin_project/src/main/kotlin/example/Model.kt", "interface", "Labelled"),
        ("synthetic/kotlin_project/src/main/kotlin/example/Model.kt", "class", "Model"),
        ("synthetic/kotlin_project/src/main/kotlin/example/Model.kt", "method", "Model.label"),
        ("synthetic/kotlin_project/src/main/kotlin/example/Model.kt", "type", "ModelFactory"),
    },
    "lua": {
        ("synthetic/lua_project/lib/model.lua", "function", "normalize"),
        ("synthetic/lua_project/lib/model.lua", "function", "M.new"),
        ("synthetic/lua_project/lib/model.lua", "function", "M.version"),
    },
    "hcl": {
        ("synthetic/hcl_project/main.tf", "block", "module.child"),
        ("synthetic/hcl_project/main.tf", "block", "resource.example_widget.main"),
        (
            "synthetic/hcl_project/main.tf",
            "block",
            "resource.example_widget.main.lifecycle",
        ),
        (
            "synthetic/hcl_project/main.tf",
            "attribute",
            "resource.example_widget.main.lifecycle.prevent_destroy",
        ),
    },
    "r": {
        ("synthetic/r_project/helpers.R", "function", "normalize_name"),
        ("synthetic/r_project/helpers.R", "function", "normalize_name.trim"),
        ("synthetic/r_project/helpers.R", "function", "double_value"),
    },
}


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pira", type=Path, required=True)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    return parser.parse_args()


def run(command: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def language_prefix(relative: str) -> list[str]:
    language = EXPLICIT_LANGUAGE.get(relative)
    return [language] if language else []


def main() -> int:
    args = arguments()
    binary = args.pira.resolve()
    data = args.data.resolve()
    if not binary.is_file() or not data.is_dir():
        raise SystemExit("--pira must be a file and --data must be a directory")

    files = sorted(
        path.relative_to(data).as_posix()
        for path in data.rglob("*")
        if path.is_file()
        and path.name != "ignored_generated.py"
        and (path.suffix.lower() in SUPPORTED_SUFFIXES or path.relative_to(data).as_posix() in EXPLICIT_LANGUAGE)
    )
    records: list[tuple[str, str, str, int, int, str]] = []
    failures: list[dict[str, object]] = []
    partial = 0
    recovered = 0
    language_counts: Counter[str] = Counter()

    for relative in files:
        result = run(
            [
                str(binary),
                *language_prefix(relative),
                "outline",
                relative,
                "--selectors",
                "--max-items",
                "10000",
            ],
            data,
        )
        if result.returncode:
            failures.append(
                {"operation": "outline", "target": relative, "stderr": result.stderr}
            )
            continue
        header = result.stdout.splitlines()[0]
        if "parse=partial" in header:
            partial += 1
        elif "parse=recovered" in header:
            recovered += 1
        language = re.search(r"\blanguage=(\S+)", header)
        if language:
            language_counts[language.group(1)] += 1
        for line in result.stdout.splitlines()[1:]:
            match = OUTLINE_RE.match(line)
            if match:
                records.append(
                    (
                        relative,
                        match.group(1),
                        match.group(2),
                        int(match.group(3)),
                        int(match.group(4)),
                        match.group(5),
                    )
                )
            elif line.strip():
                failures.append(
                    {"operation": "parse-outline", "target": relative, "line": line}
                )

    emitted = {(path, kind, name) for path, kind, name, *_ in records}
    curated_results: dict[str, dict[str, int]] = {}
    for language, expected in CURATED.items():
        found = len(expected & emitted)
        curated_results[language] = {"found": found, "expected": len(expected)}
        for missing in sorted(expected - emitted):
            failures.append(
                {"operation": "curated-recall", "language": language, "target": missing}
            )

    location_ok = 0
    selector_ok = 0
    for relative, kind, name, line, column, selector in records:
        expected_hash = selector.rsplit("@", 1)[1]
        targets = {
            "location": f"{relative}:{line}:{column}",
            "selector": selector,
        }
        for mode, target in targets.items():
            prefix = language_prefix(relative) if mode == "location" else []
            result = run([str(binary), *prefix, "show", target], data)
            header = SHOW_RE.search(result.stdout)
            valid = (
                result.returncode == 0
                and header is not None
                and header.group(1) == name
                and header.group(2) == kind
                and int(header.group(3)) == line
                and int(header.group(4)) == column
                and header.group(5) == expected_hash
            )
            if valid:
                if mode == "location":
                    location_ok += 1
                else:
                    selector_ok += 1
            else:
                failures.append(
                    {
                        "operation": mode,
                        "target": target,
                        "returncode": result.returncode,
                        "output": (result.stdout + result.stderr)[:500],
                    }
                )

    report = {
        "schema": 1,
        "files": len(files),
        "files_by_language": dict(sorted(language_counts.items())),
        "partial_files": partial,
        "recovered_files": recovered,
        "targets": len(records),
        "location_round_trips": location_ok,
        "selector_round_trips": selector_ok,
        "curated_recall": curated_results,
        "failure_count": len(failures),
        "failures": failures[:20],
    }
    rendered = json.dumps(report, indent=2) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
