"""Required native Team behavior in a disposable, credential-free environment."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


def main():
    root = Path(__file__).resolve().parents[4]
    tests = Path(__file__).resolve().parent
    with tempfile.TemporaryDirectory(prefix="team-native-ci-") as temporary:
        home = Path(temporary)
        # Do not pass Actions tokens, runner credentials or actual user profiles.
        environment = {key:value for key,value in os.environ.items()
            if key.upper() in {"PATH","SYSTEMROOT","WINDIR","COMSPEC","PATHEXT"}}
        suffix = ".exe" if os.name == "nt" else ""
        environment["PATH"] = str(root / "tools/target/debug") + os.pathsep + environment["PATH"]
        environment.update(HOME=str(home), USERPROFILE=str(home), LOCALAPPDATA=str(home),
            TMP=str(home), TEMP=str(home), TMPDIR=str(home), PYTHONDONTWRITEBYTECODE="1",
            PIRA_TEAM_BIN=str(root / "tools/target/debug" / ("pira_team"+suffix)),
            PIRA_DEC_BIN=str(root / "tools/target/debug" / ("pira_dec"+suffix)),
            TEAM_TEST_SLOW_PREFLIGHT="1")
        if os.name == "nt":
            executable = home / "codex.exe"
            subprocess.run(["rustc","--edition=2024",str(tests/"native_fake_codex.rs"),
                            "-o",str(executable)], check=True)
            environment["PIRA_TEAM_TEST_CODEX"] = str(executable)
        os.environ.clear()
        os.environ.update(environment)
        tempfile.tempdir = str(home)
        sys.path.insert(0,str(tests))
        loader = unittest.TestLoader()
        if os.name == "nt":
            suite = loader.loadTestsFromName("test_subteams")
            suite.addTests(loader.loadTestsFromNames([
                "test_team.TeamTests.test_file_auth_reused_and_detached_on_run_and_resume",
                "test_team.TeamTests.test_empty_codex_home_uses_home_auth_and_detaches_on_startup_failure",
                "test_team.TeamTests.test_explicit_api_auth_does_not_link_file_auth"]))
        else:
            suite = loader.discover(str(tests), pattern="test_*.py")
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        if not result.wasSuccessful() or result.testsRun < 24 or result.skipped:
            raise SystemExit("Required native Team behavior failed, skipped, or was not discovered")


if __name__ == "__main__":
    main()
