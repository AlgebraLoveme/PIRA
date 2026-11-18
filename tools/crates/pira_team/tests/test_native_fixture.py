"""The shared lock waiter must detect both retained owners and actual release."""
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
import unittest

from native_fixture import wait_owner


class OwnerLockTests(unittest.TestCase):
    def test_held_owner_has_deadline_and_released_owner_is_accepted(self):
        with tempfile.TemporaryDirectory(prefix="team-owner-lock-") as root:
            path = Path(root) / "run.lock"
            path.write_bytes(b"0")
            code = """import os,sys
with open(sys.argv[1], 'r+') as lock:
    if os.name == 'nt':
        import msvcrt
        msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
    else:
        import fcntl
        fcntl.flock(lock, fcntl.LOCK_EX)
    print('locked', flush=True)
    sys.stdin.read(1)
"""
            owner = subprocess.Popen([sys.executable, "-B", "-u", "-c", code, str(path)],
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            ready = queue.Queue()
            reader = threading.Thread(target=lambda: ready.put(owner.stdout.readline()), daemon=True)
            reader.start()
            try:
                self.assertEqual(ready.get(timeout=5), "locked\n")
                started = time.monotonic()
                with self.assertRaisesRegex(TimeoutError, "owner lock did not release") as error:
                    wait_owner(path, timeout=0.05)
                self.assertIn(str(path), str(error.exception))
                self.assertLess(time.monotonic() - started, 2)
                self.assertIsNone(owner.poll())
                stdout, stderr = owner.communicate("release", timeout=5)
                self.assertEqual(owner.returncode, 0, stderr)
                wait_owner(path, timeout=1)
            finally:
                if owner.poll() is None:
                    owner.kill()
                owner.communicate(timeout=5)
                reader.join(timeout=1)


if __name__ == "__main__":
    unittest.main()
