"""Fresh-directory protection; no benchmark children or large fixtures."""
from __future__ import annotations

import contextlib
import importlib.util
import io
import os
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("writing_benchmark", Path(__file__).with_name("benchmark.py"))
benchmark = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benchmark)


class StopBeforeFixtures(Exception):
    pass


class WorkDirectoryTests(unittest.TestCase):
    def invoke(self, work: Path, binary: Path) -> None:
        argv = ["benchmark.py", "--baseline", str(binary), "--candidate", str(binary),
                "--work-dir", str(work)]
        with mock.patch("sys.argv", argv), mock.patch.object(benchmark.platform, "system", return_value="Linux"):
            benchmark.main()

    def assert_rejected(self, work: Path, binary: Path) -> None:
        with mock.patch.object(benchmark, "fixtures", side_effect=AssertionError("must reject before fixtures")) as fixtures, contextlib.redirect_stderr(io.StringIO()) as errors:
            with self.assertRaises(SystemExit) as raised:
                self.invoke(work, binary)
            self.assertEqual(raised.exception.code, 2)
            self.assertIn("Cannot create fresh work directory", errors.getvalue())
            fixtures.assert_not_called()

    def test_existing_directory_preserves_regular_file_and_symlink_sentinels(self):
        with tempfile.TemporaryDirectory(prefix="write-benchmark-") as temporary:
            root = Path(temporary)
            binary = root / "binary"
            binary.write_bytes(b"not executed")
            work = root / "existing"
            work.mkdir()
            regular = work / "dense-102400.txt"
            regular.write_bytes(b"preserve regular")
            victim = root / "victim"
            victim.write_bytes(b"preserve target")
            link = work / "sparse-102400.txt"
            link.symlink_to(victim)
            missing = root / "missing-results-target"
            (work / "results.json").symlink_to(missing)
            links = (link, work / "results.json")
            targets_before = {path: os.readlink(path) for path in links}
            before = [(p.read_bytes(), p.stat().st_mtime_ns) for p in (regular, victim)]
            self.assert_rejected(work, binary)
            self.assertEqual(before, [(p.read_bytes(), p.stat().st_mtime_ns) for p in (regular, victim)])
            self.assertEqual({path: os.readlink(path) for path in links}, targets_before)
            self.assertFalse(missing.exists())

    def test_work_directory_symlinks_including_dangling_are_rejected(self):
        with tempfile.TemporaryDirectory(prefix="write-benchmark-") as temporary:
            root = Path(temporary)
            binary = root / "binary"
            binary.write_bytes(b"not executed")
            for existing in (False, True):
                with self.subTest(existing=existing):
                    target = root / f"target-{existing}"
                    if existing:
                        target.mkdir()
                    work = root / f"link-{existing}"
                    work.symlink_to(target, target_is_directory=True)
                    target_before = os.readlink(work)
                    self.assert_rejected(work, binary)
                    self.assertEqual(os.readlink(work), target_before)
                    self.assertEqual(target.exists(), existing)

    def test_new_work_directory_is_private_and_retained(self):
        with tempfile.TemporaryDirectory(prefix="write-benchmark-") as temporary:
            root = Path(temporary)
            binary = root / "binary"
            binary.write_bytes(b"not executed")
            work = root / "parent" / "new"
            with mock.patch.object(benchmark, "fixtures", side_effect=StopBeforeFixtures) as fixtures:
                with self.assertRaises(StopBeforeFixtures):
                    self.invoke(work, binary)
            fixtures.assert_called_once_with(work.absolute())
            self.assertTrue(work.is_dir())
            self.assertEqual(list(work.iterdir()), [])
            if os.name == "posix":
                self.assertEqual(work.stat().st_mode & 0o077, 0)
            self.assertEqual(binary.read_bytes(), b"not executed")


class CommunicationTests(unittest.TestCase):
    def setUp(self):
        self.process = mock.MagicMock()
        self.process.__enter__.return_value = self.process  # Also safe against the pre-fix helper.
        self.process.pid = 43210
        self.process.returncode = None
        popen_patch = mock.patch.object(benchmark.subprocess, "Popen", return_value=self.process)
        # Exercise the POSIX cleanup contract on every host, without real OS primitives.
        posix = SimpleNamespace(waitid=mock.Mock(return_value=None), killpg=mock.Mock(),
                                P_PID=1, WEXITED=2, WNOHANG=4, WNOWAIT=8)
        os_patch = mock.patch.object(benchmark, "os", posix)
        signal_patch = mock.patch.object(benchmark, "signal", SimpleNamespace(SIGKILL=9))
        os_patch.start()
        self.addCleanup(os_patch.stop)
        signal_patch.start()
        self.addCleanup(signal_patch.stop)
        self.popen = popen_patch.start()
        self.addCleanup(popen_patch.stop)
        self.waitid = posix.waitid
        self.signal = posix.killpg

    def test_communicate_failures_kill_owned_group_and_reap_bounded(self):
        failures = [KeyboardInterrupt(), SystemExit(3), OSError("communicate failed"),
                    benchmark.subprocess.TimeoutExpired(["candidate"], 0.1)]
        for failure in failures:
            with self.subTest(exception=type(failure).__name__):
                self.process.reset_mock()
                self.popen.reset_mock()
                self.signal.reset_mock()
                self.waitid.reset_mock()
                self.process.communicate.side_effect = failure
                with self.assertRaises(type(failure)) as raised:
                    benchmark.run_bounded(["candidate"], benchmark.subprocess.DEVNULL, 0.1)
                self.assertIs(raised.exception, failure)
                self.popen.assert_called_once_with(["candidate"], stdout=benchmark.subprocess.DEVNULL,
                                                  stderr=benchmark.subprocess.PIPE, start_new_session=True)
                self.waitid.assert_called_once_with(benchmark.os.P_PID, self.process.pid,
                                                    benchmark.os.WEXITED | benchmark.os.WNOHANG | benchmark.os.WNOWAIT)
                self.signal.assert_called_once_with(self.process.pid, benchmark.signal.SIGKILL)
                self.process.communicate.assert_called_once_with(timeout=0.1)
                self.process.wait.assert_called_once_with(timeout=1)
                self.process.stderr.close.assert_called_once_with()

    def test_cleanup_failures_preserve_original_exception(self):
        for cleanup_failure in [ProcessLookupError(), PermissionError(), KeyboardInterrupt()]:
            with self.subTest(cleanup=type(cleanup_failure).__name__):
                self.process.reset_mock()
                self.signal.reset_mock()
                failure = OSError("original communicate failure")
                self.process.communicate.side_effect = failure
                self.signal.side_effect = cleanup_failure
                self.process.wait.side_effect = benchmark.subprocess.TimeoutExpired(["candidate"], 1)
                self.process.stderr.close.side_effect = OSError("close failed")
                with self.assertRaises(OSError) as raised:
                    benchmark.run_bounded(["candidate"], benchmark.subprocess.DEVNULL, 0.1)
                self.assertIs(raised.exception, failure)
                self.process.wait.assert_called_once_with(timeout=1)
                self.process.stderr.close.assert_called_once_with()

    def test_reaped_child_never_signals_a_reusable_group_id(self):
        self.process.returncode = 0
        failure = OSError("communication failed after reap")
        self.process.communicate.side_effect = failure
        with self.assertRaises(OSError) as raised:
            benchmark.run_bounded(["candidate"], benchmark.subprocess.DEVNULL, 0.1)
        self.assertIs(raised.exception, failure)
        self.signal.assert_not_called()
        self.process.wait.assert_called_once_with(timeout=1)

    def test_reaped_during_communicate_never_signals_a_reused_pid(self):
        self.process.returncode = None  # Interrupted after reap but before returncode update.
        self.waitid.side_effect = ChildProcessError("no owned child")
        failure = KeyboardInterrupt()
        self.process.communicate.side_effect = failure
        with self.assertRaises(KeyboardInterrupt) as raised:
            benchmark.run_bounded(["candidate"], benchmark.subprocess.DEVNULL, 0.1)
        self.assertIs(raised.exception, failure)
        self.waitid.assert_called_once()
        self.signal.assert_not_called()
        self.process.wait.assert_called_once_with(timeout=1)

    def test_missing_ownership_probe_fails_before_child_creation(self):
        with mock.patch.object(benchmark, "os", object()):
            with self.assertRaisesRegex(RuntimeError, "waitid"):
                benchmark.run_bounded(["candidate"], benchmark.subprocess.DEVNULL, 0.1)
        self.popen.assert_not_called()
        self.signal.assert_not_called()

    def test_successful_communication_preserves_result(self):
        self.process.returncode = 0
        self.process.communicate.return_value = (None, b"metrics")
        result = benchmark.run_bounded(["candidate"], benchmark.subprocess.DEVNULL, 0.1)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stderr, b"metrics")
        self.signal.assert_not_called()
        self.process.wait.assert_not_called()
        self.process.stderr.close.assert_called_once_with()


if __name__ == "__main__":
    unittest.main()
