"""Native test-only synchronization; no polling, secrets or production endpoints."""
import contextlib
import json
import os
from pathlib import Path
import socket


class Boundary:
    def __init__(self):
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen()
        self.listener.settimeout(8)
        self.connection = None

    @property
    def environment(self):
        return {"TEAM_BOUNDARY_ENDPOINT": json.dumps(self.listener.getsockname())}

    def reached(self):
        self.connection, _ = self.listener.accept()
        self.connection.settimeout(12)
        assert self.connection.recv(1) == b"1"

    def release(self):
        self.connection.sendall(b"1")

    def close(self):
        if self.connection:
            self.connection.close()
        self.listener.close()


def process_exit_waiter(pid):
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes
        api = ctypes.WinDLL("kernel32", use_last_error=True)
        api.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        api.OpenProcess.restype = wintypes.HANDLE
        api.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        api.CloseHandle.argtypes = [wintypes.HANDLE]
        handle = api.OpenProcess(0x100000, False, pid)
        if not handle:
            raise ctypes.WinError(ctypes.get_last_error())
        def wait():
            try:
                assert api.WaitForSingleObject(handle, 8000) == 0, "fixture process did not exit"
            finally:
                api.CloseHandle(handle)
        return wait
    import select
    if hasattr(os, "pidfd_open"):
        handle = os.pidfd_open(pid)
        def wait():
            try:
                assert select.select([handle], [], [], 8)[0], "fixture process did not exit"
            finally:
                os.close(handle)
        return wait
    handle = select.kqueue()
    handle.control([select.kevent(pid, filter=select.KQ_FILTER_PROC,
        flags=select.KQ_EV_ADD | select.KQ_EV_ONESHOT, fflags=select.KQ_NOTE_EXIT)], 0, 0)
    def wait():
        try:
            assert len(handle.control(None, 1, 8)) == 1, "fixture process did not exit"
        finally:
            handle.close()
    return wait


def wait_owner(path):
    with Path(path).open("r+") as lock:
        if os.name != "nt":
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX)
            return
        import ctypes
        import msvcrt
        from ctypes import wintypes
        class Overlapped(ctypes.Structure):
            _fields_ = [("internal", ctypes.c_size_t), ("internal_high", ctypes.c_size_t),
                        ("offset", wintypes.DWORD), ("offset_high", wintypes.DWORD),
                        ("event", wintypes.HANDLE)]
        api = ctypes.WinDLL("kernel32", use_last_error=True)
        api.LockFileEx.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.DWORD,
                                  wintypes.DWORD, wintypes.DWORD, ctypes.POINTER(Overlapped)]
        overlapped = Overlapped()
        if not api.LockFileEx(msvcrt.get_osfhandle(lock.fileno()), 2, 0, 1, 0, ctypes.byref(overlapped)):
            raise ctypes.WinError(ctypes.get_last_error())
        # Closing this test-owned handle releases the lock.


@contextlib.contextmanager
def raw_authentication(endpoint):
    if endpoint["kind"] == "pipe":
        import ctypes
        import msvcrt
        from ctypes import wintypes
        api = ctypes.WinDLL("kernel32", use_last_error=True)
        api.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
            ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
        api.CreateFileW.restype = wintypes.HANDLE
        handle = api.CreateFileW(endpoint["name"], 0xC0000000, 0, None, 3, 0, None)
        if handle == wintypes.HANDLE(-1).value:
            raise ctypes.WinError(ctypes.get_last_error())
        with os.fdopen(msvcrt.open_osfhandle(handle, os.O_RDWR | os.O_BINARY), "r+b", buffering=0) as stream:
            yield stream
    else:
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(8)
            if endpoint["kind"] == "abstract":
                client.connect("\0" + endpoint["name"])
            else:
                # Darwin's per-thread cwd avoids sockaddr_un path limits without
                # changing the fake backend's other command-serving threads.
                import ctypes
                api = ctypes.CDLL(None, use_errno=True)
                directory = os.open(endpoint["root"], os.O_RDONLY)
                try:
                    if api.pthread_fchdir_np(directory) != 0:
                        raise OSError(ctypes.get_errno(), "fixture thread cwd")
                    try:
                        client.connect(endpoint["name"])
                    finally:
                        api.pthread_fchdir_np(-1)
                finally:
                    os.close(directory)
            with client.makefile("rwb", buffering=0) as stream:
                yield stream
