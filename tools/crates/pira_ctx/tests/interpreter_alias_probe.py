import os
import pathlib
import shlex
import subprocess
import sys
import tempfile

binary = sys.argv[1]
with tempfile.TemporaryDirectory(prefix="ctx-interpreter-alias-") as temporary:
    root = pathlib.Path(temporary).resolve()
    store = root / "store"

    def run(args, env=None):
        return subprocess.run([binary, args[0], "--store-dir", str(store), *args[1:]],
                              cwd=root, env=env, capture_output=True, timeout=15)

    result = run(["capture", "--intent", "Interpreter fixture", "--", sys.executable, "-c", "print('fixture')"])
    assert result.returncode == 0, result
    record = next(store.glob("*.piractx"))
    marker = root / "interpreter-ran"
    sibling = root / "python-\ufffd"
    sibling.write_text("#!/bin/sh\nprintf x >> " + shlex.quote(str(marker)) +
                       "\nexec " + shlex.quote(sys.executable) + ' "$@"\n')
    sibling.chmod(0o700)
    env = dict(os.environb)
    env[b"PIRA_CTX_PYTHON"] = os.fsencode(root) + b"/python-\xff"
    args = ["exec", str(record), "--intent", "Interpreter selection", "--code", "print('ANALYSIS_RAN')"]
    result = run(args, env)
    assert result.returncode == 125 and b"PIRA_CTX_PYTHON" in result.stderr and b"Unicode" in result.stderr, result
    assert not marker.exists(), "lossy sibling executed during probe or analysis"
    # Explicit selection still overrides an invalid environment setting.
    result = run(args + ["--python", sys.executable], env)
    assert result.returncode == 0 and b"ANALYSIS_RAN" in result.stdout, result
    assert not marker.exists()
    # A genuinely configured Unicode replacement character remains a valid path.
    env[b"PIRA_CTX_PYTHON"] = os.fsencode(sibling)
    result = run(args, env)
    assert result.returncode == 0 and b"ANALYSIS_RAN" in result.stdout, result
    assert marker.read_text() == "xx", "expected both version probe and analysis"
