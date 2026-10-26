"""Test-only snapshot reader matching Rust's Windows read/write/delete sharing.

Python's ordinary open omits FILE_SHARE_DELETE. It can both obstruct atomic
publication and fail when a publisher already holds DELETE access. No retries:
missing files, access failures and malformed snapshots must remain test failures.
"""
import os


def open_shared_snapshot(path):
    if os.name != "nt":
        return open(path, "r", encoding="utf-8")
    import ctypes
    import msvcrt
    from ctypes import wintypes

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    create = kernel.CreateFileW
    create.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                       wintypes.LPVOID, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    create.restype = wintypes.HANDLE
    close = kernel.CloseHandle
    close.argtypes = [wintypes.HANDLE]
    close.restype = wintypes.BOOL
    # GENERIC_READ; FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    # OPEN_EXISTING; FILE_ATTRIBUTE_NORMAL. Never create or truncate a snapshot.
    handle = create(os.fsdecode(path), 0x80000000, 7, None, 3, 0x80, None)
    if handle == ctypes.c_void_p(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        descriptor = msvcrt.open_osfhandle(handle, os.O_RDONLY | os.O_BINARY)
    except BaseException:
        close(handle)
        raise
    try:
        return os.fdopen(descriptor, "r", encoding="utf-8")
    except BaseException:
        os.close(descriptor)
        raise


def read_shared_snapshot(path):
    with open_shared_snapshot(path) as stream:
        return stream.read()


def report_live_failure(process, store):
    """Evidence only: called before cleanup; never converts a read failure to success."""
    import sys
    print(f"live observer failure: pid={process.pid} returncode={process.poll()}", file=sys.stderr)
    for directory in (store, store / "live"):
        try:
            print(f"entries at failure {directory}: {[p.name for p in directory.iterdir()]}", file=sys.stderr)
        except OSError as error:
            print(f"cannot inspect {directory}: {error!r}", file=sys.stderr)
