"""Matched optimized-binary time/RSS measurements and byte-exact CLI parity.

Build both binaries beforehand into separate target directories and keep them
linked throughout the run. Example (from the repository root):
  python3 tools/dev/pira_write_check/benchmark.py --baseline BASELINE \
      --candidate CANDIDATE --work-dir SCRATCH/new-benchmark

Requires Python's standard library and /usr/bin/time on macOS or Linux. Fixture
sizes are bounded at 10 MiB, with a separate 16 KiB dense single-line case.
Timing writes to /dev/null; equivalence uses temporary disk files, not RAM.
Results retain every trial, binary/input/output hashes, and platform metadata.
The work directory must not exist; it is created privately and retained.
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import statistics
import signal
import subprocess
import tempfile
import time


def digest_file(path: Path) -> str:
    with path.open("rb") as source:
        return digest_stream(source)


def digest_stream(source) -> str:
    digest = hashlib.sha256()
    for chunk in iter(lambda: source.read(1024 * 1024), b""):
        digest.update(chunk)
    return digest.hexdigest()


def run_bounded(command: list[str], stdout, timeout: float) -> subprocess.CompletedProcess:
    if not all(hasattr(os, name) for name in ("waitid", "P_PID", "WEXITED", "WNOHANG", "WNOWAIT")):
        raise RuntimeError("Safe process-group cleanup requires waitid(WNOWAIT) on macOS/Linux")
    # No Popen context manager: its exceptional exit can wait without a bound.
    process = subprocess.Popen(command, stdout=stdout, stderr=subprocess.PIPE,
                               start_new_session=True)
    try:
        _, stderr = process.communicate(timeout=timeout)
        return subprocess.CompletedProcess(command, process.returncode, stderr=stderr)
    except BaseException:
        # returncode can still be None if an interrupt raced with reaping.
        # WNOWAIT proves child ownership without releasing the PID/group ID.
        if process.returncode is None:
            with contextlib.suppress(BaseException):
                os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
                os.killpg(process.pid, signal.SIGKILL)
        # Cleanup errors (including a second interrupt) must not replace the cause.
        with contextlib.suppress(BaseException):
            process.wait(timeout=1)
        raise
    finally:
        with contextlib.suppress(BaseException):
            process.stderr.close()


def signature(binary: Path, args: list[str], timeout: float, directory: Path) -> dict:
    with tempfile.TemporaryFile(dir=directory) as output:
        result = run_bounded([str(binary), *args], output, timeout)
        size = output.tell()
        output.seek(0)
        return {
            "exit": result.returncode,
            "stderr": result.stderr.decode("utf-8", errors="backslashreplace"),
            "stdout_bytes": size,
            "stdout_sha256": digest_stream(output),
        }


def equivalent(binaries: list[Path], args: list[str], timeout: float, directory: Path) -> dict:
    before, after = [signature(binary, args, timeout, directory) for binary in binaries]
    if before != after:
        raise RuntimeError(f"CLI mismatch for {args}: {before} != {after}")
    return before


def measure(binary: Path, args: list[str], timeout: float, max_rss_mib: float) -> dict:
    mac = platform.system() == "Darwin"
    start = time.perf_counter()
    result = run_bounded(
        ["/usr/bin/time", "-l" if mac else "-v", str(binary), *args],
        subprocess.DEVNULL, timeout,
    )
    result.check_returncode()
    seconds = time.perf_counter() - start
    stats = result.stderr.decode()
    pattern = (r"(\d+)\s+maximum resident set size" if mac else
               r"Maximum resident set size \(kbytes\):\s*(\d+)")
    match = re.search(pattern, stats)
    if match is None:
        raise RuntimeError(f"Cannot parse peak RSS: {stats}")
    rss_bytes = int(match[1]) * (1 if mac else 1024)
    if rss_bytes > max_rss_mib * 1024 * 1024:
        raise RuntimeError(f"Peak RSS {rss_bytes} exceeds {max_rss_mib} MiB budget")
    return {"seconds": seconds, "peak_rss_bytes": rss_bytes}


def fixtures(directory: Path) -> list[tuple[str, Path]]:
    neutral = b"The sensor records six distinct pulses.".ljust(95) + b"\n"
    dense = b"No exception, but it may fail.".ljust(95) + b"\n"
    cases = []
    for name, unit in [("sparse", dense + neutral * 1023), ("dense", dense)]:
        for size in (100 * 1024, 1024 * 1024, 10 * 1024 * 1024):
            path = directory / f"{name}-{size}.txt"
            data = (unit * ((size + len(unit) - 1) // len(unit)))[:size]
            # A marker at EOF also checks that no benchmark scan stops early.
            path.write_bytes(data[:-10] + b"\nsnapshot\n")
            cases.append((name, path))
    path = directory / "long-line-16384.txt"
    path.write_bytes(b"not " * 4096)
    cases.append(("long-line", path))
    return cases


def parity_probes(directory: Path) -> list[list[str]]:
    source = directory / "unicode-probe.txt"
    separators = ["\n", "\r", "\r\n", "\v", "\f", "\x1c", "\x1d", "\x1e", "\x85", "\u2028", "\u2029"]
    source.write_text("😀e\u0301\tNOT necessarily; İS lıKely ſlightly. "
                      "πnot not² _not not_ not\u0301 \u0301not\n" +
                      "".join(f"delves\tinto snapshot{sep}" for sep in separators), encoding="utf-8")
    custom = directory / "custom.json"
    custom.write_text(json.dumps({"z": ["go go", "a[b]c", "x-y"], "a": ["go", "x", "go go"]}))
    literals = directory / "literal.txt"
    literals.write_text("😀 go\tgo go go x-y A[B]C abc\u2028go\u00a0go")
    bad = directory / "invalid-utf8.txt"
    bad.write_bytes(b"\xff")
    lex = directory / "invalid-lexicon.json"
    lex.write_text('{"x":["not","NOT"]}')
    probes = [[], ["--help"], ["--version"], ["--unknown"]]
    for options in ([], ["--json"]):
        probes.extend([
            [*options, str(source)],
            [*options, "--lexicon", str(custom), str(literals)],
            [*options, str(source), str(directory / "missing"), str(bad), str(source)],
            [*options, "--lexicon", str(lex), str(source)],
        ])
    return probes


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True,
                        help="New private directory; must not already exist.")
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--max-rss-mib", type=float, default=1536,
                        help="Fail after a completed trial exceeds this RSS budget.")
    args = parser.parse_args()
    if platform.system() not in ("Darwin", "Linux"):
        parser.error("RSS measurement requires macOS or Linux /usr/bin/time")
    if not 1 <= args.trials <= 10 or not 0 < args.timeout <= 300 or args.max_rss_mib <= 0:
        parser.error("Require 1..10 trials, 0 < timeout <= 300 seconds, and positive RSS budget")
    binaries = [args.baseline.resolve(strict=True), args.candidate.resolve(strict=True)]
    directory = args.work_dir.absolute()
    try:
        directory.mkdir(mode=0o700, parents=True, exist_ok=False)
    except OSError as error:
        parser.error(f"Cannot create fresh work directory: {error}")
    output = directory / "results.json"
    report = {
        "platform": platform.platform(),
        "binaries": [{"path": str(p), "sha256": digest_file(p)} for p in binaries],
        "trials": args.trials,
        "timeout_seconds": args.timeout,
        "max_rss_mib": args.max_rss_mib,
        "timing": "wall clock including /usr/bin/time wrapper; stdout to /dev/null; alternating order",
        "rows": [],
    }
    for case, path in fixtures(directory):
        for mode, options in [("text", []), ("json", ["--json"])]:
            command = [*options, str(path)]
            parity = equivalent(binaries, command, args.timeout, directory)
            if parity["exit"] != 0 or parity["stderr"]:
                raise RuntimeError(f"Unexpected fixture error: {parity}")
            samples = [[], []]
            for trial in range(args.trials):
                for index in ([0, 1] if trial % 2 == 0 else [1, 0]):
                    samples[index].append(measure(binaries[index], command, args.timeout, args.max_rss_mib))
            row = {"case": case, "mode": mode, "bytes": path.stat().st_size,
                   "input_sha256": digest_file(path), "equivalence": parity}
            for label, runs in zip(("baseline", "candidate"), samples):
                row[label] = {"trials": runs,
                              "median_seconds": statistics.median(r["seconds"] for r in runs),
                              "max_peak_rss_bytes": max(r["peak_rss_bytes"] for r in runs)}
            report["rows"].append(row)
            output.write_text(json.dumps(report, indent=2) + "\n")
            print(f"{case} {mode} {row['bytes']}: equivalent; "
                  f"{row['baseline']['median_seconds']:.4f}s -> "
                  f"{row['candidate']['median_seconds']:.4f}s", flush=True)
    probes = parity_probes(directory)
    report["probe_equivalence"] = [
        {"args": command, **equivalent(binaries, command, args.timeout, directory)}
        for command in probes
    ]
    report["complete"] = True
    output.write_text(json.dumps(report, indent=2) + "\n")
    print(output)


if __name__ == "__main__":
    main()
