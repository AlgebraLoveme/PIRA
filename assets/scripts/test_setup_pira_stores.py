from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import setup_pira_stores as setup
import setup_pira_tools as tools_setup

class StorePathTests(unittest.TestCase):
    def test_alias_before_parent_and_missing_suffix_preserve_records(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            physical = root / "physical"
            (physical / "child").mkdir(parents=True)
            record = physical / "record.bin"
            record.write_bytes(b"existing record\x00\xff")
            before = record.stat()
            alias = root / "alias"
            alias.symlink_to(physical / "child", target_is_directory=True)
            selected = alias / ".." / "new" / "store"
            expected = physical / "new" / "store"
            self.assertEqual(setup.physical_store_path(selected), expected)
            self.assertEqual(setup.physical_store_path(expected), expected)
            self.assertFalse(expected.exists())
            self.assertEqual(record.read_bytes(), b"existing record\x00\xff")
            after = record.stat()
            self.assertEqual((before.st_ino, before.st_size, before.st_mtime_ns),
                             (after.st_ino, after.st_size, after.st_mtime_ns))
            self.assertTrue(alias.is_symlink())

    def test_relative_path_resolves_against_setup_cwd(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            (root / "real").mkdir()
            (root / "alias").symlink_to(root / "real", target_is_directory=True)
            with patch.object(setup.Path, "cwd", return_value=root):
                self.assertEqual(setup.physical_store_path("alias/new"), root / "real/new")

    def test_broken_alias_loop_and_file_ancestor_fail_without_writes(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            (root / "broken").symlink_to(root / "absent", target_is_directory=True)
            (root / "loop").symlink_to(root / "loop", target_is_directory=True)
            (root / "file").write_text("not a directory")
            for name in ("broken", "loop", "file"):
                with self.subTest(name=name), self.assertRaisesRegex(RuntimeError, "Cannot resolve store path"):
                    setup.physical_store_path(root / name / "new")
            self.assertEqual(sorted(p.name for p in root.iterdir()), ["broken", "file", "loop"])

    def test_unresolved_parent_and_inaccessible_ancestor_fail(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            with self.assertRaisesRegex(RuntimeError, "nonexistent directory"):
                setup.physical_store_path(Path(temp) / "absent" / ".." / "store")
            with patch.object(setup.Path, "lstat", side_effect=PermissionError("denied")):
                with self.assertRaisesRegex(RuntimeError, "denied"):
                    setup.physical_store_path(Path(temp) / "store")
        with self.assertRaisesRegex(RuntimeError, "empty"):
            setup.physical_store_path("")

    def test_platform_defaults_match_runtime_and_create_nothing(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            cases = [
                ("darwin", {"HOME": str(root)},
                 root / "Library/Application Support/PIRA/ctx", root / "Library/Application Support/PIRA/decision"),
                ("linux", {"HOME": str(root)},
                 root / ".local/share/pira/ctx", root / ".local/share/pira/decision"),
                ("linux", {"HOME": str(root), "XDG_CACHE_HOME": str(root / "cache"),
                           "XDG_DATA_HOME": str(root / "data")},
                 root / "data/pira/ctx", root / "data/pira/decision"),
                ("win32", {"LOCALAPPDATA": str(root)},
                 root / "PIRA/ctx", root / "PIRA/decision"),
            ]
            for platform, env, ctx, dec in cases:
                with self.subTest(platform=platform, env=env), \
                     patch.dict(setup.os.environ, env, clear=True), patch.object(setup.sys, "platform", platform):
                    self.assertEqual(setup.selected_store_paths(["pira_ctx", "pira_dec", "pira_team"]),
                                     {"PIRA_CTX_STORE_DIR": str(ctx), "PIRA_DEC_STORE_DIR": str(dec),
                                      "PIRA_TEAM_DIR": str(dec.parent / "team")})
            self.assertEqual(list(root.iterdir()), [])

    def test_explicit_environment_wins_and_only_selected_tools_are_resolved(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            (root / "real").mkdir()
            (root / "alias").symlink_to(root / "real", target_is_directory=True)
            env = {"PIRA_CTX_STORE_DIR": str(root / "alias/new"), "PIRA_DEC_STORE_DIR": ""}
            with patch.dict(setup.os.environ, env, clear=True):
                self.assertEqual(setup.selected_store_paths(["pira_ctx"]),
                                 {"PIRA_CTX_STORE_DIR": str(root / "real/new")})
                self.assertEqual(setup.selected_store_paths(["pira_nav", "pira_svg_check"]), {})
                with self.assertRaisesRegex(RuntimeError, "empty"):
                    setup.selected_store_paths(["pira_dec"])
            with patch.dict(setup.os.environ, {}, clear=True):
                with self.assertRaisesRegex(RuntimeError, "PIRA_CTX_STORE_DIR"):
                    setup.selected_store_paths(["pira_ctx"])

    def test_empty_default_parent_does_not_select_setup_working_directory(self) -> None:
        for platform in ("darwin", "linux", "win32"):
            with self.subTest(platform=platform), patch.object(setup.sys, "platform", platform), \
                 patch.dict(setup.os.environ, {"HOME": "", "LOCALAPPDATA": "", "XDG_DATA_HOME": ""}, clear=True):
                with self.assertRaisesRegex(RuntimeError, "Cannot determine"):
                    setup.selected_store_paths(["pira_ctx", "pira_dec", "pira_team"])

    def test_all_custom_paths_override_new_defaults(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            custom = {key: str(root / tool) for tool, key in setup.STORE_ENV_KEYS.items()}
            for platform in ("darwin", "linux", "win32"):
                with self.subTest(platform=platform), patch.object(setup.sys, "platform", platform), \
                     patch.dict(setup.os.environ, custom, clear=True):
                    self.assertEqual(setup.selected_store_paths(list(setup.STORE_ENV_KEYS)), custom)
                    explicit = {"PIRA_TEAM_DIR": str(root / "explicit")}
                    self.assertEqual(setup.selected_store_paths(["pira_team"], explicit), explicit)
            self.assertEqual(list(root.iterdir()), [])


class StoreConfigurationTests(unittest.TestCase):
    def setUp(self) -> None:
        # These fixtures describe POSIX profile semantics, even when file/path
        # operations run on Windows. The registry test supplies its own win32
        # branch and complete winreg double below; never touch the real registry.
        platform = patch.object(setup.sys, "platform", "linux")
        platform.start()
        self.addCleanup(platform.stop)
        # tools.ensure_path chooses by os.name, not sys.platform. Replace only
        # its module view: mutating global os.name would break native pathlib.
        tools_os = SimpleNamespace(**{**vars(os), "name": "posix"})
        tools_platform = patch.object(tools_setup, "os", tools_os)
        tools_platform.start()
        self.addCleanup(tools_platform.stop)
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.real = self.root / "real space"
        (self.real / "child").mkdir(parents=True)
        self.alias = self.root / "alias"
        self.alias.symlink_to(self.real / "child", target_is_directory=True)
        self.selected = str(self.alias / ".." / "new")
        self.expected = str(self.real / "new")
        env = patch.dict(setup.os.environ, {"HOME": str(self.root), "LOCALAPPDATA": str(self.root), "USERPROFILE": str(self.root), "SHELL": "/bin/sh"}, clear=True)
        env.start()
        self.addCleanup(env.stop)
        self.profile = self.root / "profile"
        profiles = patch.object(setup, "shell_profiles", return_value=[self.profile])
        profiles.start()
        self.addCleanup(profiles.stop)

    def test_codex_quoted_dotted_inline_and_profile_store_settings(self) -> None:
        value = json.dumps(self.selected)
        cases = [
            '[shell_environment_policy.set] # retain\nPIRA_CTX_STORE_DIR = VALUE\nOTHER = "kept"\n',
            '["shell_environment_policy"."set"]\n"PIRA_CTX_STORE_DIR" = VALUE\n',
            'shell_environment_policy.set.PIRA_CTX_STORE_DIR = VALUE\n',
            'shell_environment_policy = { inherit = "none", set = { PIRA_CTX_STORE_DIR = VALUE, OTHER = "kept" } }\n',
            '[shell_environment_policy]\nset = { "PIRA_CTX_STORE_DIR" = VALUE }\n',
            '[profiles."my.profile".shell_environment_policy.set]\nPIRA_CTX_STORE_DIR = VALUE\n',
        ]
        for template in cases:
            with self.subTest(template=template):
                original = template.replace("VALUE", value)
                parsed = tomllib.loads(original)
                result = setup.codex_store_configuration(original, ["pira_ctx", "pira_dec"])
                updated = tomllib.loads(result)
                environment = updated["shell_environment_policy"]["set"]
                self.assertEqual(environment["PIRA_CTX_STORE_DIR"], self.expected if "profiles" not in parsed else setup.selected_store_paths(["pira_ctx"])["PIRA_CTX_STORE_DIR"])
                self.assertIn("PIRA_DEC_STORE_DIR", environment)
                if "profiles" in parsed:
                    self.assertEqual(updated["profiles"]["my.profile"]["shell_environment_policy"]["set"]["PIRA_CTX_STORE_DIR"], self.expected)
                if "OTHER" in original:
                    self.assertEqual(environment["OTHER"], "kept")
                if "inherit" in original:
                    self.assertEqual(updated["shell_environment_policy"]["inherit"], "none")
                self.assertEqual(setup.codex_store_configuration(result, ["pira_ctx", "pira_dec"]), result)
        self.assertFalse((self.real / "new").exists())

    def test_codex_selected_scope_does_not_resolve_unselected_broken_path(self) -> None:
        text = 'shell_environment_policy.set = { PIRA_DEC_STORE_DIR = "", PIRA_CTX_STORE_DIR = ' + json.dumps(self.selected) + '}\n'
        result = tomllib.loads(setup.codex_store_configuration(text, ["pira_ctx"]))
        self.assertEqual(result["shell_environment_policy"]["set"], {"PIRA_CTX_STORE_DIR": self.expected, "PIRA_DEC_STORE_DIR": ""})

    @unittest.skipIf(os.name == "nt", "Executes a real POSIX /bin/sh fixture")
    def test_shell_custom_assignment_and_default_persistence_are_idempotent(self) -> None:
        self.profile.write_text('OTHER=kept\nexport PIRA_CTX_STORE_DIR="${HOME}/alias/../new"\n')
        plan = setup.plan_store_environment(["pira_ctx", "pira_dec"])
        self.assertEqual(plan.stores["PIRA_CTX_STORE_DIR"], self.expected)
        before = self.profile.read_bytes()
        setup.apply_store_environment(plan, dry_run=True)
        self.assertEqual(self.profile.read_bytes(), before)
        with self.assertRaisesRegex(RuntimeError, "missing or stale"):
            setup.apply_store_environment(plan, dry_run=True, verify=True)
        self.assertEqual(self.profile.read_bytes(), before)
        setup.apply_store_environment(plan, dry_run=False)
        tools_setup.ensure_path(self.root / "bin", False)
        first = self.profile.read_bytes()
        self.assertIn(b"OTHER=kept", first)
        self.assertTrue(list(self.root.glob("profile.bak.*")))
        next_plan = setup.plan_store_environment(["pira_ctx", "pira_dec"])
        self.assertEqual(next_plan.profiles, {})
        setup.apply_store_environment(next_plan, dry_run=True, verify=True)
        self.assertEqual(self.profile.read_bytes(), first)
        result = subprocess.run(["/bin/sh", "-c", '. "$1"; printf "%s" "$PIRA_CTX_STORE_DIR"', "sh", str(self.profile)],
                                env={"HOME": str(self.root)}, text=True, capture_output=True, check=True)
        self.assertEqual(result.stdout, self.expected)

    def test_custom_store_inside_managed_path_block_survives_refresh(self) -> None:
        self.profile.write_text(setup.BLOCK_START + "\nexport PIRA_CTX_STORE_DIR=" + setup.shlex.quote(self.selected) + "\n" + setup.BLOCK_END + "\n")
        setup.apply_store_environment(setup.plan_store_environment(["pira_ctx"]), dry_run=False)
        tools_setup.ensure_path(self.root / "bin", False)
        plan = setup.plan_store_environment(["pira_ctx"])
        self.assertEqual(plan.stores["PIRA_CTX_STORE_DIR"], self.expected)
        self.assertEqual(plan.profiles, {})
        self.assertEqual(self.profile.read_text().count("export PIRA_CTX_STORE_DIR="), 1)

    def test_shell_conflicts_and_unsupported_expressions_fail_before_any_write(self) -> None:
        second = self.root / "second"
        for line in ('export PIRA_CTX_STORE_DIR="$(touch should-not-exist)"\n',
                     'export PIRA_CTX_STORE_DIR="/another/store"\n',
                     'cat <<EOF\nexport PIRA_CTX_STORE_DIR=/data\nEOF\n',
                     'PROMPT="multiline\nexport PIRA_CTX_STORE_DIR=/data\n"\n',
                     setup.BLOCK_START + "\n"):
            with self.subTest(line=line):
                first = "export PIRA_CTX_STORE_DIR=" + setup.shlex.quote(self.selected) + "\n"
                self.profile.write_text(first)
                second.write_text(line)
                with patch.object(setup, "shell_profiles", return_value=[self.profile, second]):
                    with self.assertRaises(RuntimeError):
                        setup.plan_store_environment(["pira_ctx"])
                self.assertEqual(self.profile.read_text(), first)
                self.assertEqual(second.read_text(), line)
        self.assertFalse((self.root / "should-not-exist").exists())

    def test_shell_embedded_hash_and_comment_do_not_change_identity(self) -> None:
        value = (self.root / "name#literal").as_posix()
        self.profile.write_text(f"export PIRA_CTX_STORE_DIR={value} # user's comment\n")
        self.assertEqual(setup.plan_store_environment(["pira_ctx"]).stores["PIRA_CTX_STORE_DIR"], str(Path(value)))

    def test_literal_shell_path_quotes_and_dollars_roundtrip(self) -> None:
        selected = str(self.root / "quote' $literal")
        with patch.dict(setup.os.environ, {"PIRA_CTX_STORE_DIR": selected}):
            setup.apply_store_environment(setup.plan_store_environment(["pira_ctx"]), dry_run=False)
            self.assertEqual(setup.plan_store_environment(["pira_ctx"]).profiles, {})

    def shell_effective_store(self, profiles: list[Path], env: dict[str, str]) -> str:
        if os.name == "nt":
            self.skipTest("Effective-export comparison requires a real POSIX shell")
        # Execute only these handwritten disposable fixtures, never user profiles.
        command = 'for profile do . "$profile"; done; exec /bin/sh -c \'printf %s "${PIRA_CTX_STORE_DIR-unset}"\''
        result = subprocess.run(["/bin/sh", "-c", command, "sh", *map(str, profiles)],
                                env=env, text=True, capture_output=True, check=True, timeout=10)
        return result.stdout

    def test_profile_variable_reassignment_is_rejected_before_any_mutation(self) -> None:
        second = self.root / "second"
        env = {"HOME": str(self.root), "STORE_ROOT": str(self.root / "inherited")}
        cases = [(declaration, split, '"$STORE_ROOT/ctx"') for declaration, split in (
            ("STORE_ROOT=", False), ("export STORE_ROOT=", False),
            ("export STORE_ROOT=", True), ("readonly STORE_ROOT=", False))]
        cases += [("export HOME=", split, value) for split in (False, True) for value in ("~", "~/ctx")]
        for declaration, split, value in cases:
            with self.subTest(declaration=declaration, split=split, value=value):
                assignment = declaration + setup.shlex.quote(str(self.root / "actual")) + "\n"
                export = "export PIRA_CTX_STORE_DIR=" + value + "\n"
                self.profile.write_text(assignment if split else assignment + export)
                second.write_text(export if split else "")
                before = self.shell_effective_store([self.profile, second], env)
                self.assertEqual(before, str(self.root / ("actual" if value == "~" else "actual/ctx")))
                originals = [path.read_bytes() for path in (self.profile, second)]
                entries = set(self.root.iterdir())
                with patch.dict(setup.os.environ, env, clear=True), \
                     patch.object(setup, "shell_profiles", return_value=[self.profile, second] if split else [self.profile]):
                    with self.assertRaisesRegex(RuntimeError, "Ambiguous profile setting"):
                        setup.plan_store_environment(["pira_ctx"])
                self.assertEqual([path.read_bytes() for path in (self.profile, second)], originals)
                self.assertEqual(set(self.root.iterdir()), entries)
                self.assertEqual(self.shell_effective_store([self.profile, second], env), before)

        self.assertEqual(setup.shell_store_value("~", self.profile), str(self.root))
        self.assertEqual(setup.shell_store_value("~/ctx", self.profile), str(self.root) + "/ctx")
        self.assertEqual(Path(setup.shell_store_value("~/ctx", self.profile)), self.root / "ctx")
        self.assertEqual(setup.shell_store_value("'~/ctx'", self.profile, {"HOME"}), "~/ctx")

    def test_conditional_function_and_command_contexts_are_rejected_without_changes(self) -> None:
        export = "export PIRA_CTX_STORE_DIR=" + setup.shlex.quote(self.selected)
        bodies = [
            "if false; then\n" + export + "\nfi\n",
            "store_function() {\n" + export + "\n}\n",
            "while false; do\n" + export + "\ndone\n",
            "case inactive in active)\n" + export + "\n;; esac\n",
            "false && " + export + "\n:\n",
        ]
        env = {"HOME": str(self.root)}
        for body in bodies:
            with self.subTest(body=body):
                self.profile.write_text(body)
                before = self.shell_effective_store([self.profile], env)
                self.assertEqual(before, "unset")
                entries = set(self.root.iterdir())
                with self.assertRaisesRegex(RuntimeError, "Ambiguous profile setting"):
                    setup.plan_store_environment(["pira_ctx"])
                self.assertEqual(self.profile.read_text(), body)
                self.assertEqual(set(self.root.iterdir()), entries)
                self.assertEqual(self.shell_effective_store([self.profile], env), before)

    def test_supported_exports_keep_effective_identity_with_generated_path_context(self) -> None:
        env = {"HOME": str(self.root), "STORE_ROOT": str(self.alias / ".."), "PATH": "/usr/bin:/bin"}
        for assignment in ("export PIRA_CTX_STORE_DIR=" + setup.shlex.quote(self.selected),
                           'export\tPIRA_CTX_STORE_DIR="${STORE_ROOT}/new"'):
            with self.subTest(assignment=assignment):
                self.profile.write_text("OTHER=\nPIRA_DEC_STORE_DIR=\n" + assignment + "\n")
                tools_setup.ensure_path(self.root / "bin", False)
                before = self.shell_effective_store([self.profile], env)
                with patch.dict(setup.os.environ, env, clear=True):
                    setup.apply_store_environment(setup.plan_store_environment(["pira_ctx"]), dry_run=False)
                    tools_setup.ensure_path(self.root / "bin", False)
                    self.assertEqual(setup.plan_store_environment(["pira_ctx"]).profiles, {})
                after = self.shell_effective_store([self.profile], env)
                self.assertEqual(after, self.expected)
                self.assertEqual(setup.physical_store_path(before), setup.physical_store_path(after))

    def test_shell_expanding_word_forms_are_rejected_without_mutation(self) -> None:
        env = {"HOME": str(self.root), "STORE_ROOT": "inherited"}
        for raw in ("prefix:~", '"prefix":~', "~/part:~", "{one,two}", "~$STORE_ROOT/path", " value", "=sh"):
            with self.subTest(raw=raw):
                body = "export PIRA_CTX_STORE_DIR=" + raw + "\n"
                self.profile.write_text(body)
                before = self.shell_effective_store([self.profile], env)
                with patch.dict(setup.os.environ, env, clear=True):
                    with self.assertRaisesRegex(RuntimeError, "Ambiguous profile setting"):
                        setup.plan_store_environment(["pira_ctx"])
                self.assertEqual(self.profile.read_text(), body)
                self.assertEqual(self.shell_effective_store([self.profile], env), before)

    def test_unexported_store_and_default_environment_reassignment_are_rejected(self) -> None:
        for body in ("PIRA_CTX_STORE_DIR=" + setup.shlex.quote(self.selected) + "\n",
                     "HOME=" + setup.shlex.quote(str(self.root / "other-home")) + "\n",
                     "XDG_DATA_HOME=" + setup.shlex.quote(str(self.root / "other-data")) + "\n"):
            with self.subTest(body=body):
                self.profile.write_text(body)
                with self.assertRaisesRegex(RuntimeError, "Ambiguous profile setting"):
                    setup.plan_store_environment(["pira_ctx"])
                self.assertEqual(self.profile.read_text(), body)

    def test_ordinary_opaque_profiles_are_preserved_without_execution(self) -> None:
        original = ('# user profile\n\nif [ -f "$HOME/helpers" ]; then\n'
                    '  . "$HOME/helpers"\nfi\n'
                    'user_function() { printf "kept"; }\n'
                    'touch "$HOME/should-not-execute"\n\n')
        self.profile.write_text(original, newline="")
        plan = setup.plan_store_environment(["pira_ctx", "pira_dec"], profile_paths=[self.profile])
        setup.apply_store_environment(plan, dry_run=True)
        self.assertEqual(self.profile.read_text(), original)
        setup.apply_store_environment(plan, dry_run=False)
        outside, managed = setup.split_store_block(self.profile.read_text(), self.profile)
        self.assertEqual(outside, original)
        self.assertEqual(managed, plan.stores)
        self.assertFalse((self.root / "should-not-execute").exists())
        self.assertEqual(setup.plan_store_environment(["pira_ctx", "pira_dec"]).profiles, {})
        tools_setup.ensure_path(self.root / "bin", False)
        first = self.profile.read_bytes()
        tools_setup.ensure_path(self.root / "bin", False)
        self.assertEqual(self.profile.read_bytes(), first)
        self.assertIn(original.encode(), first)

    def test_opaque_profile_bytes_including_crlf_are_preserved(self) -> None:
        original = b'# custom\r\n. "$HOME/other"\r\n\r\nprintf kept\r\n'
        self.profile.write_bytes(original)
        setup.apply_store_environment(setup.plan_store_environment(["pira_ctx"]), dry_run=False)
        outside, _ = setup.split_store_block(setup.read_profile(self.profile), self.profile)
        self.assertEqual(outside.encode(), original)
        tools_setup.ensure_path(self.root / "bin", False)
        self.assertTrue(self.profile.read_bytes().startswith(original))
        self.assertEqual(setup.plan_store_environment(["pira_ctx"]).profiles, {})

    def test_managed_store_crlf_preserves_surroundings_and_rejects_invalid_blocks(self) -> None:
        surrounding = '# opaque before\r\nprintf kept\r\n'
        trailing = '# opaque after\r\n'
        block = (setup.STORE_BLOCK_START + "\r\nexport PIRA_CTX_STORE_DIR="
                 + setup.shlex.quote(self.selected) + "\r\n" + setup.STORE_BLOCK_END + "\r\n")
        original = surrounding + block + trailing
        self.profile.write_bytes(original.encode())
        outside, managed = setup.split_store_block(setup.read_profile(self.profile), self.profile)
        self.assertEqual(outside, surrounding + trailing)
        self.assertEqual(managed, {"PIRA_CTX_STORE_DIR": self.selected})
        with patch.dict(sys.modules, {"winreg": None}):
            plan = setup.plan_store_environment(["pira_ctx"])
            setup.apply_store_environment(plan, dry_run=False)
            tools_setup.ensure_path(self.root / "bin", False)
            first = self.profile.read_bytes()
            setup.apply_store_environment(setup.plan_store_environment(["pira_ctx"]), dry_run=False)
            tools_setup.ensure_path(self.root / "bin", False)
            self.assertEqual(self.profile.read_bytes(), first)
        self.assertIn((surrounding + trailing).encode(), first)
        for broken in (block + block, block.replace(setup.STORE_BLOCK_END, "# missing"),
                       block.replace(setup.shlex.quote(self.selected), '"$HOME/not-literal"')):
            with self.subTest(broken=broken), self.assertRaises(RuntimeError):
                setup.split_store_block(broken, self.profile)

    def test_shell_backslash_literals_are_not_unquoted_windows_paths(self) -> None:
        value = r"C:\Users\fixture\name#literal"
        self.assertEqual(setup.shell_store_value(setup.shlex.quote(value), self.profile), value)
        with self.assertRaises(RuntimeError):
            setup.shell_store_value(value, self.profile)

    def test_explicit_choice_migrates_ambiguous_profiles_and_preserves_records(self) -> None:
        record = self.real / "record"
        record.write_bytes(b"existing record")
        before_stat = record.stat()
        bodies = [
            'export STORE_ROOT=' + setup.shlex.quote(str(self.real)) + '\nexport PIRA_CTX_STORE_DIR="$STORE_ROOT/new"\n',
            'if false; then\nexport PIRA_CTX_STORE_DIR=/inactive\nfi\n',
            'later() {\nexport PIRA_CTX_STORE_DIR=/inactive\n}\n',
        ]
        inherited = {"HOME": str(self.root), "STORE_ROOT": str(self.root / "inherited")}
        for body in bodies:
            with self.subTest(body=body):
                self.profile.write_text(body)
                with patch.dict(setup.os.environ, inherited, clear=True):
                    with self.assertRaisesRegex(RuntimeError, "set PIRA_CTX_STORE_DIR explicitly"):
                        setup.plan_store_environment(["pira_ctx"])
                self.assertEqual(self.profile.read_text(), body)
                chosen = {**inherited, "PIRA_CTX_STORE_DIR": self.expected}
                before = self.shell_effective_store([self.profile], chosen)
                with patch.dict(setup.os.environ, chosen, clear=True):
                    plan = setup.plan_store_environment(["pira_ctx"])
                    self.assertTrue(any("MIGRATION" in note and "explicit PIRA_CTX_STORE_DIR" in note for note in plan.notices))
                    setup.apply_store_environment(plan, dry_run=True)
                    self.assertEqual(self.profile.read_text(), body)
                    setup.apply_store_environment(plan, dry_run=False)
                outside, managed = setup.split_store_block(self.profile.read_text(), self.profile)
                self.assertEqual(outside, body)
                self.assertEqual(managed, {"PIRA_CTX_STORE_DIR": self.expected})
                after = self.shell_effective_store([self.profile], inherited)
                self.assertEqual(setup.physical_store_path(before), setup.physical_store_path(after))
                # Migration persists the choice; rerun need not repeat the override.
                with patch.dict(setup.os.environ, inherited, clear=True):
                    setup.apply_store_environment(setup.plan_store_environment(["pira_ctx"]), dry_run=True, verify=True)
        self.assertEqual(record.read_bytes(), b"existing record")
        self.assertEqual((record.stat().st_ino, record.stat().st_mtime_ns),
                         (before_stat.st_ino, before_stat.st_mtime_ns))

    def test_store_block_validation_and_selected_key_updates(self) -> None:
        env = {"PIRA_CTX_STORE_DIR": self.expected, "PIRA_DEC_STORE_DIR": str(self.real / "dec")}
        with patch.dict(setup.os.environ, env):
            setup.apply_store_environment(setup.plan_store_environment(["pira_ctx", "pira_dec"]), dry_run=False)
        with patch.dict(setup.os.environ, {"PIRA_CTX_STORE_DIR": str(self.real / "other-ctx")}):
            setup.apply_store_environment(setup.plan_store_environment(["pira_ctx"]), dry_run=False)
        _, managed = setup.split_store_block(self.profile.read_text(), self.profile)
        self.assertEqual(managed["PIRA_DEC_STORE_DIR"], env["PIRA_DEC_STORE_DIR"])
        self.assertEqual(managed["PIRA_CTX_STORE_DIR"], str(self.real / "other-ctx"))
        invalid = setup.STORE_BLOCK_START + '\nexport PIRA_CTX_STORE_DIR="$HOME/unproven"\n' + setup.STORE_BLOCK_END + '\n'
        self.profile.write_text(invalid)
        with patch.dict(setup.os.environ, env), self.assertRaisesRegex(RuntimeError, "Nonliteral PIRA store block"):
            setup.plan_store_environment(["pira_ctx"])
        self.assertEqual(self.profile.read_text(), invalid)

    def test_team_configuration_preserves_custom_path_and_unselected_keys(self) -> None:
        body = "export PIRA_TEAM_DIR=" + setup.shlex.quote(self.selected) + "\n"
        self.profile.write_text(body)
        plan = setup.plan_store_environment(["pira_team"])
        self.assertEqual(plan.stores, {"PIRA_TEAM_DIR": self.expected})
        setup.apply_store_environment(plan, dry_run=False)
        self.assertEqual(setup.plan_store_environment(["pira_team"]).profiles, {})
        text = 'shell_environment_policy.set = { PIRA_TEAM_DIR = ' + json.dumps(self.selected) + ', PIRA_CTX_STORE_DIR = "" }\n'
        result = setup.codex_store_configuration(text, ["pira_team"])
        self.assertEqual(tomllib.loads(result)["shell_environment_policy"]["set"],
                         {"PIRA_TEAM_DIR": self.expected, "PIRA_CTX_STORE_DIR": ""})
        self.assertEqual(setup.codex_store_configuration(result, ["pira_team"]), result)

    @unittest.skipIf(os.name == "nt", "POSIX profile mode-bit contract; Windows uses registry configuration")
    def test_profile_replacement_preserves_modes_and_cleans_failed_stage(self) -> None:
        for mode in (0o400, 0o600, 0o640, 0o700):
            with self.subTest(mode=oct(mode)):
                self.profile.unlink(missing_ok=True)
                self.profile.write_text("original\n")
                self.profile.chmod(mode)
                setup.write_profile(self.profile, "updated\n", False)
                self.assertEqual(self.profile.stat().st_mode & 0o777, mode)
                self.assertEqual(self.profile.read_text(), "updated\n")
        with patch.object(setup.os, "replace", side_effect=OSError("injected replace failure")):
            with self.assertRaisesRegex(OSError, "injected"):
                setup.write_profile(self.profile, "not committed\n", False)
        self.assertEqual(self.profile.read_text(), "updated\n")
        self.assertEqual(list(self.root.glob(".*.pira-tmp-*")), [])

    @unittest.skipIf(os.name == "nt", "POSIX profile permission bits, not Windows DACLs")
    def test_backup_and_stage_are_private_before_copying_contents(self) -> None:
        self.profile.write_text("private original\n")
        self.profile.chmod(0o600)
        original_copy = setup.shutil.copy2
        original_replace = setup.os.replace
        def copy(source, destination):
            self.assertEqual(Path(destination).stat().st_mode & 0o777, 0o600)
            return original_copy(source, destination)
        def replace(source, destination):
            self.assertEqual(Path(source).stat().st_mode & 0o777, 0o600)
            return original_replace(source, destination)
        with patch.object(setup.shutil, "copy2", side_effect=copy), \
             patch.object(setup.os, "replace", side_effect=replace):
            setup.write_profile(self.profile, "updated\n", False)
        backups = list(self.root.glob("profile.bak.*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_text(), "private original\n")

    def test_symlink_profile_updates_target_and_rejects_dangling_link(self) -> None:
        target = self.root / "target"
        target.write_text("# shared profile\n")
        target.chmod(0o600)
        original_mode = target.stat().st_mode
        self.profile.symlink_to(target)
        setup.apply_store_environment(setup.plan_store_environment(["pira_ctx"]), dry_run=False)
        tools_setup.ensure_path(self.root / "bin", False)
        self.assertTrue(self.profile.is_symlink())
        self.assertEqual(target.stat().st_mode, original_mode)
        self.assertIn(setup.STORE_BLOCK_START, target.read_text())
        self.assertIn(setup.BLOCK_START, target.read_text())
        self.assertEqual(setup.plan_store_environment(["pira_ctx"]).profiles, {})
        target.unlink()
        entries = set(self.root.iterdir())
        with self.assertRaisesRegex(RuntimeError, "profile"):
            setup.plan_store_environment(["pira_ctx"])
        self.assertEqual(set(self.root.iterdir()), entries)
        self.assertTrue(self.profile.is_symlink())

    def test_opaque_default_mutations_require_explicit_store_choice(self) -> None:
        for name, tool in (("HOME", "pira_ctx"), ("XDG_DATA_HOME", "pira_ctx"),
                           ("XDG_DATA_HOME", "pira_dec")):
            for declaration in ("readonly {name}=/different\nexport {name}\n",
                                "export -n {name}=/different\n", "unset {name}\n",
                                "if true; then {name}=/different; fi\n"):
                with self.subTest(name=name, declaration=declaration):
                    body = declaration.format(name=name)
                    self.profile.write_text(body)
                    entries = set(self.root.iterdir())
                    with self.assertRaisesRegex(RuntimeError, "Ambiguous"):
                        setup.plan_store_environment([tool])
                    self.assertEqual(self.profile.read_text(), body)
                    self.assertEqual(set(self.root.iterdir()), entries)
                    key = setup.STORE_ENV_KEYS[tool]
                    with patch.dict(setup.os.environ, {key: self.expected}):
                        plan = setup.plan_store_environment([tool])
                        self.assertEqual(plan.stores[key], self.expected)
                        self.assertTrue(plan.notices)
        self.profile.write_text("readonly UNRELATED=value\nprintf '%s' \"$HOME\"\n")
        self.assertTrue(setup.plan_store_environment(["pira_ctx"]).stores)
        self.profile.write_text("readonly XDG_CACHE_HOME=/different\n")
        self.assertTrue(setup.plan_store_environment(["pira_ctx"]).stores)

    def test_windows_user_store_environment_no_writes_in_dry_run_or_verify(self) -> None:
        values = {"PIRA_CTX_STORE_DIR": (self.selected, 1), "OTHER": ("untouched", 1)}
        handle = MagicMock()
        handle.__enter__.return_value = handle
        def query(_, key):
            if key not in values:
                raise FileNotFoundError(key)
            return values[key]
        def write(_, key, reserved, kind, value):
            values[key] = (value, kind)
        registry = SimpleNamespace(HKEY_CURRENT_USER=1, REG_SZ=1, REG_EXPAND_SZ=2,
            OpenKey=MagicMock(return_value=handle), CreateKey=MagicMock(return_value=handle),
            QueryValueEx=MagicMock(side_effect=query), SetValueEx=MagicMock(side_effect=write))
        with patch.dict(sys.modules, {"winreg": registry}), patch.object(setup.sys, "platform", "win32"):
            plan = setup.plan_store_environment(["pira_ctx"])
            setup.apply_store_environment(plan, dry_run=True)
            with self.assertRaisesRegex(RuntimeError, "missing or stale"):
                setup.apply_store_environment(plan, dry_run=True, verify=True)
            tools_setup.windows_user_path(self.root / "bin", True)
            registry.CreateKey.assert_not_called()
            registry.SetValueEx.assert_not_called()
            setup.apply_store_environment(plan, dry_run=False)
            self.assertEqual(values, {"PIRA_CTX_STORE_DIR": (self.expected, 1), "OTHER": ("untouched", 1)})
            setup.apply_store_environment(setup.plan_store_environment(["pira_ctx"]), dry_run=True, verify=True)
            self.assertEqual(registry.SetValueEx.call_count, 1)



if __name__ == "__main__":
    unittest.main()
