"""Offline contract tests: synthetic source trees, mocked visibility, local bare Git."""
import io
import json
import tarfile
import zipfile
import os
from pathlib import Path
import subprocess
import sys
import textwrap
import tempfile
import unittest
from unittest.mock import patch

import private_ci as ci


class NativeRelocationRunnerTests(unittest.TestCase):
    def test_exact_commands_isolation_and_retained_failure_evidence(self):
        from contextlib import redirect_stdout
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            scripts = root / 'assets/scripts'
            scripts.mkdir(parents=True)
            binary = root / 'codex.exe'
            binary.write_bytes(b'not executed by this fake fixture')
            test = scripts / 'test_team_store_relocation.py'
            test.write_text(textwrap.dedent("""
                import json, os, pathlib, sys
                scratch = pathlib.Path(os.environ['PIRA_RELOCATION_TEST_SCRATCH'])
                assert 'OPENAI_API_KEY' not in os.environ
                assert 'GITHUB_TOKEN' not in os.environ
                assert 'HTTP_PROXY' not in os.environ
                assert pathlib.Path.home() == scratch / 'home'
                assert pathlib.Path(os.environ['CODEX_HOME']) == scratch / 'home/codex'
                assert os.environ['PYTHONDONTWRITEBYTECODE'] == '1'
                assert pathlib.Path(os.environ['TMP']) == scratch / 'home'
                if '--native' in sys.argv:
                    expected = ['--native', '--completed-turns', '--scratch', str(scratch),
                                '--binary', str(pathlib.Path(__file__).resolve().parents[2] / 'codex.exe')]
                    assert sys.argv[1:] == expected, sys.argv
                    (scratch / 'result.json').write_text(json.dumps({'status':'synthetic'}))
                    (scratch / 'native.stderr').write_text('::error::synthetic stderr')
                    (scratch / 'auth.json').write_text('MUST_NOT_REPLAY')
                    print('completed native fake fixture')
                    raise SystemExit(2 if scratch.name == 'native-fail' else 0)
                assert sys.argv[1:] == ['-v']
                print('deterministic fake fixture')
                raise SystemExit(1 if scratch.name == 'suite-fail' else 0)
            """), encoding='utf-8')
            for name in ('success', 'native-fail', 'suite-fail'):
                with self.subTest(name=name), patch.dict(os.environ, {
                    'OPENAI_API_KEY':'synthetic', 'GITHUB_TOKEN':'synthetic',
                    'HTTP_PROXY':'http://invalid', 'CODEX_HOME':'not-used',
                }):
                    output = io.StringIO()
                    with redirect_stdout(output):
                        if name == 'success':
                            ci.native_relocation(root / name, binary, root)
                        else:
                            with self.assertRaisesRegex(RuntimeError, 'exit [12]'):
                                ci.native_relocation(root / name, binary, root)
                    evidence = output.getvalue()
                    self.assertIn('completed native fake fixture', evidence)
                    self.assertIn('deterministic fake fixture', evidence)
                    self.assertIn('fixture: "::error::synthetic stderr"', evidence)
                    self.assertNotIn('MUST_NOT_REPLAY', evidence)
                    self.assertTrue((root / name / 'result.json').is_file())
            with patch.object(ci.subprocess, 'run') as run:
                with self.assertRaises(FileExistsError):
                    ci.native_relocation(root / 'success', binary, root)
                run.assert_not_called()

    def test_timeout_is_failure_and_still_runs_other_required_suite(self):
        from contextlib import redirect_stdout
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / 'codex'
            binary.touch()
            with patch.object(ci.subprocess, 'run', side_effect=[
                subprocess.TimeoutExpired('fixture', 480), subprocess.CompletedProcess([], 0)
            ]) as run, redirect_stdout(io.StringIO()):
                with self.assertRaisesRegex(RuntimeError, 'timed out'):
                    ci.native_relocation(root / 'evidence', binary, root)
                self.assertEqual(run.call_count, 2)


class PrivateCITests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name)
        self.root = self.base / 'source'
        self.root.mkdir()
        files = {
            'AGENTS.md': b'fixture policy\r\n', 'modules/CODING_STYLE.md': b'fixture style',
            'tools/Cargo.toml': b'[workspace]\n', 'tools/Cargo.lock': b'lock',
            '.github/workflows/build-pira-tool-bundles.yml':
                b'uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1\n',
            'assets/scripts/setup_pira_tools.py': b'import setup_pira_stores',
            'assets/scripts/test_setup_pira_tools.py': b'# fixture',
            'assets/scripts/retire_pira_audio.py': b'# retirement helper fixture',
            'assets/scripts/test_retire_pira_audio.py': b'# retirement test fixture',
            'assets/scripts/setup_pira_stores.py': b'import migrate_pira_stores\nimport setup_migration_choices',
            'assets/scripts/setup_migration_choices.py': b'# receipt helper fixture',
            'assets/scripts/test_setup_migration_choices.py': b'import setup_migration_choices',
            'assets/scripts/migrate_pira_stores.py': b'# migration fixture',
            'assets/scripts/test_migrate_pira_stores.py': b'# migration test fixture',
            'assets/scripts/setup_pira.py': b'import setup_pira_stores',
            'assets/scripts/test_setup_pira.py': b'# unified fixture',
            'assets/scripts/test_setup_pira_stores.py': b'# store fixture',
            'assets/scripts/team_store_relocation.py': b'# helper fixture',
            'assets/scripts/test_team_store_relocation.py': b'# helper tests',
            'assets/scripts/team_relocation_fixture.py': b'# native fixture',
            'tools/build/private_ci.py': b'# provisioning helper',
            'assets/LEGACY_LIST.md': b'# fixture legacy list',
            'tools/select_tool_for_platform.py': b'# fixture',
            'tools/crates/pira_svg_check/tests/fixtures/linux-dejavu-fonts.conf': b'<fontconfig/>',
        }
        for tool in ci.TOOLS:
            files[f'tools/crates/{tool}/Cargo.toml'] = b'[package]\n'
            files[f'tools/src/{tool}/lib.rs'] = b'// fixture\r\n'
            files[f'tools/crates/{tool}/tests/check.rs'] = b'// test'
        for name in ci.POLICIES:
            files[f'tools/src/pira_team/{name}'] = b'policy'
        files['tools/src/pira_team/lib.rs'] = b'const S: &str = include_str!("main.md");'
        self.files = files
        for name, content in files.items():
            self.write(name, content)
        ci.git(self.root, 'init', '--template=')
        ci.git(self.root, 'add', '.')
        ci.git(self.root, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
               '-c', 'commit.gpgSign=false', 'commit', '-m', 'fixture')

    def write(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)

    def stage(self, tools=ci.TOOLS):
        destination = self.base / 'stage'
        destination.mkdir()
        with patch.object(ci.tempfile, 'mkdtemp', return_value=str(destination)):
            self.result = ci.stage(self.root, tools)
        return destination

    def test_staging_exclusions_dependencies_bytes_and_provenance(self):
        excluded = ['tools/src/pira_ctx/stores/private.rs',
                    'tools/src/pira_ctx/debug/private.rs',
                    'tools/crates/pira_team/tests/live_smoke.py',
                    'tools/crates/pira_team/tests/__pycache__/private.py',
                    'tools/crates/pira_team/tests/benchmark_private.py',
                    'tools/auth.json', 'tools/src/pira_team/profile.json',
                    'assets/scripts/auth.json', 'assets/scripts/test_unselected_private.py',
                    'assets/scripts/debug/private.py']
        for name in excluded:
            self.write(name, b'excluded')
        self.write('tools/src/pira_ctx/lib.rs', b'// dirty\r\n')
        before = ci.git(self.root, 'status', '--porcelain=v1', '-z').stdout
        stage = self.stage(('pira_ctx',))
        data = ci.verify(stage)
        self.assertIn('tools/src/pira_team/worker_profiles.json', data)
        for name in excluded:
            self.assertNotIn(name, data)
        expected_sources = set(self.files) - {'.github/workflows/build-pira-tool-bundles.yml'}
        self.assertEqual({path.as_posix() for path in ci.selected(self.root)}, expected_sources)
        self.assertEqual(set(data), expected_sources | {'SNAPSHOT_PROVENANCE.json',
            '.github/workflows/private-pira-tests.yml', '.gitattributes', 'SOURCE_SHA256SUMS'})
        for name in ci.selected(self.root):
            self.assertEqual(data[name.as_posix()], (self.root / name).read_bytes())
        self.assertTrue(self.result['original_dirty'])
        self.assertEqual(self.result['tested_tools'], ['pira_ctx'])
        self.assertEqual(before, ci.git(self.root, 'status', '--porcelain=v1', '-z').stdout)
        workflow = data['.github/workflows/private-pira-tests.yml'].decode()
        self.assertIn('crate: ["pira_ctx"]', workflow)
        self.assertIn('ubuntu-latest, windows-latest', workflow)
        self.assertIn('apt-get\', \'download', workflow)
        self.assertNotIn('sudo', workflow)
        self.assertIn('  setup:', workflow)
        self.assertIn('  native_relocation:', workflow)
        self.assertIn('provision-codex --directory', workflow)
        self.assertIn('native-relocation --directory', workflow)
        self.assertIn('pira-relocation-codex/codex-x86_64-pc-windows-msvc.exe', workflow)
        self.assertNotIn('pending owner delivery', workflow)
        self.assertIn("pattern='test_*.py'", workflow)
        self.assertIn("sys.modules['winreg'] = None", workflow)
        self.assertNotIn("if: matrix.crate == 'pira_team'\n        run: python -m unittest", workflow)
        for name in ('setup_pira.py', 'test_setup_pira.py', 'setup_pira_tools.py',
                     'test_setup_pira_tools.py', 'setup_pira_stores.py', 'test_setup_pira_stores.py',
                     'migrate_pira_stores.py', 'test_migrate_pira_stores.py',
                     'setup_migration_choices.py', 'test_setup_migration_choices.py',
                     'retire_pira_audio.py', 'test_retire_pira_audio.py',
                     'team_store_relocation.py', 'test_team_store_relocation.py', 'team_relocation_fixture.py'):
            self.assertIn('assets/scripts/' + name, data)
        self.assertIn('tests/test_team.py', workflow)
        self.assertNotIn('@CHECKOUT@', workflow)

    def test_setup_job_isolates_environment_and_requires_native_success(self):
        stage = self.stage(('pira_ctx',))
        workflow = (stage / '.github/workflows/private-pira-tests.yml').read_text()
        setup_job = workflow.split('  setup:', 1)[1].split('  native_relocation:', 1)[0]
        self.assertIn('timeout-minutes: 45', setup_job)
        self.assertIn('cargo build --locked --manifest-path tools/Cargo.toml --target-dir tools/target -p pira_ctx', setup_job)
        self.assertIn("'pira_ctx.exe' if sys.platform == 'win32' else 'pira_ctx'", setup_job)
        code = textwrap.dedent(workflow.split("python -B - <<'PY'\n", 1)[1].split("\n          PY", 1)[0])
        fixture = stage / 'assets/scripts/test_migrate_pira_stores.py'
        binary = stage / 'tools/target/debug' / ('pira_ctx.exe' if sys.platform == 'win32' else 'pira_ctx')
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b'fixture; not executed by synthetic test')
        for assertion, expected in (("self.assertTrue(True)", 0), ("self.fail('injected')", 1),
                                    ("self.skipTest('native unavailable')", 1), (None, 1)):
            with self.subTest(assertion=assertion):
                fixture.write_text('' if assertion is None else
                    "import os, pathlib, sys, tempfile, unittest\n"
                    "class MigrationTests(unittest.TestCase):\n"
                    "    def test_native_ctx_event_merge_and_post_use_rerun(self):\n"
                    "        self.assertNotIn('CODEX_HOME', os.environ)\n"
                    "        self.assertNotIn('PIRA_TEAM_DIR', os.environ)\n"
                    "        self.assertNotIn('OPENAI_API_KEY', os.environ)\n"
                    "        self.assertTrue(pathlib.Path(os.environ['HOME']).is_dir())\n"
                    "        self.assertEqual(tempfile.gettempdir(), os.environ['HOME'])\n"
                    "        self.assertIsNone(sys.modules['winreg'])\n"
                    "        binary = pathlib.Path(os.environ['PIRA_TEST_CTX_BINARY'])\n"
                    "        self.assertTrue(binary.is_absolute() and binary.is_file())\n"
                    "        self.assertEqual(binary.parent, (pathlib.Path.cwd() / 'tools/target/debug').resolve())\n"
                    "        " + assertion + "\n")
                result = subprocess.run([sys.executable, '-B', '-c', code], cwd=stage,
                                        env=dict(os.environ, CODEX_HOME='synthetic-private',
                                                 PIRA_TEAM_DIR='synthetic-private',
                                                 OPENAI_API_KEY='synthetic', PIRA_TEST_CTX_BINARY='not-trusted'),
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                if expected:
                    self.assertIn('Required native Ctx migration regression', result.stderr)
        binary.unlink()
        result = subprocess.run([sys.executable, '-B', '-c', code], cwd=stage,
                                capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('FileNotFoundError', result.stderr)

    def test_missing_setup_dependency_fails_staging(self):
        for name in ('retire_pira_audio.py', 'test_retire_pira_audio.py',
                     'setup_pira_stores.py', 'setup_migration_choices.py',
                     'test_setup_migration_choices.py'):
            with self.subTest(name=name):
                path = self.root / 'assets/scripts' / name
                original = path.read_bytes()
                path.unlink()
                try:
                    with self.assertRaises(FileNotFoundError):
                        ci.stage(self.root)
                finally:
                    path.write_bytes(original)

    def test_missing_native_fixture_fails_staging(self):
        (self.root / 'assets/scripts/team_relocation_fixture.py').unlink()
        with self.assertRaises(FileNotFoundError):
            self.stage()

    def test_missing_include_dependency(self):
        self.write('tools/src/pira_ctx/lib.rs', b'include_str!("missing.txt");')
        with self.assertRaisesRegex(RuntimeError, 'unselected include dependency'):
            self.stage()

    def test_concurrent_source_change(self):
        selected = ci.selected
        calls = 0
        def changed(root):
            nonlocal calls
            calls += 1
            if calls == 2:
                self.write('tools/src/pira_ctx/lib.rs', b'changed')
            return selected(root)
        with patch.object(ci, 'selected', side_effect=changed):
            with self.assertRaisesRegex(RuntimeError, 'source changed'):
                self.stage()

    def test_source_symlink_rejected(self):
        target = self.root / 'tools/src/pira_ctx/link.rs'
        target.symlink_to(self.root / 'tools/src/pira_ctx/lib.rs')
        with self.assertRaisesRegex(RuntimeError, 'symlink'):
            self.stage()

    def test_manifest_unsafe_duplicate_and_extraneous(self):
        stage = self.stage()
        manifest = stage / 'SOURCE_SHA256SUMS'
        original = manifest.read_bytes()
        for name in ['../outside', '/absolute', 'tools/../outside', './file',
                     'a//b', 'C:/file', 'a\\b', '.git/config']:
            with self.subTest(name=name):
                manifest.write_bytes(original + f'{"0" * 64}  {name}\n'.encode())
                with self.assertRaisesRegex(ValueError, 'unsafe'):
                    ci.verify(stage)
        manifest.write_bytes(original + original.splitlines(keepends=True)[0])
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            ci.verify(stage)
        manifest.write_bytes(original)
        (stage / 'extra').write_text('extra')
        with self.assertRaisesRegex(ValueError, 'extraneous'):
            ci.verify(stage)

    def test_hash_mismatch(self):
        stage = self.stage()
        (stage / 'AGENTS.md').write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'hash mismatch'):
            ci.verify(stage)

    def test_stage_symlink_rejected(self):
        stage = self.stage()
        (stage / 'AGENTS.md').unlink()
        (stage / 'AGENTS.md').symlink_to(self.root / 'AGENTS.md')
        with self.assertRaisesRegex(ValueError, 'linked'):
            ci.verify(stage)

    def test_privacy_fail_closed(self):
        for info in [{'private': False, 'full_name': 'owner/repo'},
                     {'private': True, 'full_name': 'other/repo'}, {}]:
            with self.subTest(info=info), patch.object(ci.subprocess, 'run', return_value=
                    subprocess.CompletedProcess([], 0, stdout=json.dumps(info))):
                with self.assertRaisesRegex(ValueError, 'non-private'):
                    ci.require_private('owner/repo')

    def test_publish_rejects_public_before_git(self):
        stage = self.stage()
        with patch.object(ci, 'require_private', side_effect=ValueError('non-private')), \
                patch.object(ci, 'git') as git:
            with self.assertRaisesRegex(ValueError, 'non-private'):
                ci.publish(stage, 'owner/repo', 'ci/new')
            git.assert_not_called()

    def publication(self, fail=False, race=False, uncertain=False):
        stage = self.stage()
        remote = self.base / 'remote.git'
        real_git = ci.git
        real_git(self.base, 'init', '--bare', '--template=', str(remote))
        before = ci.verify(stage)
        source_status = real_git(self.root, 'status', '--porcelain=v1', '-z').stdout
        def local_git(root, *args):
            args = tuple(str(remote) if a == 'https://github.com/owner/repo.git' else a for a in args)
            if args[0] == 'push' and race:
                real_git(root, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
                         '-c', 'commit.gpgSign=false', 'commit', '--allow-empty', '-m', 'competing')
                real_git(root, 'push', str(remote), 'HEAD:refs/heads/ci/new')
                real_git(root, 'checkout', '--detach', 'HEAD~1')
            if args[0] == 'push' and uncertain:
                real_git(root, *args)
            if args[0] == 'push' and (fail or uncertain):
                raise subprocess.CalledProcessError(1, args, stderr=b'injected transport failure')
            return real_git(root, *args)
        with patch.object(ci, 'git', side_effect=local_git), patch.object(ci, 'require_private') as private:
            if fail or race or uncertain:
                with self.assertRaisesRegex(RuntimeError, 'stage is unchanged.*Expected commit.*ls-remote'):
                    ci.publish(stage, 'owner/repo', 'ci/new')
                if uncertain:
                    with self.assertRaisesRegex(ValueError, 'already exists'):
                        ci.publish(stage, 'owner/repo', 'ci/new')
                if fail:
                    fail = False
                    ci.publish(stage, 'owner/repo', 'ci/new')
            else:
                result = ci.publish(stage, 'owner/repo', 'ci/new')
                self.assertIn(result['commit'], real_git(self.base, 'ls-remote', str(remote)).stdout.decode())
                for name in ('AGENTS.md', 'SOURCE_SHA256SUMS'):
                    self.assertEqual(before[name], real_git(remote, 'show', f'ci/new:{name}').stdout)
                with self.assertRaisesRegex(ValueError, 'already exists'):
                    ci.publish(stage, 'owner/repo', 'ci/new')
            self.assertGreaterEqual(private.call_count, 2)
        self.assertEqual(before, ci.verify(stage))
        self.assertEqual(source_status, real_git(self.root, 'status', '--porcelain=v1', '-z').stdout)
        self.assertFalse((stage / '.git').exists())

    def test_publish_and_existing_branch(self):
        self.publication()

    def test_publish_failure_recovery(self):
        self.publication(fail=True)

    def test_publish_succeeded_but_response_lost(self):
        self.publication(uncertain=True)

    def test_visibility_rechecked_before_push(self):
        stage = self.stage()
        real_git = ci.git
        def offline_git(root, *args):
            if args[0] == 'ls-remote':
                return subprocess.CompletedProcess(args, 0, stdout=b'')
            self.assertNotEqual(args[0], 'push')
            return real_git(root, *args)
        with patch.object(ci, 'git', side_effect=offline_git), patch.object(
                ci, 'require_private', side_effect=[None, ValueError('non-private')]):
            with self.assertRaisesRegex(ValueError, 'non-private'):
                ci.publish(stage, 'owner/repo', 'ci/new')
        ci.verify(stage)

    def test_git_repository_overrides_removed(self):
        with patch.dict(os.environ, {'GIT_DIR': str(self.base / 'missing'),
                                    'GIT_INDEX_FILE': str(self.base / 'index')}):
            self.assertEqual(ci.git(self.root, 'rev-parse', '--show-toplevel').stdout.decode().strip(),
                             str(self.root.resolve()))
        self.assertFalse((self.base / 'index').exists())

    def test_branch_race_refuses_overwrite(self):
        self.publication(race=True)


class NativeCodexProvisionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.destination = self.root / "codex-bin"
        self.version_checks = []

    def archive(self, system, names=None):
        asset, _, member = ci.NATIVE_CODEX_ASSETS[system]
        output = io.BytesIO()
        names = names or (ci.WINDOWS_CODEX_MEMBERS if system == "win32" else [member])
        if system == "win32":
            with zipfile.ZipFile(output, "w") as package:
                for name in names:
                    payload = (json.dumps(ci.WINDOWS_CODEX_MANIFEST).encode() if name == 'codex-package.json'
                               else b'' if name.endswith('/') else b'fixture binary')
                    package.writestr(name, payload)
        else:
            with tarfile.open(fileobj=output, mode="w:gz") as package:
                for name in names:
                    entry = tarfile.TarInfo(name)
                    entry.size = len(b"fixture binary")
                    package.addfile(entry, io.BytesIO(b"fixture binary"))
        return asset, member, output.getvalue()

    def provision(self, system, payload, *, expected=None, version="codex-cli 0.161.0"):
        asset, _, member = ci.NATIVE_CODEX_ASSETS[system]
        def run(command, **kwargs):
            self.version_checks.append(command)
            self.assertEqual(command[1:], ["--version"])
            if system == 'win32':
                self.assertEqual(Path(command[0]).name, ci.WINDOWS_CODEX_MANIFEST['entrypoint'])
                self.assertTrue((Path(command[0]).parent / 'codex-code-mode-host.exe').is_file())
                self.assertTrue((Path(command[0]).parent / 'codex-resources/voice/manifest.json').is_file())
            self.assertEqual(kwargs["stdin"], subprocess.DEVNULL)
            self.assertEqual(kwargs["timeout"], 15)
            self.assertNotIn("CODEX_API_KEY", kwargs["env"])
            self.assertNotIn("OPENAI_API_KEY", kwargs["env"])
            self.assertTrue(Path(kwargs["env"]["CODEX_HOME"]).is_relative_to(self.root))
            return subprocess.CompletedProcess(command, 0, version, "")
        with patch.object(ci.sys, "platform", system), patch.object(ci.platform, "machine", return_value="AMD64"), \
             patch.dict(ci.NATIVE_CODEX_ASSETS, {system: (asset, expected or ci.digest(payload), member)}), \
             patch.dict(os.environ, {"CODEX_API_KEY": "synthetic-private", "OPENAI_API_KEY": "synthetic-private"}), \
             patch.object(ci, "urlopen", return_value=io.BytesIO(payload)) as fetch, \
             patch.object(ci.subprocess, "run", side_effect=run):
            result = ci.provision_codex(self.destination)
            fetch.assert_called_once_with("https://github.com/openai/codex/releases/download/rust-v0.161.0/" + asset,
                                          timeout=60)
            return result

    def test_exact_pins(self):
        self.assertEqual(ci.NATIVE_CODEX_VERSION, "0.161.0")
        self.assertEqual(ci.NATIVE_CODEX_ASSETS["linux"][1], "b1efb95097660d7f2e5a3887618a23f2ea1b0d548078bf92b0f7a5d229a0cef2")
        self.assertEqual(ci.NATIVE_CODEX_ASSETS["win32"][1], "a7493348634867c905f7298211923c57eb01dfe45400207a423c5453b190b11a")

    def test_archive_formats_are_verified_before_local_publication(self):
        for system in ("linux", "win32"):
            with self.subTest(system=system):
                self.destination = self.root / system
                _, _, payload = self.archive(system)
                binary = self.provision(system, payload)
                self.assertEqual(binary.name, "codex-x86_64-pc-windows-msvc.exe" if system == "win32" else "codex")
                self.assertEqual(binary.read_bytes(), b"fixture binary")
                if system == 'win32':
                    self.assertEqual({p.relative_to(self.destination).as_posix() + ('/' if p.is_dir() else '')
                                      for p in self.destination.rglob('*')}, set(ci.WINDOWS_CODEX_MEMBERS))
                    self.assertEqual(json.loads((self.destination / 'codex-package.json').read_text()),
                                     ci.WINDOWS_CODEX_MANIFEST)
                    self.assertEqual(len(ci.WINDOWS_CODEX_MEMBERS), 53)
                else:
                    self.assertEqual(list(self.destination.iterdir()), [binary])
        self.assertEqual(sorted(path.name for path in self.root.iterdir()), ["linux", "win32"])

    def test_bad_digest_and_wrong_version_never_publish(self):
        _, _, payload = self.archive("linux")
        for options, message in (({"expected": "0" * 64}, "SHA-256"), ({"version": "codex-cli 0.160.0"}, "version")):
            with self.subTest(message=message), self.assertRaisesRegex(RuntimeError, message):
                self.provision("linux", payload, **options)
            self.assertEqual(list(self.root.iterdir()), [])
            self.assertEqual(len(self.version_checks), 0 if message == "SHA-256" else 1)

    def test_unexpected_members_and_links_are_rejected(self):
        for system in ("linux", "win32"):
            member = ci.NATIVE_CODEX_ASSETS[system][2]
            for names in (["../escaped"], [member, "unexpected"]):
                with self.subTest(system=system, names=names):
                    _, _, payload = self.archive(system, names)
                    with self.assertRaisesRegex(RuntimeError, "archive member"):
                        self.provision(system, payload)
                    self.assertEqual(list(self.root.iterdir()), [])
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w:gz") as package:
            entry = tarfile.TarInfo(ci.NATIVE_CODEX_ASSETS["linux"][2])
            entry.type, entry.linkname = tarfile.SYMTYPE, "../elsewhere"
            package.addfile(entry)
        with self.assertRaisesRegex(RuntimeError, "archive member"):
            self.provision("linux", output.getvalue())
        self.assertEqual(list(self.root.iterdir()), [])
        output = io.BytesIO()
        with zipfile.ZipFile(output, "w") as package:
            entry = zipfile.ZipInfo(ci.NATIVE_CODEX_ASSETS["win32"][2])
            entry.create_system = 3
            entry.external_attr = (ci.stat.S_IFLNK | 0o777) << 16
            package.writestr(entry, "../elsewhere")
        with self.assertRaisesRegex(RuntimeError, "archive member"):
            self.provision("win32", output.getvalue())
        self.assertEqual(self.version_checks, [])
        self.assertEqual(list(self.root.iterdir()), [])

    def test_windows_complete_layout_rejects_unsafe_metadata_and_inventory(self):
        _, _, valid = self.archive('win32')
        for attack in ('duplicate', 'traversal', 'backslash', 'case-alias', 'missing',
                       'symlink', 'fifo', 'directory-bit', 'manifest', 'expanded-size'):
            with self.subTest(attack=attack):
                output = io.BytesIO()
                with zipfile.ZipFile(io.BytesIO(valid)) as original, zipfile.ZipFile(output, 'w') as package:
                    for item in original.infolist():
                        payload = original.read(item)
                        if item.filename == 'codex-code-mode-host.exe':
                            if attack == 'missing': continue
                            if attack == 'traversal': item.filename = '../codex-code-mode-host.exe'
                            if attack == 'backslash': item.filename = 'codex-path\\..\\host.exe'
                            if attack == 'case-alias': item.filename = item.filename.upper()
                            if attack == 'duplicate': item.filename = 'codex-command-runner.exe'
                            if attack in ('symlink', 'fifo'):
                                item.create_system = 3
                                item.external_attr = (ci.stat.S_IFLNK if attack == 'symlink' else ci.stat.S_IFIFO) << 16
                            if attack == 'directory-bit': item.external_attr |= 0x10
                        if item.filename == 'codex-package.json' and attack == 'manifest':
                            payload = b'{"entrypoint":"unexpected.exe"}'
                        import warnings
                        with warnings.catch_warnings():
                            warnings.simplefilter('ignore', UserWarning)
                            package.writestr(item, payload)
                payload = output.getvalue()
                # Keep download below its bound, but exceed the expanded aggregate.
                if attack == 'expanded-size':
                    packed = io.BytesIO()
                    with zipfile.ZipFile(io.BytesIO(valid)) as original, zipfile.ZipFile(packed, 'w', compression=zipfile.ZIP_DEFLATED) as package:
                        for item in original.infolist():
                            data = original.read(item)
                            if item.filename in ('codex-code-mode-host.exe', 'codex-command-runner.exe'): data = b'x' * 15000
                            package.writestr(item.filename, data)
                    payload = packed.getvalue()
                limit = max(len(payload) + 1, 20000) if attack == 'expanded-size' else ci.MAX_CODEX_BYTES
                with patch.object(ci, 'MAX_CODEX_BYTES', limit), self.assertRaises(RuntimeError):
                    self.provision('win32', payload)
                self.assertEqual(self.version_checks, [])
                self.assertEqual(list(self.root.iterdir()), [])

    def test_bounded_download_and_corrupt_archive_never_execute(self):
        _, _, payload = self.archive("linux")
        with patch.object(ci, "MAX_CODEX_BYTES", 1), self.assertRaisesRegex(RuntimeError, "safety limit"):
            self.provision("linux", payload)
        with self.assertRaisesRegex(RuntimeError, "Invalid Codex archive"):
            self.provision("linux", b"not an archive")
        self.assertEqual(self.version_checks, [])
        self.assertEqual(list(self.root.iterdir()), [])

    def test_existing_destination_and_unsupported_host_never_download(self):
        self.destination.mkdir()
        marker = self.destination / "kept"
        marker.write_bytes(b"unrelated")
        with patch.object(ci.sys, "platform", "linux"), patch.object(ci.platform, "machine", return_value="x86_64"), \
             patch.object(ci, "urlopen") as fetch:
            with self.assertRaisesRegex(RuntimeError, "Refusing to overwrite"):
                ci.provision_codex(self.destination)
            self.assertEqual(marker.read_bytes(), b"unrelated")
            fetch.assert_not_called()
        with patch.object(ci.platform, "machine", return_value="arm64"), patch.object(ci, "urlopen") as fetch:
            with self.assertRaisesRegex(RuntimeError, "x64 Linux or Windows"):
                ci.provision_codex(self.root / "new")
            fetch.assert_not_called()


if __name__ == '__main__':
    unittest.main()
