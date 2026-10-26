from __future__ import annotations

import json
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import retire_pira_audio as retire

# Exact historical shared Windows helper, retained only as a retirement fixture.
PLAY = '# Detached local audio helper for PIRA Codex notifications on Windows.\nparam(\n    [Parameter(Mandatory = $true)]\n    [string]$AudioPath\n)\n\n$ErrorActionPreference = "SilentlyContinue"\n$resolved = (Resolve-Path -LiteralPath $AudioPath).Path\nAdd-Type -AssemblyName PresentationCore\n$player = New-Object System.Windows.Media.MediaPlayer\n$player.Open([Uri]$resolved)\nStart-Sleep -Milliseconds 150\n$player.Play()\n\n$maxSeconds = 15\n$started = Get-Date\nwhile (((Get-Date) - $started).TotalSeconds -lt $maxSeconds) {\n    Start-Sleep -Milliseconds 100\n    if ($player.NaturalDuration.HasTimeSpan) {\n        $duration = $player.NaturalDuration.TimeSpan\n        if ($player.Position -ge $duration -and $duration.TotalMilliseconds -gt 0) { break }\n    }\n}\n$player.Close()'


def historical_config(hooks: Path, windows: bool = False) -> str:
    notify = (["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(hooks / "speak_notify.ps1")]
              if windows else ["/bin/bash", str(hooks / "speak_notify.sh")])
    waiting = (f'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "{hooks / "speak_waiting.ps1"}"'
               if windows else "/bin/bash " + retire.shlex.quote(str(hooks / "speak_waiting.sh")))
    return (f'{retire.START}\nnotify = {json.dumps(notify)}\n{retire.END}\n'
            'model = "custom"\n[features]\nhooks = true\nother = false\n'
            '[[hooks.PermissionRequest]]\nmatcher = "user"\n'
            '[[hooks.PermissionRequest.hooks]]\ntype = "command"\ncommand = "my-handler"\n'
            f'{retire.START}\n[[hooks.PermissionRequest]]\nmatcher = "*"\n'
            '[[hooks.PermissionRequest.hooks]]\ntype = "command"\n'
            f'command = {json.dumps(waiting)}\ntimeout = 1\n'
            f'statusMessage = "Checking waiting status audio"\n{retire.END}\n')


class RetirementTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="pira-audio-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.config = self.root / "config.toml"
        self.hooks = self.root / "hooks"
        self.hooks.mkdir()
        self.profile = self.root / "profile"

    def test_literal_quoted_waiting_path_preserves_custom_commands(self):
        for directory in ("plain", "space dir", "apostrophe'dir"):
            with self.subTest(directory=directory):
                hooks = self.root / directory
                script = str(hooks / "speak_waiting.sh")
                quoted = "'" + script.replace("'", "'\"'\"'") + "'"
                command = "/bin/bash " + quoted
                original = historical_config(hooks)
                original = original.replace(json.dumps("/bin/bash " + retire.shlex.quote(script)),
                                            json.dumps(command))
                cleaned = tomllib.loads(retire.clean_config(original, hooks))
                self.assertEqual(cleaned["hooks"]["PermissionRequest"],
                                 [{"matcher": "user", "hooks": [{"type": "command", "command": "my-handler"}]}])
                for suffix in (" extra", "; echo custom", " && echo custom"):
                    with self.subTest(suffix=suffix):
                        self.assertFalse(retire.command_matches(command + suffix, hooks, waiting=True))

    def snapshot(self):
        return {str(p.relative_to(self.root)): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}

    def test_mac_and_windows_blocks_preserve_independent_hooks_settings_media(self):
        for windows in (False, True):
            with self.subTest(windows=windows):
                original = historical_config(self.hooks, windows)
                self.config.write_bytes(original.replace("\n", "\r\n").encode())
                start, end = retire.STARTUP_MARKERS[windows]
                self.profile.write_text(f'custom-before\n{start}\nold codex wrapper\n{end}\ncustom-after\n')
                media = self.root / "my voice.m4a"
                media.write_bytes(b"custom media")
                foreign = self.hooks / "speak_notify.sh"
                foreign.write_bytes(b"custom script")
                plan = retire.plan_retirement(self.config, [self.profile])
                retire.apply_retirement(plan)
                parsed = tomllib.loads(self.config.read_text())
                self.assertNotIn("notify", parsed)
                self.assertEqual(parsed["features"], {"hooks": True, "other": False})
                self.assertEqual(parsed["hooks"]["PermissionRequest"][0]["matcher"], "user")
                self.assertEqual(len(parsed["hooks"]["PermissionRequest"]), 1)
                self.assertEqual(media.read_bytes(), b"custom media")
                self.assertEqual(foreign.read_bytes(), b"custom script")
                self.assertEqual(self.profile.read_text(), "custom-before\ncustom-after\n")
                self.assertTrue(any(p.read_bytes() == original.replace("\n", "\r\n").encode() for p in self.root.glob("config.toml.pira-audio.bak.*")))
                after = self.snapshot()
                retire.apply_retirement(retire.plan_retirement(self.config, [self.profile]))
                self.assertEqual(self.snapshot(), after)

    def test_readonly_cli_and_fresh_absent_setup_are_idempotent(self):
        backup_dir = self.root / "backups"
        args = ["--config", str(self.config), "--profile", str(self.profile), "--backup-dir", str(backup_dir)]
        self.assertEqual(retire.main([*args, "--verify"]), 0)
        self.assertEqual(self.snapshot(), {})
        self.config.write_text(historical_config(self.hooks))
        (self.hooks / "pira_play_audio.ps1").write_text(PLAY)
        before = self.snapshot()
        self.assertEqual(retire.main([*args, "--dry-run"]), 0)
        self.assertEqual(retire.main([*args, "--verify"]), 1)
        self.assertEqual(self.snapshot(), before)
        self.assertFalse(backup_dir.exists())
        self.assertEqual(retire.main(args), 0)
        self.assertEqual(len(list(backup_dir.glob("*.pira-audio.bak.*"))), 2)
        self.assertFalse(list(self.root.glob("*.pira-audio.bak.*")))
        self.assertFalse((self.hooks / "pira_play_audio.ps1").exists())
        self.assertEqual(retire.main([*args, "--verify"]), 0)
        after = self.snapshot()
        self.assertEqual(retire.main(args), 0)
        self.assertEqual(after, self.snapshot())

    def test_invalid_backup_directory_preserves_inputs(self):
        self.config.write_text(historical_config(self.hooks))
        blocked = self.root / "not-a-directory"
        blocked.write_text("keep")
        before = self.snapshot()
        with self.assertRaises(OSError):
            retire.apply_retirement(retire.plan_retirement(self.config, []), backup_dir=blocked)
        self.assertEqual(self.snapshot(), before)

    def test_mixed_marker_blocks_preserve_global_hook_state_and_mcp_values(self):
        for windows in (False, True):
            with self.subTest(windows=windows):
                globals_text = 'personality = "steady"\nservice_tier = "default"\nextra_choice = [1, 2]\n'
                tables_text = ('[hooks.state]\nmode = "independent"\n'
                               '[mcp_servers.example]\ncommand = "custom-server"\n'
                               'args = ["--keep", "a=b"]\n'
                               '[mcp_servers.example.env]\nCUSTOM = "unchanged"\n')
                text = historical_config(self.hooks, windows)
                text = text.replace(retire.END, globals_text + retire.END, 1)
                boundary = text.rfind(retire.END)
                text = text[:boundary] + tables_text + text[boundary:]
                self.config.write_bytes(text.replace("\n", "\r\n").encode())
                backups = self.root / f"rollback-{windows}"
                args = ["--config", str(self.config), "--profile", str(self.profile), "--backup-dir", str(backups)]
                before = self.snapshot()
                self.assertEqual(retire.main([*args, "--dry-run"]), 0)
                self.assertEqual(retire.main([*args, "--verify"]), 1)
                self.assertEqual(self.snapshot(), before)
                self.assertFalse(backups.exists())
                self.assertEqual(retire.main(args), 0)
                cleaned = self.config.read_bytes().decode()
                expected = {
                    "personality": "steady", "service_tier": "default", "extra_choice": [1, 2],
                    "model": "custom", "features": {"hooks": True, "other": False},
                    "hooks": {"PermissionRequest": [{"matcher": "user", "hooks": [
                        {"type": "command", "command": "my-handler"}]}], "state": {"mode": "independent"}},
                    "mcp_servers": {"example": {"command": "custom-server", "args": ["--keep", "a=b"],
                                                 "env": {"CUSTOM": "unchanged"}}},
                }
                self.assertEqual(tomllib.loads(cleaned), expected)
                self.assertIn(globals_text.replace("\n", "\r\n"), cleaned)
                self.assertIn(tables_text.replace("\n", "\r\n"), cleaned)
                self.assertNotIn(retire.START, cleaned)
                self.assertNotIn(retire.END, cleaned)
                self.assertTrue(any(p.read_bytes() == before["config.toml"] for p in backups.iterdir()))
                self.assertEqual(retire.main([*args, "--verify"]), 0)
                after = self.snapshot()
                self.assertEqual(retire.main(args), 0)
                self.assertEqual(self.snapshot(), after)

    def test_quoted_multiline_layout_preserves_added_events_and_literal_table_text(self):
        waiting = "/bin/bash " + retire.shlex.quote(str(self.hooks / "speak_waiting.sh"))
        global_text = ('display_mode = "compact"\n'
                       'arbitrary = { enabled = true, names = ["a", "b"] }\n')
        ui_text = ('["ui"."tab.view"]\n'
                   'description = """\n[[hooks.PermissionRequest]]\nnotify = ["literal data"]\n"""\n')
        state_text = '[ "hooks" . "state" ]\nflags = { manual = true }\n'
        foreign_event = ('[[ "hooks" . \'PermissionRequest\' ]]\nmatcher = "independent-after"\n'
                         'hooks = [{ type = "command", command = "custom-handler", timeout = 7 }]\n')
        mcp_text = ('[ "mcp_servers" . "srv.with.dot" ]\n'
                    'args = [\n  "a#b", # keep this comment\n  "[not-a-header]",\n]\n')
        text = (f'{retire.START}\n"notify" = [\n  "/bin/bash",\n'
                f'  {json.dumps(str(self.hooks / "speak_notify.sh"))},\n]\n'
                f'{global_text}{ui_text}{retire.END}\n'
                f'{retire.START}\n[[ "hooks" . \'PermissionRequest\' ]] # owned header\n'
                'matcher = "*"\nhooks = [\n'
                f'  {{ type = "command", command = {json.dumps(waiting)}, timeout = 1, '
                'statusMessage = "Checking waiting status audio" },\n]\n'
                f'{state_text}{foreign_event}{mcp_text}{retire.END}\n')
        cleaned = retire.clean_config(text, self.hooks)
        expected = {
            "display_mode": "compact", "arbitrary": {"enabled": True, "names": ["a", "b"]},
            "ui": {"tab.view": {"description": '[[hooks.PermissionRequest]]\nnotify = ["literal data"]\n'}},
            "hooks": {"state": {"flags": {"manual": True}}, "PermissionRequest": [
                {"matcher": "independent-after", "hooks": [
                    {"type": "command", "command": "custom-handler", "timeout": 7}]}]},
            "mcp_servers": {"srv.with.dot": {"args": ["a#b", "[not-a-header]"]}},
        }
        self.assertEqual(tomllib.loads(cleaned), expected)
        for untouched in (global_text, ui_text, state_text, foreign_event, mcp_text):
            self.assertIn(untouched, cleaned)
        self.assertNotIn(retire.START, cleaned)
        self.assertEqual(retire.clean_config(cleaned, self.hooks), cleaned)

    def test_explicit_empty_hooks_table_is_preserved(self):
        waiting = "/bin/bash " + retire.shlex.quote(str(self.hooks / "speak_waiting.sh"))
        text = ('[ "hooks" ] # user table\n' + retire.START + '\n'
                '[[hooks.PermissionRequest]]\nmatcher = "*"\n'
                '[[hooks.PermissionRequest.hooks]]\ntype = "command"\n'
                f'command = {json.dumps(waiting)}\ntimeout = 1\n'
                'statusMessage = "Checking waiting status audio"\n' + retire.END + '\n')
        cleaned = retire.clean_config(text, self.hooks)
        self.assertEqual(cleaned, '[ "hooks" ] # user table\n')
        self.assertEqual(tomllib.loads(cleaned), {"hooks": {}})

    def test_mixed_blocks_still_reject_custom_owned_events_and_cross_marker_scope(self):
        text = historical_config(self.hooks).replace(retire.END, 'other_setting = true\n' + retire.END, 1)
        boundary = text.rfind(retire.END)
        mixed = text[:boundary] + '[hooks.state]\nkeep = true\n[mcp_servers.custom]\ncommand = "server"\n' + text[boundary:]
        owned_header = '[[hooks.PermissionRequest]]\nmatcher = "*"\n'
        managed_event = mixed[mixed.index(owned_header):mixed.index('[hooks.state]')]
        waiting = "/bin/bash " + retire.shlex.quote(str(self.hooks / "speak_waiting.sh"))
        combined = (retire.START + '\nnotify = ["/bin/bash",' + json.dumps(str(self.hooks / "speak_notify.sh")) + ']\n'
                    + owned_header + 'hooks = {type="command",command=' + json.dumps(waiting)
                    + ',timeout=1,statusMessage="Checking waiting status audio"}\n' + retire.END + '\n')
        cases = [mixed.replace('timeout = 1', 'timeout = true'),
                 mixed.replace('timeout = 1', 'timeout = 1.0'),
                 mixed.replace('statusMessage = "Checking waiting status audio"', 'statusMessage = "custom"'),
                 mixed.replace('type = "command"\ncommand = ' + json.dumps("/bin/bash " + retire.shlex.quote(str(self.hooks / "speak_waiting.sh"))),
                               'type = "command"\ncommand = "custom"'),
                 mixed.replace('timeout = 1', 'timeout = 1\ncustom_key = "retain"'),
                 mixed.replace(owned_header, owned_header + 'unknown_owned = "retain"\n'),
                 text[:boundary] + '[hooks.PermissionRequest.extra]\nkeep = true\n' + text[boundary:],
                 text.replace('timeout = 1\n', retire.END + '\ntimeout = 1\n').rsplit(retire.END, 1)[0],
                 mixed.replace('[hooks.state]', managed_event + '[hooks.state]'),
                 combined,
                 combined.replace('hooks = {', 'hooks = [{').replace('}\n', '}]\n')
                         .replace(json.dumps(waiting), '"custom-handler"')
                         .replace('Checking waiting status audio', 'custom status')]
        for candidate in cases:
            with self.subTest(candidate=candidate):
                tomllib.loads(candidate)  # These controls are valid TOML, not syntax failures.
                self.config.write_text(candidate)
                before = self.snapshot()
                backups = self.root / "rollback"
                self.assertEqual(retire.main(["--config", str(self.config), "--profile", str(self.profile),
                                             "--backup-dir", str(backups)]), 1)
                self.assertEqual(self.snapshot(), before)
                self.assertFalse(backups.exists())

    def test_foreign_notify_and_helpers_are_preserved(self):
        text = 'notify = ["custom-notify"]\n[hooks]\ncustom = "pira_play_audio.ps1"\n'
        self.config.write_text(text)
        helper = self.hooks / "pira_play_audio.ps1"
        helper.write_text(PLAY)
        self.assertEqual(retire.plan_retirement(self.config, []), [])
        helper.write_text(PLAY + "\n# Custom extension\n")
        self.config.write_text('notify = ["custom-notify"]\n')
        self.assertEqual(retire.plan_retirement(self.config, []), [])

    def test_malformed_ambiguous_or_unmarked_config_fails_without_changes(self):
        valid = historical_config(self.hooks)
        cases = [retire.END, retire.START, valid.replace(retire.END, "", 1),
                 valid.replace('notify = ', 'custom = ', 1),
                 valid.replace('timeout = 1', 'timeout = 2'),
                 valid.replace('matcher = "*"', 'matcher = "custom"'),
                 valid.replace(retire.START + "\n", "", 1).replace(retire.END + "\n", "", 1),
                 valid + '[[hooks.PermissionRequest.hooks]]\ncommand = "foreign"\n',
                 'not valid toml',
                 retire.START + '\nhooks = 1\n' + retire.END]
        for text in cases:
            with self.subTest(text=text):
                self.config.write_text(text)
                before = self.snapshot()
                self.assertEqual(retire.main(["--config", str(self.config), "--profile", str(self.profile)]), 1)
                self.assertEqual(self.snapshot(), before)

    def test_malformed_profile_prevents_config_publication(self):
        self.config.write_text(historical_config(self.hooks))
        start, end = retire.STARTUP_MARKERS[0]
        for text in (start, end, f'{start}\n{start}\n{end}\n', f'{start}\n{end}\n{start}\n{end}\n'):
            self.profile.write_text(text)
            before = self.snapshot()
            with self.assertRaises(RuntimeError):
                retire.plan_retirement(self.config, [self.profile])
            self.assertEqual(self.snapshot(), before)

    def test_input_change_and_aliases_fail_closed(self):
        self.config.write_text(historical_config(self.hooks))
        plan = retire.plan_retirement(self.config, [])
        self.config.write_text('model = "later change"\n')
        with self.assertRaisesRegex(RuntimeError, "input changed"):
            retire.apply_retirement(plan)
        self.assertFalse(list(self.root.glob("*.bak.*")))
        target = self.root / "target"
        target.write_text(historical_config(self.hooks))
        self.config.unlink()
        try:
            self.config.symlink_to(target)
        except OSError as exc:
            self.skipTest(f"symlinks unavailable: {exc}")
        plan = retire.plan_retirement(self.config, [])
        self.config.unlink()
        replacement = self.root / "replacement"
        replacement.write_text("untouched")
        self.config.symlink_to(replacement)
        with self.assertRaisesRegex(RuntimeError, "input changed"):
            retire.apply_retirement(plan)
        self.assertEqual(target.read_text(), historical_config(self.hooks))
        self.assertEqual(replacement.read_text(), "untouched")
        self.config.unlink()
        retire.os.link(target, self.config)
        with self.assertRaisesRegex(RuntimeError, "Unsafe"):
            retire.plan_retirement(self.config, [])

    def test_stable_config_and_profile_aliases_are_preserved(self):
        target = self.root / "real-config"
        target.write_text(historical_config(self.hooks))
        profile_target = self.root / "real-profile"
        start, end = retire.STARTUP_MARKERS[0]
        profile_target.write_text(f'{start}\nwrapper\n{end}\nkeep\n')
        try:
            self.config.symlink_to(target)
            self.profile.symlink_to(profile_target)
        except OSError as exc:
            self.skipTest(f"symlinks unavailable: {exc}")
        retire.apply_retirement(retire.plan_retirement(self.config, [self.profile]))
        self.assertTrue(self.config.is_symlink())
        self.assertTrue(self.profile.is_symlink())
        self.assertNotIn("notify", tomllib.loads(target.read_text()))
        self.assertEqual(profile_target.read_text(), "keep\n")

    def test_interrupted_publication_replans_and_preserves_backups(self):
        self.config.write_text(historical_config(self.hooks))
        start, end = retire.STARTUP_MARKERS[0]
        self.profile.write_text(f'{start}\nwrapper\n{end}\nkeep\n')
        helper = self.hooks / "pira_play_audio.ps1"
        helper.write_text(PLAY)
        replace = retire.os.replace
        def publish(source, destination):
            if destination == self.profile:
                raise OSError("injected publication interruption")
            return replace(source, destination)
        with patch.object(retire.os, "replace", side_effect=publish):
            with self.assertRaisesRegex(OSError, "interruption"):
                retire.apply_retirement(retire.plan_retirement(self.config, [self.profile]))
        self.assertNotIn("notify", tomllib.loads(self.config.read_text()))
        self.assertTrue(helper.exists())
        backups = {p: p.read_bytes() for p in self.root.glob("*.bak.*")}
        retire.apply_retirement(retire.plan_retirement(self.config, [self.profile]))
        self.assertFalse(helper.exists())
        self.assertEqual(self.profile.read_text(), "keep\n")
        for p, data in backups.items():
            self.assertEqual(p.read_bytes(), data)
        self.assertFalse(list(self.root.glob(".pira-audio-*")))

    def test_windows_profile_bom_encodings_and_command_paths(self):
        from pathlib import PureWindowsPath
        hooks = PureWindowsPath(r"C:\Users\Test User\.codex\hooks")
        shell = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
        self.assertTrue(retire.command_matches([shell, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(hooks / "speak_notify.ps1")], hooks))
        self.assertTrue(retire.command_matches(f'{shell} -NoProfile -ExecutionPolicy Bypass -File "{hooks / "speak_waiting.ps1"}"', hooks, waiting=True))
        start, end = retire.STARTUP_MARKERS[1]
        text = f'custom-before\r\n{start}\r\nold wrapper\r\n{end}\r\ncustom-after\r\n'
        kept = 'custom-before\r\ncustom-after\r\n'
        for prefix, codec in ((b"\xff\xfe", "utf-16-le"), (b"\xfe\xff", "utf-16-be"), (b"\xef\xbb\xbf", "utf-8")):
            with self.subTest(codec=codec):
                original = prefix + text.encode(codec)
                self.profile.write_bytes(original)
                retire.apply_retirement(retire.plan_retirement(self.config, [self.profile]))
                self.assertEqual(self.profile.read_bytes(), prefix + kept.encode(codec))
                self.assertTrue(any(p.read_bytes() == original for p in self.root.glob("profile.pira-audio.bak.*")))

    def test_windows_redirected_documents_profiles(self):
        docs = self.root / "Redirected Documents"
        with patch.object(retire, "os", SimpleNamespace(name="nt")), patch.object(retire.subprocess, "run", return_value=SimpleNamespace(stdout=str(docs))) as query:
            profiles = retire.default_profiles()
        self.assertEqual(profiles, [docs / shell / "Microsoft.PowerShell_profile.ps1" for shell in ("PowerShell", "WindowsPowerShell")])
        self.assertIn("GetFolderPath", query.call_args.args[0][-1])


if __name__ == "__main__":
    unittest.main()
