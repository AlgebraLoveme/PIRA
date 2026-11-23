"""Prerequisite regressions with synthetic provider config and native metadata only."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import unittest

import test_provider_config
from team_fixture import TeamFixture


class PrerequisiteLocalTests(unittest.TestCase):
    def setUp(self) -> None:
        self.fixture = TeamFixture(self.addCleanup)
        self.root = self.fixture.root
        self.home = Path(self.fixture.env["CODEX_HOME"])
        self.fixture.env.pop("CODEX_SQLITE_HOME", None)
        self.fixture.env.pop("PIRA_DEC_WORKSPACE_DIR", None)
        self.fixture.env.pop("TEAM_REAL_COORDINATOR_CODEX", None)

    def source_log(self) -> Path:
        return self.home / "sessions/2026/10/01/rollout-2026-10-01-parent-session.jsonl"

    def index(self, path: Path, *, wal: bool = False, root: Path | None = None) -> sqlite3.Connection:
        connection = sqlite3.connect((root or self.home) / "state_5.sqlite")
        self.addCleanup(connection.close)
        if wal:
            connection.execute("PRAGMA journal_mode=WAL")
            connection.execute("PRAGMA wal_autocheckpoint=0")
        connection.execute("CREATE TABLE threads(id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL)")
        connection.execute("INSERT INTO threads VALUES (?, ?)", ("parent-session", str(path)))
        # An unrelated row points at an invalid/nonconversation target and must never be selected.
        connection.execute("INSERT INTO threads VALUES (?, ?)", ("unrelated", "/synthetic-private-never-read"))
        connection.commit()
        return connection

    def success(self, task: str = "inert prerequisite fixture") -> dict:
        result = self.fixture.launch(task)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_typed_provider_capability_metadata_does_not_enable_tools_or_permissions(self) -> None:
        for enabled, compaction in [(False, "unsupported"), (True, "v2")]:
            with self.subTest(enabled=enabled):
                test_provider_config.install_probe(self.fixture)
                source = test_provider_config.CONFIG.replace("requires_openai_auth = false", "requires_openai_auth = false\n" +
                    f"supports_standalone_web_search = {str(enabled).lower()}\n" +
                    f'capabilities = {{cyber_access_programs = {str(enabled).lower()}, external_web_access = {str(enabled).lower()}, remote_compaction = "{compaction}"}}')
                (self.home / "config.toml").write_text(source)
                expected = json.loads(json.dumps(test_provider_config.EXPECTED))
                provider = expected["model_providers"]["vendor.with.dots"]
                provider.update(supports_standalone_web_search=enabled,
                    capabilities={"cyber_access_programs":enabled, "external_web_access":enabled, "remote_compaction":compaction})
                self.fixture.env["TEAM_PROVIDER_EXPECT"] = json.dumps(expected)
                peer = self.root / "codex"
                text = peer.read_text()
                probe = 'if "generate-json-schema" not in sys.argv: assert ' + repr('web_search="disabled"') + ' in sys.argv\nargs = sys.argv[1:]'
                peer.write_text(text.replace("args = sys.argv[1:]", probe, 1))
                receipt = self.success()
                state = json.loads((Path(receipt["run_root"]) / "manifest.json").read_text())
                self.assertEqual(state["execution_permissions"]["sandbox_policy"]["type"], "workspaceWrite")
                self.assertFalse(state["execution_permissions"]["sandbox_policy"]["networkAccess"])
                self.assertEqual(state["execution_permissions"]["sandbox_policy"]["writableRoots"], [str(self.root)])
                # Reset the temporary peer before installing another independent probe.
                peer.write_text(Path(__file__).with_name("fake_codex.py").read_text())

    def test_invalid_capability_shapes_and_unknown_fields_fail_without_values(self) -> None:
        for value in ['supports_standalone_web_search = "PRIVATE_SENTINEL"',
                      'capabilities = {remote_compaction = "PRIVATE_SENTINEL"}',
                      'capabilities = {external_web_access = 1}',
                      'capabilities = {auth = {command = "/never-run"}}']:
            (self.home / "config.toml").write_text('[model_providers.openai]\n' + value)
            result = self.fixture.launch("must reject invalid capability")
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn("PRIVATE_SENTINEL", result.stderr)
            self.assertFalse(list((self.root / "logs").rglob("requests.jsonl")))

    def test_selected_catalogs_are_explicit_decision_blockers_not_silent_defaults(self) -> None:
        catalog = self.root / "private-catalog.json"
        contents = '{"models":[{"slug":"synthetic","base_instructions":"PRIVATE_SENTINEL"}]}'
        catalog.write_text(contents)
        for source in [
            f'model_catalog_json = {json.dumps(str(catalog))}\n' + test_provider_config.CONFIG,
            test_provider_config.CONFIG.replace('[profiles.chosen]', '[profiles.chosen]\nmodel_catalog_json = ' + json.dumps(str(catalog))),
            test_provider_config.CONFIG.replace('requires_openai_auth = false', 'requires_openai_auth = false\nmodel_catalog_url = "https://example.invalid/catalog?token=PRIVATE_SENTINEL"'),
        ]:
            (self.home / "config.toml").write_text(source)
            result = self.fixture.launch("catalog selection must not disappear")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("catalog requires", result.stderr)
            self.assertNotIn("PRIVATE_SENTINEL", result.stderr)
            self.assertNotIn(str(catalog), result.stderr)
            self.assertFalse(list((self.root / "logs").rglob("requests.jsonl")))
            self.assertEqual(catalog.read_text(), contents)

    def test_unselected_catalogs_do_not_block_ordinary_work(self) -> None:
        test_provider_config.install_probe(self.fixture)
        (self.home / "config.toml").write_text(test_provider_config.CONFIG + '\n[profiles.inactive]\nmodel_catalog_json="/never-read"\n[model_providers.inactive]\nmodel_catalog_url="https://never-request.invalid"\n')
        self.success()

    def test_unavailable_native_index_uses_verified_bounded_fallback(self) -> None:
        db = self.home / "state_5.sqlite"
        db.write_bytes(b"not a SQLite database")
        self.success("corrupt index is not an identity prerequisite")
        db.unlink()
        connection = self.index(self.source_log())
        connection.execute("BEGIN EXCLUSIVE")
        self.success("busy metadata index is not an identity prerequisite")
        connection.rollback()

    def test_unrelated_deep_directory_is_not_a_native_fallback_layout(self) -> None:
        deep = self.home / "sessions/unrelated"
        for _ in range(12):
            deep /= "nested"
        deep.mkdir(parents=True)
        (deep / "rollout-not-this-thread.jsonl").write_text("not conversation data to inspect")
        self.success()

    @unittest.skipUnless(os.name == "posix", "POSIX no-follow boundary")
    def test_index_uses_pinned_physical_log_without_walking_symlinked_sessions(self) -> None:
        physical = self.root / "physical-session-store"
        (self.home / "sessions").rename(physical)
        (self.home / "sessions").symlink_to(physical, target_is_directory=True)
        log = physical / "2026/10/01/rollout-2026-10-01-parent-session.jsonl"
        self.index(log)
        self.success()

    def test_wal_index_lookup_is_read_only_and_can_select_outside_legacy_layout(self) -> None:
        path = self.root / "native-direct/rollout-direct-parent-session.jsonl"
        path.parent.mkdir()
        path.write_bytes(self.source_log().read_bytes())
        self.source_log().unlink()
        connection = self.index(path, wal=True)
        before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                  for p in (self.home / "state_5.sqlite", self.home / "state_5.sqlite-wal")}
        self.success()
        after = {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                 for p in (self.home / "state_5.sqlite", self.home / "state_5.sqlite-wal")}
        self.assertEqual(after, before)
        self.assertEqual(connection.execute("SELECT COUNT(*) FROM threads").fetchone()[0], 2)

    def test_explicit_native_sqlite_home_selects_its_index(self) -> None:
        root = self.root / "native-index"
        root.mkdir()
        path = self.root / "direct/rollout-direct-parent-session.jsonl"
        path.parent.mkdir()
        path.write_bytes(self.source_log().read_bytes())
        self.source_log().unlink()
        self.index(path, root=root)
        self.fixture.env["CODEX_SQLITE_HOME"] = str(root)
        self.success()

    def test_relative_native_home_and_index_use_the_invoking_directory(self) -> None:
        index = self.root / "native-index"
        index.mkdir()
        self.index(self.source_log(), root=index)
        self.fixture.env["CODEX_HOME"] = self.home.name
        self.fixture.env["CODEX_SQLITE_HOME"] = index.name
        result = subprocess.run([str(self.fixture.bin), "run", "--completion-gate", "inert fixture",
            "--model", "test-model", "--effort", "high", "--store", str(self.root / "logs"),
            "--cwd", str(self.root), "--task", "relative homes"], cwd=self.root,
            env=self.fixture.env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)

    @unittest.skipUnless(os.name == "posix", "POSIX no-follow boundary")
    def test_fallback_resolves_the_selected_home_alias_without_following_tree_links(self) -> None:
        alias = self.root / "native-home-alias"
        alias.symlink_to(self.home, target_is_directory=True)
        self.fixture.env["CODEX_HOME"] = str(alias)
        self.success("selected native home alias")

    @unittest.skipUnless(os.name == "posix", "POSIX directory alias fixture")
    def test_native_index_pinned_parent_alias_and_fallback_root_alias_are_resolved(self) -> None:
        physical = self.root / "physical-native-sessions"
        self.index(self.source_log())
        (self.home / "sessions").rename(physical)
        (self.home / "sessions").symlink_to(physical, target_is_directory=True)
        self.success("native index contains the ordinary alias spelling")
        (self.home / "state_5.sqlite").unlink()
        self.success("selected root alias with bounded fallback")

    def test_inconsistent_or_nonunique_index_never_falls_back_to_a_different_identity(self) -> None:
        connection = self.index(self.root / "auth.json")
        bad = self.fixture.launch("index path must match caller filename")
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn("does not match caller identity", bad.stderr)
        connection.execute("DROP TABLE threads")
        connection.execute("CREATE TABLE threads(id TEXT, rollout_path TEXT)")
        for _ in range(2):
            connection.execute("INSERT INTO threads VALUES (?, ?)", ("parent-session", str(self.source_log())))
        connection.commit()
        bad = self.fixture.launch("nonunique index must not confer identity")
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn("unique thread identity", bad.stderr)
        self.assertFalse(list((self.root / "logs").rglob("requests.jsonl")))

    def test_indexed_transcript_still_requires_matching_native_metadata(self) -> None:
        connection = self.index(self.source_log())
        for suffix in ["", "_01234567-89ab-cdef-0123-456789abcdef"]:
            with self.subTest(suffix=suffix):
                path = self.root / f"rollout-wrong-parent-session{suffix}.jsonl"
                path.write_text('{"type":"session_meta","payload":{"id":"other-session"}}\n')
                connection.execute("UPDATE threads SET rollout_path = ? WHERE id = ?",
                                   (str(path), "parent-session"))
                connection.commit()
                bad = self.fixture.launch("mismatched indexed identity")
                self.assertNotEqual(bad.returncode, 0)
                self.assertIn("identity mismatch", bad.stderr)
        self.assertFalse(list((self.root / "logs").rglob("requests.jsonl")))

    def test_indexed_uuid_preserves_caller_permissions_without_broadening_discovery(self) -> None:
        connection = self.index(self.source_log())
        legacy = self.success("legacy indexed caller")
        before = json.loads((Path(legacy["run_root"]) / "manifest.json").read_text())
        path = self.source_log().with_name(
            "rollout-native-parent-session_01234567-89ab-cdef-0123-456789abcdef.jsonl")
        self.source_log().rename(path)
        connection.execute("DELETE FROM threads WHERE id = 'parent-session'")
        connection.commit()
        bad = self.fixture.launch("UUID trailer is indexed-only")
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn("expected one matching parent session log", bad.stderr)
        connection.execute("INSERT INTO threads VALUES (?, ?)", ("parent-session", str(path)))
        connection.commit()
        receipt = self.success("UUID indexed caller")
        # Identity-only access must work through the same indexed filename too.
        result = self.fixture.access("read", receipt["run_id"], "manifest.json")
        self.assertEqual(result.returncode, 0, result.stderr)
        after = json.loads(result.stdout)
        self.assertEqual(after["creator_main_session_id"], "parent-session")
        self.assertEqual(after["execution_permissions"], before["execution_permissions"])
        self.assertEqual(after["execution_permissions"]["source"], "verified-caller-turn-context")
        self.assertEqual(after["execution_permissions"]["caller_thread_id"], "parent-session")

    def test_indexed_uuid_rejects_relative_and_traversal_paths(self) -> None:
        path = self.root / "rollout-native-parent-session_01234567-89ab-cdef-0123-456789abcdef.jsonl"
        path.write_bytes(self.source_log().read_bytes())
        connection = self.index(path)
        (self.root / "traversal").mkdir()
        for invalid in [Path(path.name), self.root / "traversal" / ".." / path.name]:
            with self.subTest(path=invalid):
                connection.execute("UPDATE threads SET rollout_path = ? WHERE id = ?",
                                   (str(invalid), "parent-session"))
                connection.commit()
                bad = self.fixture.launch("indexed paths remain absolute without traversal")
                self.assertNotEqual(bad.returncode, 0)
                self.assertIn("invalid indexed caller path", bad.stderr)
        self.assertFalse(list((self.root / "logs").rglob("requests.jsonl")))

    def test_indexed_uuid_still_requires_verified_caller_permissions(self) -> None:
        source = self.fixture.parent([{"approval_policy": "on-request"}])
        path = source.with_name(
            "rollout-native-parent-session_01234567-89ab-cdef-0123-456789abcdef.jsonl")
        source.rename(path)
        self.index(path)
        bad = self.fixture.launch("UUID filename must not bypass caller permissions")
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn("cannot inherit caller approval policy", bad.stderr)
        self.assertFalse(list((self.root / "logs").rglob("requests.jsonl")))

    @unittest.skipUnless(os.name == "posix", "POSIX mode boundary")
    def test_indexed_uuid_rejects_group_writable_file(self) -> None:
        path = self.root / "rollout-native-parent-session_01234567-89ab-cdef-0123-456789abcdef.jsonl"
        # Only this disposable creator changes its process mask; neither the shell
        # nor the launcher inherits it. Never chmod a transcript to bypass checks.
        subprocess.run([sys.executable, "-c",
            "import os,sys; os.umask(0); fd=os.open(sys.argv[1], os.O_WRONLY|os.O_CREAT|os.O_EXCL, 0o620); "
            "os.write(fd, sys.stdin.buffer.read()); os.close(fd)", str(path)],
            input=self.source_log().read_bytes(), check=True, timeout=5)
        self.assertEqual(path.stat().st_mode & 0o777, 0o620)
        self.index(path)
        bad = self.fixture.launch("indexed caller metadata must remain non-writable by others")
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn("not group/world writable", bad.stderr)
        self.assertFalse(list((self.root / "logs").rglob("requests.jsonl")))

    @unittest.skipUnless(os.name == "posix", "POSIX symlink boundary")
    def test_indexed_symlink_transcript_is_not_followed(self) -> None:
        connection = self.index(self.source_log())
        for suffix in ["", "_01234567-89ab-cdef-0123-456789abcdef"]:
            with self.subTest(suffix=suffix):
                link = self.root / f"rollout-link-parent-session{suffix}.jsonl"
                link.symlink_to(self.source_log())
                connection.execute("UPDATE threads SET rollout_path = ? WHERE id = ?",
                                   (str(link), "parent-session"))
                connection.commit()
                bad = self.fixture.launch("must not follow indexed file link")
                self.assertNotEqual(bad.returncode, 0)
                self.assertIn("symlinks", bad.stderr)
        self.assertFalse(list((self.root / "logs").rglob("requests.jsonl")))


if __name__ == "__main__":
    unittest.main()
