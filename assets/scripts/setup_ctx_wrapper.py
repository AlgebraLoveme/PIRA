"""Read-only ancestry guard for migration through released native Ctx."""
from __future__ import annotations

import ctypes
import os
from pathlib import Path
import sys

RETRY = ("Store migration must run unwrapped. Wait for pira_ctx and other tool runs "
         "to finish, then rerun the same setup command directly without pira_ctx "
         "(for example: python3 assets/scripts/setup_pira.py with the same options). "
         "Do not delete old stores or skip migration.")


class WindowsProcessEntry(ctypes.Structure):
    # Fixed-width Windows ABI, including when deterministic tests run on POSIX.
    _fields_ = [("size", ctypes.c_uint32), ("usage", ctypes.c_uint32),
                ("pid", ctypes.c_uint32), ("heap", ctypes.c_size_t),
                ("module", ctypes.c_uint32), ("threads", ctypes.c_uint32),
                ("parent", ctypes.c_uint32), ("priority", ctypes.c_int32),
                ("flags", ctypes.c_uint32), ("name", ctypes.c_uint16 * 260)]


class FileTime(ctypes.Structure):
    _fields_ = [("low", ctypes.c_uint32), ("high", ctypes.c_uint32)]


def windows_kernel():
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    handle, dword = ctypes.c_void_p, ctypes.c_uint32
    signatures = {
        "OpenProcess": ([dword, ctypes.c_int, dword], handle),
        "CloseHandle": ([handle], ctypes.c_int),
        "WaitForSingleObject": ([handle, dword], dword),
        "GetProcessTimes": ([handle] + [ctypes.POINTER(FileTime)] * 4, ctypes.c_int),
        "QueryFullProcessImageNameW": ([handle, dword, ctypes.c_wchar_p, ctypes.POINTER(dword)], ctypes.c_int),
        "CreateToolhelp32Snapshot": ([dword, dword], handle),
        "Process32FirstW": ([handle, ctypes.POINTER(WindowsProcessEntry)], ctypes.c_int),
        "Process32NextW": ([handle, ctypes.POINTER(WindowsProcessEntry)], ctypes.c_int),
    }
    for name, (arguments, result) in signatures.items():
        function = getattr(kernel, name)
        function.argtypes, function.restype = arguments, result
    return kernel


def windows_parent(kernel, pid: int) -> int:
    snapshot = kernel.CreateToolhelp32Snapshot(2, 0)  # TH32CS_SNAPPROCESS
    if snapshot == ctypes.c_void_p(-1).value or not snapshot:
        raise OSError(ctypes.get_last_error(), "Cannot snapshot process ancestry")
    try:
        entry = WindowsProcessEntry()
        entry.size = ctypes.sizeof(entry)
        found = kernel.Process32FirstW(snapshot, ctypes.byref(entry))
        while found and entry.pid != pid:
            found = kernel.Process32NextW(snapshot, ctypes.byref(entry))
        if not found:
            raise RuntimeError("Ancestor missing from process snapshot")
        if pid == 4 and bytes(entry.name).decode("utf-16-le").split("\0", 1)[0] != "System":
            raise RuntimeError("Unverified Windows System process identity")
        return entry.parent
    finally:
        kernel.CloseHandle(snapshot)


def windows_process(pid: int) -> tuple[int, Path | None, int]:
    kernel = windows_kernel()
    # Windows reserves PID 4 for its non-user System process. OpenProcess is
    # denied there by design; require the kernel snapshot's root link as well.
    # No other access-denied/missing process is a terminal ancestry exemption.
    if pid == 4:
        if windows_parent(kernel, pid) != 0:
            raise RuntimeError("Unverified Windows System process boundary")
        return 0, None, 0
    # Query + synchronize only; never request debug privileges or VM access.
    handle = kernel.OpenProcess(0x1000 | 0x100000, False, pid)
    if not handle:
        raise OSError(ctypes.get_last_error(), "Cannot open ancestor process")

    def alive():
        status = kernel.WaitForSingleObject(handle, 0)
        if status == 0:
            raise RuntimeError("Ancestor process exited during inspection")
        if status != 258:  # WAIT_TIMEOUT is the only proof the process is running.
            raise OSError(ctypes.get_last_error(), "Cannot determine ancestor liveness")

    try:
        alive()
        created, exited, kernel_time, user_time = (FileTime() for _ in range(4))
        if not kernel.GetProcessTimes(handle, ctypes.byref(created), ctypes.byref(exited),
                                      ctypes.byref(kernel_time), ctypes.byref(user_time)):
            raise OSError(ctypes.get_last_error(), "Cannot inspect ancestor creation time")
        image = ctypes.create_unicode_buffer(32768)
        size = ctypes.c_uint32(len(image))
        if not kernel.QueryFullProcessImageNameW(handle, 0, image, ctypes.byref(size)):
            raise OSError(ctypes.get_last_error(), "Cannot inspect ancestor image")
        if not image.value or not 0 < size.value < len(image):
            raise RuntimeError("Invalid ancestor image path")
        parent = windows_parent(kernel, pid)
        alive()
        started = (created.high << 32) | created.low
        if not started:
            raise RuntimeError("Invalid ancestor creation time")
        return parent, Path(image.value), started
    finally:
        kernel.CloseHandle(handle)


def process_parent(pid: int) -> tuple[int, Path | None, int]:
    """Read kernel parent, executable and birth time; never argv or environment."""
    if sys.platform == "win32":
        return windows_process(pid)
    if sys.platform == "linux":
        root = Path(f"/proc/{pid}")
        # comm can contain spaces/parentheses; fields after the last ')' are fixed.
        fields = (root / "stat").read_text().rpartition(")")[2].split()
        if fields[0] in ("Z", "X"):
            raise RuntimeError("Ancestor process exited during inspection")
        return int(fields[1]), root / "exe", int(fields[19])
    if sys.platform == "darwin":
        class BsdInfo(ctypes.Structure):
            _fields_ = [("ids", ctypes.c_uint32 * 12), ("names", ctypes.c_char * 48),
                        ("groups", ctypes.c_uint32 * 6),
                        ("seconds", ctypes.c_uint64), ("microseconds", ctypes.c_uint64)]
        library = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        library.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64,
                                        ctypes.c_void_p, ctypes.c_int]
        library.proc_pidinfo.restype = ctypes.c_int
        library.proc_pidpath.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
        library.proc_pidpath.restype = ctypes.c_int
        if pid == 1:
            # macOS can deny BSD birth metadata for init. Its reserved PID and
            # kernel-reported protected system image identify the root instead.
            buffer = ctypes.create_string_buffer(4096)
            if library.proc_pidpath(pid, buffer, len(buffer)) <= 0:
                raise OSError(ctypes.get_errno(), "Cannot inspect macOS init image")
            image = Path(os.fsdecode(buffer.value))
            if image != Path("/sbin/launchd"):
                raise RuntimeError("Unverified macOS init process boundary")
            return 0, image, 0
        info = BsdInfo()
        if library.proc_pidinfo(pid, 3, 0, ctypes.byref(info), ctypes.sizeof(info)) != ctypes.sizeof(info):
            raise OSError(ctypes.get_errno(), "Cannot inspect ancestor process identity")
        if info.ids[3] != pid or info.ids[1] == 5:  # SZOMB
            raise RuntimeError("Ancestor process exited or changed during inspection")
        buffer = ctypes.create_string_buffer(4096)
        if library.proc_pidpath(pid, buffer, len(buffer)) <= 0:
            raise OSError(ctypes.get_errno(), "Cannot inspect ancestor executable")
        return info.ids[4], Path(os.fsdecode(buffer.value)), info.seconds * 1000000 + info.microseconds
    raise RuntimeError("Process ancestry inspection is unavailable on this platform")


def native_ctx(executable: Path) -> bool:
    # Inspect the actual running image, not its name (renamed Ctx still matches).
    # PIRA: released Ctx 2.0.0 embeds this option; update this signature if a
    # future release removes it. It is present even for fast exact commands,
    # before any live metadata exists. No ancestor program is executed as a probe.
    marker = b"PIRA_CTX_LIVE_CHECKPOINT_MS"
    tail = b""
    with executable.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            data = tail + block
            if marker in data:
                return True
            tail = data[-len(marker):]
    return False


def require_unwrapped_migration() -> None:
    """Reject enclosing Ctx, or unknown ancestry, before any migration writes."""
    try:
        if sys.platform not in ("linux", "darwin", "win32"):
            raise RuntimeError("Process ancestry inspection is unavailable on this platform")
        current = process_parent(os.getpid())
        pid, _, child_started = current
        seen = {os.getpid()}
        while pid:
            if pid in seen:
                raise RuntimeError("Cyclic process ancestry")
            seen.add(pid)
            identity = process_parent(pid)
            parent, executable, started = identity
            if started > child_started:
                raise RuntimeError("Recorded ancestor PID was reused after its child started")
            if executable is None:
                if sys.platform != "win32" or pid != 4 or parent != 0:
                    raise RuntimeError("Unverified process image boundary")
                enclosing_ctx = False
            else:
                enclosing_ctx = native_ctx(executable)
            if process_parent(pid) != identity:
                raise RuntimeError("Ancestor identity changed during image inspection")
            if enclosing_ctx:
                raise RuntimeError("Enclosing native Ctx detected. " + RETRY)
            pid, child_started = parent, started
        if process_parent(os.getpid()) != current:
            raise RuntimeError("Setup ancestry changed during inspection")
    except (OSError, ValueError, IndexError, RuntimeError) as error:
        if RETRY in str(error):
            raise
        raise RuntimeError(f"Cannot verify unwrapped migration: {error}. {RETRY}") from error
