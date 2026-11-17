"""Local usability regressions. Only synthetic caller logs and inert native peers."""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from team_fixture import TeamFixture


class LocalUsabilityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.fixture = TeamFixture(self.addCleanup)
        self.root, self.bin, self.env = self.fixture.root, self.fixture.bin, self.fixture.env
        self.launch, self.parent, self.access = self.fixture.launch, self.fixture.parent, self.fixture.access
        self.env.pop("PIRA_DEC_WORKSPACE_DIR", None)

    def cli(self, *args: str, cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
        return subprocess.run([str(self.bin), *args, "--store", str(self.root / "logs")],
                              cwd=cwd, env=self.env, capture_output=True, text=True, timeout=10)

    def receipt(self, result: subprocess.CompletedProcess[str]) -> dict:
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def manifest(self, receipt: dict) -> dict:
        return json.loads((Path(receipt["run_root"]) / "manifest.json").read_text())

    def test_read_and_interrupt_ignore_incomplete_current_policy_but_execution_does_not(self) -> None:
        proc, run, _ = self.fixture.start_control_fixture()
        log = self.parent([])
        with log.open("a") as output:
            output.write('{"type":"turn_context"')
        observed = self.access("read", run.name, "manifest.json")
        self.assertEqual(observed.returncode, 0, observed.stderr)
        self.assertEqual(json.loads(observed.stdout)["status"], "running")
        steered = self.access("steer", run.name, "new task")
        self.assertNotEqual(steered.returncode, 0)
        self.assertIn("incomplete record", steered.stderr)
        interrupted = self.access("interrupt", run.name)
        self.assertEqual(interrupted.returncode, 0, interrupted.stderr)
        proc.communicate(timeout=8)
        self.assertEqual(json.loads((run / "manifest.json").read_text())["status"], "interrupted")
        self.assertNotEqual(self.access("resume", run.name).returncode, 0)
        # A mismatching complete identity record must still prevent access.
        self.parent([], identity="other-session")
        self.assertNotEqual(self.access("read", run.name, "manifest.json").returncode, 0)

    @unittest.skipUnless(os.name == "posix", "POSIX inert coordinator fixture")
    def test_coordinator_read_interrupt_keep_role_checks_without_current_policy(self) -> None:
        import test_subteams
        fixture = test_subteams.SubteamTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        process, coordinator = fixture.start()
        child = fixture.child(coordinator)
        native_log, = (fixture.root / "logs" / coordinator / "codex-home/sessions").rglob("*.jsonl")
        with native_log.open("a") as output:
            output.write('{"type":"turn_context"')
        read = fixture.command(coordinator, "read", child["run_id"], "manifest.json")
        self.assertEqual(read["returncode"], 0, read)
        self.assertEqual(json.loads(read["stdout"])["run_id"], child["run_id"])
        missing = fixture.command(coordinator, "read", child["run_id"], "manifest.json",
                                  env={"PIRA_TEAM_ATTEMPT": None})
        self.assertNotEqual(missing["returncode"], 0)
        steer = fixture.command(coordinator, "steer", child["run_id"], "--task", "must not execute",
                                "--completion-gate", "inert")
        self.assertNotEqual(steer["returncode"], 0)
        interrupted = fixture.command(coordinator, "interrupt", child["run_id"])
        self.assertEqual(interrupted["returncode"], 0, interrupted)
        fixture.command(coordinator, operation="wait", handle=child["handle"])
        self.assertEqual(fixture.manifest(child["run_id"])["status"], "interrupted")
        fixture.finish(coordinator)
        self.assertEqual(fixture.wait(process)[0], 0)

    def test_claude_identity_only_read_needs_no_current_permissions(self) -> None:
        first = self.receipt(self.launch("identity fixture"))
        home = self.root / "claude-home"
        projects = home / "projects/project"
        projects.mkdir(parents=True)
        session = "local-claude-session"
        (projects / f"{session}.jsonl").write_text(json.dumps({"sessionId": session}) + '\n{"partial":')
        # Rebind this isolated result to the independently verified Claude owner.
        # This exercises ownership lookup, not backend execution.
        manifest = Path(first["run_root"]) / "manifest.json"
        state = json.loads(manifest.read_text())
        state["creator_main_session_id"] = "claude-" + session
        manifest.write_text(json.dumps(state))
        self.env.update(CLAUDE_CONFIG_DIR=str(home), CLAUDE_CODE_SESSION_ID=session)
        read = self.access("read", first["run_id"], "manifest.json")
        self.assertEqual(read.returncode, 0, read.stderr)
        self.assertNotEqual(self.access("resume", first["run_id"]).returncode, 0)
        (projects / f"{session}.jsonl").write_text('{"sessionId":"wrong"}\n')
        self.assertNotEqual(self.access("read", first["run_id"], "manifest.json").returncode, 0)

    def test_corrupt_defaults_recovery_is_explicit_and_preserves_original(self) -> None:
        store = self.root / "logs"
        store.mkdir(mode=0o700)
        path = store / "worker_defaults.json"
        original = b'{"broken":'
        path.write_bytes(original)
        for args in [("show",), ("reset", "--main", "main"), ("reset", "--all", "--main", "main")]:
            self.assertNotEqual(self.cli("config", *args).returncode, 0)
            self.assertEqual(path.read_bytes(), original)
        self.receipt(self.launch("fully explicit ignores defaults"))
        self.assertEqual(path.read_bytes(), original)
        recovered = self.receipt(self.cli("config", "reset", "--all"))
        backup = Path(recovered["backup"])
        self.assertEqual(backup.parent, store)
        self.assertEqual(backup.read_bytes(), original)
        self.assertEqual(json.loads(path.read_text()), {})
        self.assertEqual(recovered["overrides"], {})

    @unittest.skipUnless(os.name == "posix", "POSIX managed-file boundaries")
    def test_reset_all_rejects_unsafe_file_and_contended_lock(self) -> None:
        store = self.root / "logs"
        store.mkdir(mode=0o700)
        path = store / "worker_defaults.json"
        target = self.root / "outside-defaults"
        target.write_text("{")
        path.symlink_to(target)
        self.assertNotEqual(self.cli("config", "reset", "--all").returncode, 0)
        self.assertEqual(target.read_text(), "{")
        self.assertTrue(path.is_symlink())
        path.unlink()
        path.write_text("{")
        path.chmod(0o666)
        self.assertNotEqual(self.cli("config", "reset", "--all").returncode, 0)
        self.assertEqual(path.read_text(), "{")
        path.chmod(0o600)
        # Rust File::try_lock uses flock on Unix, so this represents another writer.
        import fcntl
        with (store / "worker_defaults.lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            failed = self.cli("config", "reset", "--all")
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("busy", failed.stderr)
        self.assertEqual(path.read_text(), "{")
        self.assertFalse(list(store.glob("*.backup")))

    def test_invalid_backend_effort_is_rejected_before_saving(self) -> None:
        first = self.receipt(self.cli("config", "set", "--main", "main", "--model", "custom", "--effort", "none"))
        path = Path(first["path"])
        original = path.read_bytes()
        failed = self.cli("config", "set", "--main", "other", "--model", "claude-fixture", "--effort", "none")
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("Claude Code does not support", failed.stderr)
        self.assertEqual(path.read_bytes(), original)
        self.receipt(self.cli("config", "set", "--main", "other", "--model", "claude-fixture", "--effort", "high"))

    def test_ambient_out_of_scope_cache_is_optional_but_explicit_root_is_required(self) -> None:
        outside = tempfile.TemporaryDirectory(prefix="team-ambient-cache-", dir=self.root.parent)
        self.addCleanup(outside.cleanup)
        self.env["CARGO_HOME"] = outside.name
        first = self.receipt(self.launch("does not build"))
        self.assertNotIn(str(Path(outside.name).resolve()), self.manifest(first)["build_roots"])
        self.assertEqual(self.manifest(first)["execution_permissions"]["sandbox_policy"]["writableRoots"], [str(self.root)])
        self.env["PIRA_TEAM_BUILD_ROOTS"] = json.dumps([outside.name])
        failed = self.launch("required build root")
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("no extra write root", failed.stderr)

    def test_relative_stores_use_invoking_cwd_and_anchor_is_retained_on_resume(self) -> None:
        invoking, working = self.root / "invoking", self.root / "working"
        invoking.mkdir(); working.mkdir()
        self.env.update(PIRA_CTX_STORE_DIR="relative-ctx", PIRA_DEC_STORE_DIR="relative-dec",
                        PIRA_DEC_WORKSPACE_DIR=str(self.root))
        first = self.receipt(self.cli("run", "--cwd", str(working), "--model", "test-model", "--effort", "high",
                                      "--task", "memory paths", "--completion-gate", "inert", cwd=invoking))
        state = self.manifest(first)
        self.assertEqual(state["ctx_store"], str(invoking / "relative-ctx"))
        self.assertEqual(state["dec_store"], str(invoking / "relative-dec"))
        self.assertEqual(state["dec_workspace"], str(self.root))
        self.env["PIRA_DEC_WORKSPACE_DIR"] = str(working)
        self.receipt(self.cli("resume", first["run_id"], cwd=working))
        self.assertEqual(self.manifest(first)["dec_workspace"], str(self.root))

    def test_anchor_validation_and_nested_git_boundary_match_dec(self) -> None:
        nested = self.root / "nested"
        nested.mkdir()
        (nested / ".git").mkdir()
        self.env["PIRA_DEC_WORKSPACE_DIR"] = str(self.root)
        first = self.receipt(self.cli("run", "--cwd", str(nested), "--model", "test-model", "--effort", "high",
                                      "--task", "git anchor", "--completion-gate", "inert", cwd=nested))
        self.assertEqual(self.manifest(first)["dec_workspace"], str(nested))
        for anchor in ("relative", str(self.root / "missing")):
            self.env["PIRA_DEC_WORKSPACE_DIR"] = anchor
            failed = self.launch("invalid anchor")
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("PIRA_DEC_WORKSPACE_DIR", failed.stderr)
        if os.name == "posix":
            alias = self.root / "alias"
            alias.symlink_to(self.root, target_is_directory=True)
            self.env["PIRA_DEC_WORKSPACE_DIR"] = str(alias)
            failed = self.launch("noncanonical anchor")
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("canonical", failed.stderr)

    def test_bookmarks_and_search_paginate_past_limit_without_exposing_hidden_rows(self) -> None:
        first = self.receipt(self.launch("bookmarks fixture"))
        entries = [["parent-session", f"label-{i:04d}", first["run_id"]] for i in range(1001)]
        entries += [["other-session", "hidden", first["run_id"]], ["parent-session", "dangling", "missing-run"]]
        index = self.root / "logs/bookmarks.json"
        index.write_text(json.dumps([2, entries, []]))
        original = index.read_bytes()
        page = self.receipt(self.cli("bookmarks", "--limit", "1000"))
        self.assertEqual(len(page["matches"]), 1000)
        self.assertTrue(page["has_more"])
        self.assertEqual(page["next_after"], "label-0999")
        last = self.receipt(self.cli("bookmarks", "--limit", "1000", "--after", page["next_after"]))
        self.assertEqual([m["label"] for m in last["matches"]], ["label-1000"])
        self.assertFalse(last["has_more"])
        self.assertNotIn("next_after", last)
        found = self.receipt(self.cli("search", "LABEL", "--after", page["next_after"]))
        self.assertEqual(found["matches"], last["matches"])
        empty = self.receipt(self.cli("bookmarks", "--after", "label-1000"))
        self.assertEqual(empty["matches"], [])
        for args in [("--limit", "1001"), ("--after", "bad/label"), ("--after", "one", "--after", "two")]:
            self.assertNotEqual(self.cli("bookmarks", *args).returncode, 0)
        self.assertEqual(index.read_bytes(), original)

    def test_live_tier_protocol_is_optional_but_core_interrupt_remains_required(self) -> None:
        contract_path = Path(self.env["TEAM_TEST_BACKEND_CONTRACT"])
        for missing in ("method", "response"):
            with self.subTest(missing=missing):
                contract = json.loads(contract_path.read_text())
                if missing == "method":
                    del contract["schemas"]["ClientRequest.json"]["methods"]["turn/settings/update"]
                else:
                    del contract["schemas"]["v2/TurnSettingsUpdateResponse.json"]
                reduced = self.root / (missing + "-contract.json")
                reduced.write_text(json.dumps(contract))
                self.env["TEAM_TEST_BACKEND_CONTRACT"] = str(reduced)
                self.receipt(self.launch("ordinary task", "--service-tier", "fast"))
                proc, run, _ = self.fixture.start_control_fixture()
                rejected = self.access("tier", run.name, "fast")
                self.assertNotEqual(rejected.returncode, 0)
                self.assertIn("turn/settings/update", rejected.stdout)
                self.assertIsNone(proc.poll())
                self.assertEqual(json.loads((run / "manifest.json").read_text())["service_tier"], "standard")
                self.assertEqual(self.access("interrupt", run.name).returncode, 0)
                proc.communicate(timeout=8)
        self.env["TEAM_TEST_BACKEND_CONTRACT"] = str(contract_path)
        self.env["TEAM_BAD_BACKEND"] = "method"
        failed = self.launch("missing core interrupt")
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("turn/interrupt", failed.stderr)


if __name__ == "__main__":
    unittest.main()
