"""Isolated prerequisite checks: never invoke a real package manager."""
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest

LIB = Path(__file__).resolve().parent / "lib"


@unittest.skipUnless(os.name == "posix", "POSIX shell fixtures")
class ShellBootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="pira-bootstrap-")
        self.addCleanup(self.temp.cleanup)
        self.bin = Path(self.temp.name)
        self.env = {**os.environ, "PATH": str(self.bin), "PIRA_SETUP_ASSUME_YES": "0"}
        self.executable("uname", "#!/bin/sh\nprintf 'Linux\\n'\n")

    def executable(self, name, content):
        path = self.bin / name
        path.write_text(content)
        path.chmod(0o755)
        return path

    def python(self, name, version):
        return self.executable(name, f"#!{sys.executable}\nimport sys\nsys.version_info = {version!r}\nexec(sys.argv[2])\n")

    def run_shell(self, command):
        return subprocess.run(["/bin/sh", "-eu", "-c", f'. "{LIB / "pira_python_bootstrap.sh"}"; {command}'],
                              env=self.env, input="n\n", text=True, capture_output=True, timeout=10)

    def test_configuration_boundary(self):
        for version, accepted in [((2, 7), False), ((3, 10), False), ((3, 11), True), ((3, 14), True)]:
            with self.subTest(version=version):
                path = self.python("python3", version)
                result = self.run_shell("pira_bootstrap_python3")
                self.assertEqual(result.returncode == 0, accepted, result.stderr)
                self.assertEqual(result.stdout.strip(), str(path) if accepted else "")
                if not accepted:
                    self.assertIn("Python 3.11+", result.stderr)
                    self.assertIn("rerun", result.stderr)

    def test_valid_alternative_after_old_python3(self):
        self.python("python3", (3, 10))
        for name in ("python", "python3.11"):
            with self.subTest(candidate=name):
                path = self.python(name, (3, 11))
                result = self.run_shell("pira_bootstrap_python3")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), str(path))
                path.unlink()

    def test_broken_candidate_does_not_mask_alternative(self):
        self.executable("python3", "#!/bin/sh\nexit 42\n")
        path = self.python("python", (3, 11))
        result = self.run_shell("pira_bootstrap_python3")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), str(path))

    def test_missing_python_fails_actionably(self):
        result = self.run_shell("pira_bootstrap_python3 --yes")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertIn("Python 3.11+", result.stderr)

    def test_readonly_missing_python_never_installs_or_prompts(self):
        self.executable("uname", "#!/bin/sh\nprintf 'Darwin\\n'\n")
        self.executable("brew", "#!/bin/sh\nprintf 'UNEXPECTED_INSTALL\\n' >&2\n")
        for flag in ("--dry-run", "--verify"):
            for assume in ("0", "1"):
                with self.subTest(flag=flag, assume=assume):
                    self.env["PIRA_SETUP_ASSUME_YES"] = assume
                    result = self.run_shell("pira_bootstrap_python3 --yes " + flag)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertNotIn("UNEXPECTED_INSTALL", result.stderr)
                    self.assertNotIn("now with Homebrew?", result.stderr)
                    self.assertIn("Python 3.11+", result.stderr)

    def test_audio_keeps_python3_requirement(self):
        path = self.python("python3", (3, 10))
        result = self.run_shell("pira_require_python3")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), str(path))

    def test_fake_install_rechecked_and_stdout_not_captured(self):
        self.executable("uname", "#!/bin/sh\nprintf 'Darwin\\n'\n")
        for version, accepted in [((3, 10), False), ((3, 11), True)]:
            with self.subTest(installed_version=version):
                target = self.bin / "python3"
                target.unlink(missing_ok=True)
                payload = f"#!{sys.executable}\nimport sys\nsys.version_info = {version!r}\nexec(sys.argv[2])\n"
                self.executable("brew", f"#!{sys.executable}\nimport sys\nassert sys.argv[1:] == ['install', 'python'], sys.argv\nfrom pathlib import Path\np=Path({str(target)!r})\np.write_text({payload!r})\np.chmod(0o755)\nprint('fake install output')\n")
                result = self.run_shell("pira_bootstrap_python3 --yes")
                self.assertEqual(result.returncode == 0, accepted, result.stderr)
                self.assertEqual(result.stdout.strip(), str(target) if accepted else "")
                self.assertIn("fake install output", result.stderr)

    def test_shell_syntax(self):
        result = subprocess.run(["/bin/sh", "-n", str(LIB / "pira_python_bootstrap.sh")], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)


class PowerShellBootstrapTests(unittest.TestCase):
    def test_installer_command_contract(self):
        # Literal command contract only: this does not emulate PowerShell execution.
        source = (LIB / "pira_python_bootstrap.ps1").read_text()
        command, output = re.search(r"^    & winget (.+) \| (.+)$", source, re.MULTILINE).groups()
        self.assertEqual(shlex.split(command), [
            "install", "--id", "Python.Python.3.14", "--source", "winget",
            "--accept-package-agreements", "--accept-source-agreements",
        ])
        self.assertEqual(output, "Out-Host")
        self.assertIn('Write-Host "  winget install --id Python.Python.3.14 --source winget"', source)

    @unittest.skipUnless(shutil.which("pwsh"), "native PowerShell unavailable")
    def test_mocked_installer_arguments_and_output(self):
        path = str(LIB / "pira_python_bootstrap.ps1").replace("'", "''")
        code = f"""
$ErrorActionPreference = 'Stop'
. '{path}'
function winget {{
    $script:captured = @($args)
    $global:LASTEXITCODE = 0
    'fake install output'
}}
$env:PIRA_SETUP_ASSUME_YES = '1'
$result = @(Install-PiraPythonHint)
$expected = 'install|--id|Python.Python.3.14|--source|winget|--accept-package-agreements|--accept-source-agreements'
if (($script:captured -join '|') -ne $expected) {{ throw 'Unexpected winget arguments' }}
if ($result.Count -ne 0) {{ throw 'Installer output leaked into interpreter result' }}
"""
        result = subprocess.run([shutil.which("pwsh"), "-NoProfile", "-NonInteractive", "-Command", code], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_powershell_readonly_guard_precedes_installer(self):
        source = (LIB / "pira_python_bootstrap.ps1").read_text()
        bootstrap = source.split("function Bootstrap-PiraPython3", 1)[1]
        self.assertLess(bootstrap.index('"--dry-run"'), bootstrap.index("Install-PiraPythonHint"))
        self.assertLess(bootstrap.index('"--verify"'), bootstrap.index("Install-PiraPythonHint"))
        self.assertIn("throw", bootstrap[:bootstrap.index("Install-PiraPythonHint")])

    @unittest.skipUnless(shutil.which("pwsh"), "native PowerShell unavailable")
    def test_readonly_missing_python_never_calls_mocked_installer(self):
        path = str(LIB / "pira_python_bootstrap.ps1").replace("'", "''")
        code = f"""
$ErrorActionPreference = 'Stop'
. '{path}'
function Find-PiraPython3 {{ return $null }}
function Install-PiraPythonHint {{ throw 'UNEXPECTED_INSTALL' }}
$env:PIRA_SETUP_ASSUME_YES = '1'
foreach ($flag in '--dry-run', '--verify') {{
    try {{ Bootstrap-PiraPython3 -Args @('--yes', $flag); throw 'unexpected success' }}
    catch {{ if ($_.Exception.Message -notlike '*read-only setup*') {{ throw }} }}
}}
"""
        result = subprocess.run([shutil.which("pwsh"), "-NoProfile", "-NonInteractive", "-Command", code],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_probe_expression_boundary(self):
        # Exercise the actual embedded Python expression even without PowerShell.
        source = (LIB / "pira_python_bootstrap.ps1").read_text()
        probe = re.search(r'"(import sys; raise SystemExit[^"\n]+)"', source).group(1)
        for minimum, version, accepted in [(11, (3, 10), False), (11, (3, 11), True), (11, (3, 14), True), (0, (3, 10), True), (0, (2, 7), False)]:
            with self.subTest(minimum=minimum, version=version):
                code = f"import sys; sys.version_info={version!r}; " + probe.replace("$MinimumMinor", str(minimum))
                result = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0 if accepted else 1, result.stderr)

    @unittest.skipUnless(shutil.which("pwsh"), "native PowerShell unavailable")
    def test_native_syntax(self):
        path = str(LIB / "pira_python_bootstrap.ps1").replace("'", "''")
        code = f"$tokens=$null; $errors=$null; [void][System.Management.Automation.Language.Parser]::ParseFile('{path}', [ref]$tokens, [ref]$errors); if ($errors.Count) {{ $errors | Out-String | Write-Error; exit 1 }}"
        result = subprocess.run([shutil.which("pwsh"), "-NoProfile", "-NonInteractive", "-Command", code], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
