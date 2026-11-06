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
        team_binary = Path(os.environ.get("PIRA_TEAM_BIN", root / "tools/target/debug" / ("pira_team"+suffix))).resolve(strict=True)
        dec_binary = Path(os.environ.get("PIRA_DEC_BIN", root / "tools/target/debug" / ("pira_dec"+suffix))).resolve(strict=True)
        # Resolve rustup's selected native toolchain before replacing HOME. Tests
        # compile tiny native peers, but must not inherit user/runner configuration.
        compiler = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip()) / "bin"
        (compiler / ("rustc"+suffix)).resolve(strict=True)
        environment["PATH"] = os.pathsep.join((str(team_binary.parent), str(compiler), environment["PATH"]))
        environment.update(HOME=str(home), USERPROFILE=str(home), LOCALAPPDATA=str(home),
            TMP=str(home), TEMP=str(home), TMPDIR=str(home), PYTHONDONTWRITEBYTECODE="1",
            PIRA_TEAM_BIN=str(team_binary), PIRA_DEC_BIN=str(dec_binary),
            TEAM_TEST_SLOW_PREFLIGHT="1")
        if os.name == "nt":
            executable = home / "codex.exe"
            subprocess.run(["rustc","--edition=2024",str(tests/"native_fake_codex.rs"),
                            "-o",str(executable)], check=True)
            environment["PIRA_TEAM_TEST_CODEX"] = str(executable)
            claude = home / "claude.exe"
            subprocess.run(["rustc", "--edition=2024", str(tests/"native_fake_claude.rs"), "-o", str(claude)], check=True)
            environment["PIRA_TEAM_TEST_CLAUDE"] = str(claude)
        os.environ.clear()
        os.environ.update(environment)
        tempfile.tempdir = str(home)
        sys.path.insert(0,str(tests))
        loader = unittest.TestLoader()
        requested = sys.argv[1:]
        if os.name == "nt":
            import test_subteams
            # These two non-exec shell-wrapper fixtures require POSIX. Keep the
            # remaining suite's no-skips gate, including newly added tests.
            posix_wrappers = {
                "test_wrapped_backend_rejects_malformed_runtime_and_stale_revision",
                "test_wrapped_backend_preserves_context_and_delegation_limits",
            }
            suite = loader.loadTestsFromNames([
                "test_subteams.SubteamTests." + name
                for name in loader.getTestCaseNames(test_subteams.SubteamTests)
                if name not in posix_wrappers
            ])
            import test_claude
            suite.addTests(loader.loadTestsFromNames([
                "test_claude.ClaudeTests." + name
                for name in loader.getTestCaseNames(test_claude.ClaudeTests)
                if name != "test_worker_context_preserves_role_when_path_changes"
            ]))
            suite.addTests(loader.loadTestsFromName("test_retention"))
            suite.addTests(loader.loadTestsFromNames([
                "test_team.TeamTests.test_file_auth_reused_and_detached_on_run_and_resume",
                "test_team.TeamTests.test_empty_codex_home_uses_home_auth_and_detaches_on_startup_failure",
                "test_team.TeamTests.test_explicit_api_auth_does_not_link_file_auth",
                "test_team.TeamTests.test_final_output_closed_stdout",
                "test_team.TeamTests.test_postpublication_usage_warning_closed_stderr",
                "test_team.TeamTests.test_postpublication_repair_warning_closed_stderr",
                "test_team.TeamTests.test_final_answer_and_full_logs",
                "test_team.TeamTests.test_artifact_receipt_hides_answer_and_preserves_report"]))
            suite.addTests(loader.loadTestsFromNames([
                "test_orphan_recovery.OrphanRecoveryTests." + name for name in (
                    "test_target_only_abandoned_owner_retains_artifacts_and_explicit_resume",
                    "test_missing_control_does_not_authorize_live_owner_settlement",
                    "test_missing_or_mismatched_identity_evidence_fails_closed",
                    "test_live_backend_and_reused_scope_refused_without_signaling",
                    "test_existing_snapshot_republished_without_overwrite_or_duplicate_history",
                    "test_stale_control_failure_cannot_settle_a_resumed_revision",
                    "test_concurrent_recovery_and_resume_preserve_single_attempt_admission",
                    "test_resume_preflight_missing_or_stale_revision_runtime_refused",
                    "test_same_revision_repair_preflight_rejects_previous_and_legacy_bindings",
                    "test_legacy_single_attempt_binding_remains_supported",
                    "test_abandoned_resume_preserves_prior_history_and_cumulative_usage",
                )]))
        else:
            suite = loader.discover(str(tests), pattern="test_*.py")
        if requested:
            # Keep the normal gate's module-import context before selecting cases.
            if loader.errors:
                raise SystemExit("\n".join(loader.errors))
            suite = loader.loadTestsFromNames(requested)
        def supported(suite):
            for case in suite:
                if isinstance(case, unittest.TestSuite):
                    yield from supported(case)
                elif not (getattr(type(case), "__unittest_skip__", False)
                          or getattr(getattr(case, case._testMethodName), "__unittest_skip__", False)):
                    yield case
        suite = unittest.TestSuite(supported(suite))
        required = {"test_subteams.SubteamTests." + name for name in (
            "test_100_simultaneous_launches_preserve_independent_lifecycle",
            "test_100_interleaved_harnesses_preserve_sessions_and_resume")}
        if not requested and not required.issubset({case.id() for case in suite}):
            raise SystemExit("Required Codex/mixed 100-launch cases missing")
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        if not result.wasSuccessful() or result.testsRun < (1 if requested else 24) or result.skipped:
            raise SystemExit("Required native Team behavior failed, skipped, or was not discovered")


if __name__ == "__main__":
    main()
