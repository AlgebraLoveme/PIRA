from __future__ import annotations

import ctypes
import os
from pathlib import Path
import select
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

import migrate_pira_stores as migration
import setup_ctx_wrapper as guard
import setup_pira_stores as setup
from test_migrate_pira_stores import native_ctx_environment


class GuardTests(unittest.TestCase):
    def setUp(self):
        self.identities = {100: (101, Path('python'), 300),
                           101: (102, Path('shell'), 200),
                           102: (0, Path('terminal'), 100)}
        self.enterContext(patch.object(guard.os, 'getpid', return_value=100))
        self.inspect = self.enterContext(patch.object(guard, 'process_parent',
            side_effect=lambda pid: self.identities[pid]))
        self.image = self.enterContext(patch.object(guard, 'native_ctx', return_value=False))

    def test_actual_image_marker_not_process_or_environment_names(self):
        # Exercise the real scanner, independent of mocked process metadata.
        with tempfile.TemporaryDirectory() as directory:
            fake = Path(directory) / 'pira_ctx'
            fake.write_bytes(b'unrelated executable')
            renamed = Path(directory) / 'renamed'
            renamed.write_bytes(b'x' * (1024 * 1024 - 8) + b'PIRA_CTX_LIVE_CHECKPOINT_MS')
            self.assertFalse(REAL_SCANNER(fake))
            self.assertTrue(REAL_SCANNER(renamed))

    def test_forged_protocol_environment_grants_no_exemption_or_dependency(self):
        with patch.dict(os.environ, {'PIRA_CTX_SETUP_SOCKET': '@forged', 'PIRA_CTX_SETUP_PID': '102'}):
            guard.require_unwrapped_migration()
        self.assertEqual(self.image.call_count, 2)

    def test_ancestor_not_just_direct_parent_is_rejected(self):
        self.image.side_effect = [False, True]
        with self.assertRaisesRegex(RuntimeError, 'rerun the same setup command directly without pira_ctx'):
            guard.require_unwrapped_migration()

    def test_unavailable_or_exited_ancestry_fails_closed(self):
        for error in (PermissionError('system boundary denied'), ProcessLookupError('ancestor exited')):
            with self.subTest(error=error):
                self.inspect.side_effect = error
                with self.assertRaisesRegex(RuntimeError, 'Cannot verify unwrapped migration'):
                    guard.require_unwrapped_migration()

    def test_unreadable_executable_fails_closed(self):
        self.image.side_effect = PermissionError('image denied')
        with self.assertRaisesRegex(RuntimeError, 'Cannot verify unwrapped migration'):
            guard.require_unwrapped_migration()

    def test_reused_parent_pid_is_not_treated_as_an_unrelated_process(self):
        self.identities[101] = (0, Path('new unrelated image'), 400)
        with self.assertRaisesRegex(RuntimeError, 'ancestor PID was reused'):
            guard.require_unwrapped_migration()
        self.image.assert_not_called()

    def test_identity_change_during_image_read_fails_closed(self):
        self.inspect.side_effect = [self.identities[100], self.identities[101],
                                    (102, Path('replacement'), 250)]
        with self.assertRaisesRegex(RuntimeError, 'identity changed during image inspection'):
            guard.require_unwrapped_migration()

    def test_exit_during_image_read_fails_closed(self):
        self.inspect.side_effect = [self.identities[100], self.identities[101], ProcessLookupError('exited')]
        with self.assertRaisesRegex(RuntimeError, 'Cannot verify unwrapped migration'):
            guard.require_unwrapped_migration()

    def test_current_parent_change_fails_closed(self):
        self.inspect.side_effect = [self.identities[100], self.identities[101], self.identities[101],
                                    self.identities[102], self.identities[102], (0, Path('python'), 300)]
        with self.assertRaisesRegex(RuntimeError, 'Setup ancestry changed'):
            guard.require_unwrapped_migration()

    def test_windows_valid_chain_is_allowed(self):
        self.identities[102] = (4, Path('terminal'), 100)
        self.identities[4] = (0, None, 0)
        with patch.object(guard.sys, 'platform', 'win32'):
            guard.require_unwrapped_migration()

    def test_missing_image_is_only_allowed_for_verified_windows_root(self):
        self.identities[101] = (0, None, 200)
        with patch.object(guard.sys, 'platform', 'win32'):
            with self.assertRaisesRegex(RuntimeError, 'Unverified process image boundary'):
                guard.require_unwrapped_migration()

    def test_unsupported_platform_fails_closed(self):
        with patch.object(guard.sys, 'platform', 'unsupported'):
            with self.assertRaisesRegex(RuntimeError, 'unavailable on this platform'):
                guard.require_unwrapped_migration()


REAL_SCANNER = guard.native_ctx


class WindowsAdapterTests(unittest.TestCase):
    def setUp(self):
        self.kernel = Mock()
        self.kernel.OpenProcess.return_value = 10
        self.kernel.CloseHandle.return_value = 1
        self.kernel.WaitForSingleObject.return_value = 258
        self.kernel.CreateToolhelp32Snapshot.return_value = 20
        self.kernel.GetProcessTimes.side_effect = self.times
        self.kernel.QueryFullProcessImageNameW.side_effect = self.image
        self.kernel.Process32FirstW.side_effect = self.first
        self.kernel.Process32NextW.side_effect = self.next
        self.enterContext(patch.object(guard, 'windows_kernel', return_value=self.kernel))
        self.enterContext(patch.object(guard.ctypes, 'get_last_error', return_value=5, create=True))

    @staticmethod
    def times(handle, created, exited, kernel_time, user_time):
        created._obj.low, created._obj.high = 99, 2
        return 1

    @staticmethod
    def image(handle, flags, buffer, size):
        buffer.value = '/images/real-process.exe'
        size._obj.value = len(buffer.value)
        return 1

    @staticmethod
    def first(snapshot, entry):
        assert entry._obj.size == ctypes.sizeof(guard.WindowsProcessEntry)
        entry._obj.pid, entry._obj.parent = 1, 0
        return 1

    @staticmethod
    def next(snapshot, entry):
        entry._obj.pid, entry._obj.parent = 73, 42
        return 1

    def test_handle_based_image_parent_and_64_bit_creation_time(self):
        self.assertEqual(guard.windows_process(73),
                         (42, Path('/images/real-process.exe'), (2 << 32) + 99))
        self.kernel.OpenProcess.assert_called_once_with(0x101000, False, 73)
        self.assertEqual([call.args[0] for call in self.kernel.CloseHandle.call_args_list], [20, 10])
        self.assertEqual(ctypes.sizeof(guard.FileTime), 8)
        self.assertEqual(ctypes.sizeof(guard.WindowsProcessEntry), 568 if ctypes.sizeof(ctypes.c_void_p) == 8 else 556)

    def test_api_failures_fail_closed_and_close_acquired_handles(self):
        for name, value, expected_handles in [
                ('OpenProcess', 0, []),
                ('GetProcessTimes', 0, [10]),
                ('QueryFullProcessImageNameW', 0, [10]),
                ('CreateToolhelp32Snapshot', ctypes.c_void_p(-1).value, [10]),
                ('Process32FirstW', 0, [20, 10])]:
            with self.subTest(api=name):
                function = getattr(self.kernel, name)
                original = function.side_effect, function.return_value
                function.side_effect, function.return_value = None, value
                self.kernel.CloseHandle.reset_mock()
                try:
                    with self.assertRaises((OSError, RuntimeError)):
                        guard.windows_process(73)
                    self.assertEqual([c.args[0] for c in self.kernel.CloseHandle.call_args_list], expected_handles)
                finally:
                    function.side_effect, function.return_value = original

    def test_exited_and_unqueryable_processes_are_not_system_boundary_exemptions(self):
        for states, expected_handles in [([0], [10]), ([0xffffffff], [10]), ([258, 0], [20, 10])]:
            with self.subTest(states=states):
                self.kernel.WaitForSingleObject.side_effect = states
                self.kernel.CloseHandle.reset_mock()
                with self.assertRaises((OSError, RuntimeError)):
                    guard.windows_process(73)
                self.assertEqual([c.args[0] for c in self.kernel.CloseHandle.call_args_list], expected_handles)

    def test_missing_snapshot_process_fails_closed(self):
        self.kernel.Process32NextW.side_effect = None
        self.kernel.Process32NextW.return_value = 0
        with self.assertRaisesRegex(RuntimeError, 'missing from process snapshot'):
            guard.windows_process(73)
        self.assertEqual([c.args[0] for c in self.kernel.CloseHandle.call_args_list], [20, 10])

    def test_system_root_requires_kernel_pid_and_zero_parent(self):
        def system(snapshot, entry):
            entry._obj.pid, entry._obj.parent = 4, 0
            for index, char in enumerate('System'):
                entry._obj.name[index] = ord(char)
            return 1
        self.kernel.Process32FirstW.side_effect = system
        self.assertEqual(guard.windows_process(4), (0, None, 0))
        self.kernel.OpenProcess.assert_not_called()
        self.kernel.CloseHandle.assert_called_once_with(20)
        def not_root(snapshot, entry):
            entry._obj.pid, entry._obj.parent = 4, 73
            for index, char in enumerate('System'):
                entry._obj.name[index] = ord(char)
            return 1
        self.kernel.Process32FirstW.side_effect = not_root
        with self.assertRaisesRegex(RuntimeError, 'Unverified Windows System process boundary'):
            guard.windows_process(4)

    def test_system_pid_with_unverified_snapshot_name_is_rejected(self):
        def unrelated(snapshot, entry):
            entry._obj.pid, entry._obj.parent = 4, 0
            return 1
        self.kernel.Process32FirstW.side_effect = unrelated
        with self.assertRaisesRegex(RuntimeError, 'Unverified Windows System process identity'):
            guard.windows_process(4)
        self.kernel.OpenProcess.assert_not_called()

    def test_native_ctypes_signatures_are_declared(self):
        # Exercise the real binding initializer, not just the fake API behavior.
        with patch.object(guard.ctypes, 'WinDLL', return_value=self.kernel, create=True):
            real = REAL_WINDOWS_KERNEL()
        self.assertIs(real, self.kernel)
        self.assertEqual(real.OpenProcess.argtypes, [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32])
        self.assertIs(real.QueryFullProcessImageNameW.restype, ctypes.c_int)


REAL_WINDOWS_KERNEL = guard.windows_kernel


class PosixAdapterTests(unittest.TestCase):
    def test_linux_comm_parentheses_and_creation_time(self):
        fields = ['S', '42'] + ['0'] * 17 + ['1234']
        with patch.object(guard.sys, 'platform', 'linux'), \
             patch.object(Path, 'read_text', return_value='73 (name ) with spaces) ' + ' '.join(fields)):
            self.assertEqual(guard.process_parent(73), (42, Path('/proc/73/exe'), 1234))

    def test_linux_zombie_fails_closed(self):
        with patch.object(guard.sys, 'platform', 'linux'), patch.object(Path, 'read_text', return_value='73 (dead) Z 42'):
            with self.assertRaisesRegex(RuntimeError, 'exited'):
                guard.process_parent(73)

    def test_macos_kernel_parent_and_creation_time(self):
        library = Mock()
        def info(pid, flavor, argument, output, size):
            output._obj.ids[3], output._obj.ids[4] = pid, 42
            output._obj.seconds, output._obj.microseconds = 123, 456
            return size
        def image(pid, output, size):
            output.value = b'/image/actual'
            return len(output.value)
        library.proc_pidinfo.side_effect, library.proc_pidpath.side_effect = info, image
        with patch.object(guard.sys, 'platform', 'darwin'), patch.object(guard.ctypes, 'CDLL', return_value=library):
            self.assertEqual(guard.process_parent(73), (42, Path('/image/actual'), 123000456))

    def test_macos_init_requires_kernel_image_not_denied_metadata_fallback(self):
        library = Mock()
        def image(pid, output, size):
            output.value = b'/sbin/launchd'
            return len(output.value)
        library.proc_pidpath.side_effect = image
        library.proc_pidinfo.side_effect = PermissionError('SIP')
        with patch.object(guard.sys, 'platform', 'darwin'), patch.object(guard.ctypes, 'CDLL', return_value=library):
            self.assertEqual(guard.process_parent(1), (0, Path('/sbin/launchd'), 0))
            library.proc_pidinfo.assert_not_called()
            def unrelated(pid, output, size):
                output.value = b'/tmp/launchd'
                return len(output.value)
            library.proc_pidpath.side_effect = unrelated
            with self.assertRaisesRegex(RuntimeError, 'Unverified macOS init process boundary'):
                guard.process_parent(1)
            library.proc_pidpath.side_effect = None
            library.proc_pidpath.return_value = 0
            with self.assertRaises(OSError):
                guard.process_parent(1)

    def test_macos_ordinary_process_identity_denial_is_not_an_exemption(self):
        library = Mock()
        library.proc_pidinfo.return_value = 0
        with patch.object(guard.sys, 'platform', 'darwin'), patch.object(guard.ctypes, 'CDLL', return_value=library):
            with self.assertRaises(OSError):
                guard.process_parent(73)
            library.proc_pidpath.assert_not_called()


class NoCopyTests(unittest.TestCase):
    def setUp(self):
        directory = self.enterContext(tempfile.TemporaryDirectory())
        self.source, self.destination = Path(directory) / 'old', Path(directory) / 'new'
        self.source.mkdir(); self.destination.mkdir()
        for root in (self.source, self.destination):
            (root / 'record.piractx').write_bytes(b'identical')
        self.enterContext(patch.object(guard, 'require_unwrapped_migration',
                                      side_effect=AssertionError('no migration needs no ancestry exemption')))
        self.plan = migration.plan_migration('pira_ctx', [self.source], self.destination)
        self.assertFalse(self.plan.copy_required)

    def test_destination_only_writes_are_not_migration_input(self):
        before = migration.inventory(self.source)
        (self.destination / 'new.piractx').write_bytes(b'new native output')
        migration.apply_migrations([self.plan])
        self.assertEqual(migration.inventory(self.source), before)
        self.assertEqual((self.destination / 'new.piractx').read_bytes(), b'new native output')
        self.assertFalse(list(self.destination.parent.glob('.pira-migrate-*')))

    def test_changed_source_blocks_no_copy_update(self):
        (self.source / 'late.piractx').write_bytes(b'late record')
        with self.assertRaisesRegex(RuntimeError, 'Store changed after migration preflight'):
            migration.apply_migrations([self.plan])
        self.assertFalse((self.destination / 'late.piractx').exists())

    def test_changed_shared_record_blocks_no_copy_update(self):
        (self.destination / 'record.piractx').write_bytes(b'conflicting')
        with self.assertRaisesRegex(RuntimeError, 'Migration destination verification failed'):
            migration.apply_migrations([self.plan])
        self.assertEqual((self.source / 'record.piractx').read_bytes(), b'identical')


CHILD = r'''import sys,time
from pathlib import Path
import setup_pira_stores as setup
root,delay=Path(sys.argv[1]),float(sys.argv[2])
print('before setup',flush=True)
if delay<0: time.sleep(-delay)
p=setup.plan_store_environment(['pira_ctx'],profile_paths=[root/'profile'])
if delay>0: time.sleep(delay)
setup.apply_store_environment(p,dry_run=False)
print('after setup',flush=True)
'''


@unittest.skipUnless(sys.platform in ('linux', 'darwin') and os.environ.get('PIRA_TEST_CTX_BINARY'),
                     'native POSIX released Ctx fixture required')
class ReleasedWrapperTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.profile = self.root / 'profile'
        self.profile.write_text('# original\n')
        self.child = self.root / 'child.py'
        self.child.write_text(CHILD)
        self.binary = str(Path(os.environ['PIRA_TEST_CTX_BINARY']).resolve())
        self.env = native_ctx_environment(self.root)
        with patch.dict(os.environ, self.env, clear=True):
            self.source = setup.historical_store_paths('pira_ctx')[0]
            self.destination = Path(setup.selected_store_paths(['pira_ctx'])['PIRA_CTX_STORE_DIR'])
        self.env.update(PIRA_CTX_STORE_DIR=str(self.source),
                        PYTHONPATH=str(Path(__file__).resolve().parent),
                        PIRA_CTX_THREAD_ID='released-setup-focused-regression')
        self.source.mkdir(parents=True)
        (self.source / 'original.piractx').write_bytes(b'original record')

    def command(self, mode, delay=0):
        return [self.binary, mode, '--intent', 'released-setup-check', '--',
                sys.executable, str(self.child), str(self.root), str(delay)]

    def invoke(self, command, env=None, timeout=50):
        return subprocess.run(command, cwd=self.root, env=env or self.env,
                              capture_output=True, text=True, timeout=timeout)

    def assert_rejected_and_retry(self, mode, delay=0, command=None):
        result = self.invoke(command or self.command(mode, delay))
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        if mode in ('exact', 'auto') and delay < 0:
            self.assertIn('rerun the same setup command directly without pira_ctx', result.stdout + result.stderr)
        else:
            # Capture summaries may select only the beginning of a long traceback.
            self.assertIn('Store migration must run unwrapped', result.stdout + result.stderr)
        self.assertEqual(self.profile.read_text(), '# original\n')
        self.assertFalse((self.destination / 'original.piractx').exists())
        self.assertEqual((self.source / 'original.piractx').read_bytes(), b'original record')
        before = migration.inventory(self.source, tool='pira_ctx')
        retry = self.invoke([sys.executable, str(self.child), str(self.root), '0'])
        self.assertEqual(retry.returncode, 0, retry.stdout + retry.stderr)
        self.assertEqual(migration.inventory(self.source, tool='pira_ctx'), before)
        for name, entry in before.items():
            if migration.copied('pira_ctx', name):
                self.assertTrue(migration.same_data(entry, migration.file_entry(self.destination / name)), name)
        self.assertTrue(any(name.endswith('.piraevt') for name in before), before)
        handles = [p for p in (self.source / 'short-ids').rglob('*') if p.is_file()]
        if mode in ('auto', 'capture', 'check'):
            self.assertTrue(handles, before)
        for handle in handles:
            args = [self.binary, 'raw', '@' + handle.name, '--stdout']
            original = self.invoke(args)
            copied = self.invoke(args, {**self.env, 'PIRA_CTX_STORE_DIR': str(self.destination)})
            self.assertEqual((original.returncode, copied.returncode), (0, 0))
            self.assertEqual(original.stdout, copied.stdout)
        history = self.invoke([self.binary, 'history', 'released-setup-check'],
                              {**self.env, 'PIRA_CTX_STORE_DIR': str(self.destination)})
        self.assertEqual(history.returncode, 0, history.stderr)
        self.assertIn('released-setup-check', history.stdout)
        repeat = self.invoke([sys.executable, str(self.child), str(self.root), '0'])
        self.assertEqual(repeat.returncode, 0, repeat.stdout + repeat.stderr)

    def test_long_exact_refuses_and_direct_retry_preserves_final_history(self):
        self.assert_rejected_and_retry('exact', -31)

    def test_long_auto_refuses_and_direct_retry_preserves_final_capture_and_handles(self):
        self.assert_rejected_and_retry('auto', -31)

    def test_capture_refuses_and_direct_retry_preserves_final_records(self):
        self.assert_rejected_and_retry('capture')

    def test_check_refuses_and_direct_retry_preserves_final_records(self):
        self.assert_rejected_and_retry('check')

    def test_fast_exact_refuses_before_checkpoint(self):
        self.assert_rejected_and_retry('exact')

    def test_empty_historical_runtime_selection_still_requires_unwrapped(self):
        (self.source / 'original.piractx').unlink()
        self.source.rmdir()
        result = self.invoke(self.command('exact'))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('rerun the same setup command directly without pira_ctx', result.stdout + result.stderr)
        self.assertEqual(self.profile.read_text(), '# original\n')
        self.assertFalse(self.destination.exists())

    def test_nested_shell_cannot_hide_native_wrapper(self):
        command = [self.binary, 'check', '--intent', 'released-setup-check', '--', '/bin/sh', '-c',
                   '"$@"; status=$?; exit "$status"', 'sh', sys.executable, str(self.child), str(self.root), '0']
        self.assert_rejected_and_retry('check', command=command)

    def test_renamed_released_binary_is_still_rejected(self):
        renamed = self.root / 'unrelated-name'
        shutil.copyfile(self.binary, renamed)
        renamed.chmod(0o700)
        command = self.command('check'); command[0] = str(renamed)
        self.assert_rejected_and_retry('check', command=command)

    def no_migration(self, mode, delay=0):
        self.destination.mkdir(parents=True)
        shutil.copy2(self.source / 'original.piractx', self.destination / 'original.piractx')
        env = {**self.env, 'PIRA_CTX_STORE_DIR': str(self.destination),
               'PIRA_CTX_SETUP_SOCKET': '@pira-ctx-setup-nonexistent'}
        result = self.invoke(self.command(mode, delay), env)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(str(self.destination), self.profile.read_text())
        self.assertEqual((self.source / 'original.piractx').read_bytes(), b'original record')
        history = self.invoke([self.binary, 'history', 'released-setup-check'], env)
        self.assertEqual(history.returncode, 0, history.stderr)
        self.assertIn('released-setup-check', history.stdout)

    def test_long_auto_no_migration_survives_checkpoint_after_planning(self):
        self.no_migration('auto', 31)

    def test_exact_no_migration(self):
        self.no_migration('exact')

    def test_capture_no_migration(self):
        self.no_migration('capture')

    def test_check_no_migration(self):
        self.no_migration('check')

    def test_unrelated_concurrent_capture_rejected_unwrapped(self):
        fifo = self.root / 'ready'; os.mkfifo(fifo)
        fd = os.open(fifo, os.O_RDONLY | os.O_NONBLOCK)
        self.addCleanup(os.close, fd)
        other = subprocess.Popen([self.binary, 'capture', '--intent', 'unrelated', '--', sys.executable,
                                  '-c', "import sys; f=open(sys.argv[1],'wb',buffering=0); f.write(b'R'); sys.stdin.read(1)", str(fifo)],
                                 cwd=self.root, env=self.env, stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            self.assertTrue(select.select([fd], [], [], 10)[0])
            self.assertEqual(os.read(fd, 1), b'R')
            result = self.invoke([sys.executable, str(self.child), str(self.root), '0'])
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('Unfinished capture', result.stderr)
            self.assertEqual(self.profile.read_text(), '# original\n')
            self.assertFalse(self.destination.exists())
        finally:
            other.communicate('X', timeout=10)
        self.assertEqual(other.returncode, 0)


@unittest.skipUnless(sys.platform == 'win32' and os.environ.get('PIRA_TEST_CTX_BINARY'),
                     'native Windows Ctx fixture required')
class WindowsNativeTests(unittest.TestCase):
    def test_unwrapped_copy_wrapped_refusal_then_no_copy_update(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            source, destination = root / 'old', root / 'new'
            source.mkdir()
            (source / 'record.piractx').write_bytes(b'original')
            binary = str(Path(os.environ['PIRA_TEST_CTX_BINARY']).resolve())
            environment = native_ctx_environment(root)
            environment.update(PYTHONPATH=str(Path(__file__).resolve().parent),
                               PIRA_CTX_STORE_DIR=str(root / 'wrapper'))
            code = ("from pathlib import Path; import sys; import migrate_pira_stores as m; "
                    "p=m.plan_migration('pira_ctx',[Path(sys.argv[1])],Path(sys.argv[2])); "
                    "m.apply_migrations([p]); print('verified')")
            direct = [sys.executable, '-c', code, str(source), str(destination)]
            wrapped = [binary, 'capture', '--intent', 'Windows setup ancestry', '--', *direct]
            refused = subprocess.run(wrapped, cwd=root, env=environment, capture_output=True, text=True, timeout=30)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn('Store migration must run unwrapped', refused.stdout + refused.stderr)
            self.assertFalse(destination.exists())
            copied = subprocess.run(direct, cwd=root, env=environment, capture_output=True, text=True, timeout=30)
            self.assertEqual(copied.returncode, 0, copied.stdout + copied.stderr)
            self.assertEqual((destination / 'record.piractx').read_bytes(), b'original')
            update = subprocess.run(wrapped, cwd=root, env=environment, capture_output=True, text=True, timeout=30)
            self.assertEqual(update.returncode, 0, update.stdout + update.stderr)
            self.assertEqual((source / 'record.piractx').read_bytes(), b'original')


if __name__ == '__main__':
    unittest.main()
