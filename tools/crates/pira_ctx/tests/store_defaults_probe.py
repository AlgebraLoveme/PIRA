import os
import pathlib
import subprocess
import sys
import tempfile

binary = sys.argv[1]
with tempfile.TemporaryDirectory(prefix="ctx-store-defaults-") as temporary:
    root = pathlib.Path(temporary).resolve()
    home, data, cache, local = [root / name for name in ("home", "data", "cache", "local")]
    home.mkdir()
    env = dict(os.environ)
    env.pop("PIRA_CTX_STORE_DIR", None)
    env.update(HOME=str(home), XDG_DATA_HOME=str(data), XDG_CACHE_HOME=str(cache), LOCALAPPDATA=str(local))
    if sys.platform == "darwin":
        destination = home / "Library/Application Support/PIRA/ctx"
        legacy = home / "Library/Caches/PIRA/ctx"
    elif os.name == "nt":
        destination = local / "PIRA/ctx"
        legacy = None
    else:
        destination = data / "pira/ctx"
        legacy = cache / "pira/ctx"

    def run(args, settings=env):
        return subprocess.run([binary, *args], env=settings, cwd=root,
                              capture_output=True, timeout=15)

    def capture(extra=(), settings=env):
        result = run(["capture", *extra, "--intent", "Default selection fixture", "--",
                      sys.executable, "-c", "print('stored')"], settings)
        assert result.returncode == 0, result
        return result

    if legacy is not None:
        legacy.mkdir(parents=True)
    result = run(["list"])
    assert result.returncode == 0 and b"legacy default store" not in result.stderr, result
    assert not destination.exists(), "read-only selection must not create store"
    if legacy is not None:
        # A history/watch-only store is still existing data, not just root captures.
        marker = legacy / "watch/state/preserved.json"
        marker.parent.mkdir(parents=True)
        marker.write_bytes(b"preserved legacy fixture")
        result = run(["list"])
        assert result.returncode == 0 and b"legacy default store" in result.stderr, result
        assert str(legacy).encode() in result.stderr and str(destination).encode() in result.stderr, result
        assert b"--store-dir" in result.stderr and b"migration" in result.stderr, result
        assert marker.read_bytes() == b"preserved legacy fixture" and not destination.exists()
        assert b"legacy default store" not in run(["--help"]).stderr
        assert b"legacy default store" not in run(["--version"]).stderr

    if legacy is not None:
        result = capture(["--store-dir", str(legacy)])
        assert b"legacy default store" not in result.stderr
        old_record = next(legacy.glob("*.piractx"))
        old_bytes = old_record.read_bytes()

    result = capture()
    assert list(destination.glob("*.piractx")), destination
    if legacy is not None:
        assert b"legacy default store" in result.stderr, result
        assert marker.read_bytes() == b"preserved legacy fixture"
        assert list(legacy.glob("*.piractx")) == [old_record] and old_record.read_bytes() == old_bytes, "legacy records must remain unchanged"
        # New data does not silently suppress notice of remaining originals.
        assert b"legacy default store" in run(["list"]).stderr
        assert b"legacy default store" not in run(["list", "--store-dir", str(legacy)]).stderr

    explicit_env = dict(env, PIRA_CTX_STORE_DIR=str(root / "env-store"))
    result = capture(settings=explicit_env)
    assert list((root / "env-store").glob("*.piractx")) and b"legacy default store" not in result.stderr
    result = capture(["--store-dir", str(root / "cli-store")], explicit_env)
    assert list((root / "cli-store").glob("*.piractx")) and b"legacy default store" not in result.stderr
    if sys.platform != "darwin" and os.name != "nt":
        # Empty XDG_DATA_HOME uses HOME, not cwd; XDG_CACHE_HOME never selects new storage.
        fallback = dict(env, XDG_DATA_HOME="")
        result = capture(settings=fallback)
        assert list((home / ".local/share/pira/ctx").glob("*.piractx")), result

    if legacy is not None:
        # A filesystem obstruction must disclose uncertain discovery, not hide it.
        saved = root / "saved-legacy"
        legacy.rename(saved)
        legacy.write_bytes(b"not a directory")
        result = run(["list"])
        assert result.returncode == 0 and b"could not be inspected" in result.stderr, result
        assert legacy.read_bytes() == b"not a directory"
