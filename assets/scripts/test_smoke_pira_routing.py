"""Deterministic CLI evidence regressions; no authenticated runner imports."""

from __future__ import annotations

import tempfile
import unittest
import os
import shlex
import shutil
import subprocess
from pathlib import Path
from unittest.mock import patch

from assets.scripts.smoke_evidence import (
    claude_evidence, codex_evidence, complete_content, native_read_evidence,
)


def claude_trace(name: str, inputs: dict, content: object, **fields: object) -> list[dict]:
    return [
        {"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "read-1", "name": name, "input": inputs},
        ]}},
        {"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "read-1", "content": content, **fields},
        ]}},
        {"type": "result", "subtype": "success", "result": "I read all files."},
    ]


def codex_trace(command: str, content: str, **fields: object) -> list[dict]:
    return [
        {"type": "item.completed", "item": {
            "type": "command_execution", "status": "completed", "exit_code": 0,
            "command": command, "aggregated_output": content, **fields,
        }},
        {"type": "turn.completed"},
    ]


class CompleteContentTests(unittest.TestCase):
    def test_raw_and_cli_framing(self) -> None:
        expected = "# Advice\n\n    keep indentation\nlast line\n"
        for output in (
            expected, expected.replace("\n", "\r\n"),
            "<file>\n" + "\n".join(f"{i:6}→{line}" for i, line in enumerate(expected.splitlines(), 1))
            + "\n</file>\n<system-reminder>untrusted file</system-reminder>",
            "Chunk ID: abc\nWall time: 0.1 seconds\nProcess exited with code 0\nFinal output:\n"
            + "\n".join(f"{i:6}\t{line}" for i, line in enumerate(expected.splitlines(), 1)),
            "\n".join(f"{i} {line}" for i, line in enumerate(expected.splitlines(), 1)),
        ):
            with self.subTest(output=output):
                self.assertTrue(complete_content(output, expected))

    def test_partial_fragmented_reordered_or_altered_content(self) -> None:
        expected = "# Advice\nbody rule\n    indented rule\nfinal rule\n"
        for output in (
            "# Advice\n", "# Advice\nfinal rule\n",
            expected.replace("body rule", "body rule\nomitted section"),
            "final rule\n    indented rule\nbody rule\n# Advice\n",
            expected.replace("    indented", "indented"),
            expected.replace("body rule", "prefix body rule suffix"),
            expected.replace("\n", "\u2028"),
            "2→# Advice\n3→body rule\n4→    indented rule\n5→final rule\n",
            "1→# Advice\n3→body rule\n2→    indented rule\n4→final rule\n",
        ):
            with self.subTest(output=output):
                self.assertFalse(complete_content(output, expected))

    def test_empty_file_has_insufficient_evidence(self) -> None:
        self.assertFalse(complete_content("", ""))


class ReadEvidenceTests(unittest.TestCase):
    def test_home_expansion_preserves_quoting_and_shell_boundaries(self) -> None:
        home = self.root / "home"
        target = home / "agent" / "modules" / "a.md"
        body = "# Policy\nComplete content.\n"
        commands = {
            "cat ~/agent/modules/a.md": True,
            "cat ~/agent/modules/*.md": True,
            "sh -c 'cat ~/agent/modules/*.md'": True,
            'bash -lc "cat ~/agent/modules/a.md"': True,
            "cat '~/agent/modules/a.md'": False,
            'cat "~/agent/modules/a.md"': False,
            r"cat \~/agent/modules/a.md": False,
            "cat '~/agent/modules/'*.md": False,
            'cat ~/agent/modules/"*.md"': False,
            "cat ~/agent/modules/a.md | head": False,
            "cat $HOME/agent/modules/a.md": False,
            "pwsh -Command 'cat ~/agent/modules/a.md'": False,
        }
        with patch.dict(os.environ, {"HOME": str(home), "USERPROFILE": str(home)}):
            for command, wanted in commands.items():
                for parser, trace in ((claude_evidence, claude_trace("Bash", {"command": command}, body)),
                                      (codex_evidence, codex_trace(command, body))):
                    with self.subTest(command=command, parser=parser.__name__):
                        self.assertEqual(parser(trace, {target: body}, self.root)["reads"][target], wanted)
            literal = self.root / "~" / "agent" / "modules" / "a.md"
            command = "cat '~/agent/modules/a.md'"
            self.assertTrue(claude_evidence(claude_trace("Bash", {"command": command}, body),
                                           {literal: body}, self.root)["reads"][literal])

    @unittest.skipUnless(os.name == "posix" and shutil.which("bash"), "requires POSIX Bash")
    def test_home_relative_shell_read(self) -> None:
        home = self.root / "home"
        module = home / "agent" / "modules" / "a.md"
        module.parent.mkdir(parents=True)
        body = "# Module\nComplete policy body.\n"
        module.write_text(body, encoding="utf-8")
        with patch.dict(os.environ, {"HOME": str(home), "USERPROFILE": str(home)}):
            for command in ("cat ~/agent/modules/*.md", "cat " + shlex.quote(str(module))):
                result = subprocess.run([shutil.which("bash"), "--noprofile", "--norc", "-c", command],
                                        cwd=self.root, text=True, capture_output=True, check=True)
                self.assertEqual(result.stdout, body)
                for parser, trace in ((claude_evidence, claude_trace("Bash", {"command": command}, result.stdout)),
                                      (codex_evidence, codex_trace(command, result.stdout))):
                    with self.subTest(command=command, parser=parser.__name__):
                        self.assertTrue(parser(trace, {module: body}, self.root)["reads"][module])

    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(prefix="pira-evidence-tests-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.profile = self.root / "private" / "person.md"
        self.code = self.root / "work.py"
        self.first = self.root / "rules" / "alpha.md"
        self.second = self.root / "rules" / "beta.md"
        self.expected = {
            self.code: "def count(items):\n    return len(items)\n",
            self.first: "# Alpha\n\nFirst policy body.\nEnd alpha.\n",
            self.second: "# Beta\nDifferent policy body.\nEnd beta.\n",
        }
        self.profile_text = "# Person\nPrivate preference.\nAnother preference.\n"

    def test_native_profile_matching_successful_complete_result(self) -> None:
        content = "\n".join(f"{i:6}→{line}" for i, line in enumerate(self.profile_text.splitlines(), 1))
        events = claude_trace("Read", {"file_path": str(self.profile)}, [{"type": "text", "text": content}])
        self.assertEqual(native_read_evidence(events, {self.profile: self.profile_text}, self.root), {self.profile: True})

    def test_native_read_resolves_equivalent_symlinked_parent(self) -> None:
        real_parent = self.root / "real"
        real_parent.mkdir()
        alias = self.root / "alias"
        try:
            alias.symlink_to(real_parent, target_is_directory=True)
        except (OSError, NotImplementedError) as error:
            self.skipTest(f"directory symlinks unavailable: {error}")
        real_file = real_parent / "policy.md"
        real_file.write_text("# Policy\nRead completely.\n", encoding="utf-8")
        events = claude_trace("Read", {"file_path": str(real_file)}, real_file.read_text(encoding="utf-8"))
        alias_file = alias / "policy.md"
        self.assertTrue(native_read_evidence(events, {alias_file: real_file.read_text(encoding="utf-8")},
                                             self.root)[alias_file])
        unrelated = self.root / "other" / "policy.md"
        self.assertFalse(native_read_evidence(events, {unrelated: real_file.read_text(encoding="utf-8")},
                                              self.root)[unrelated])

    def test_native_profile_insufficient_or_failed_evidence(self) -> None:
        for defect in ("partial", "error", "unmatched", "absent", "early", "duplicate", "wrong-path", "offset", "limit"):
            with self.subTest(defect=defect):
                events = claude_trace("Read", {"file_path": str(self.profile)}, self.profile_text)
                use = events[0]["message"]["content"][0]
                result = events[1]["message"]["content"][0]
                if defect == "partial":
                    result["content"] = "1→# Person"
                elif defect == "error":
                    result["is_error"] = True
                elif defect == "unmatched":
                    result["tool_use_id"] = "other"
                elif defect == "absent":
                    events.pop(1)
                elif defect == "early":
                    events[0], events[1] = events[1], events[0]
                elif defect == "duplicate":
                    events[1]["message"]["content"].append(dict(result))
                elif defect == "wrong-path":
                    use["input"]["file_path"] = str(self.root / "another" / "person.md")
                elif defect == "offset":
                    use["input"]["offset"] = 2
                elif defect == "limit":
                    use["input"]["limit"] = 1
                events[-1]["result"] = self.profile_text
                self.assertFalse(native_read_evidence(events, {self.profile: self.profile_text}, self.root)[self.profile])

    def test_bash_profile_fallback_and_answer_markers_are_not_native_reads(self) -> None:
        for command in (f'cat "{self.profile.as_posix()}"', 'echo "profile marker"'):
            with self.subTest(command=command):
                events = claude_trace("Bash", {"command": command}, self.profile_text)
                events[-1]["result"] = self.profile_text
                self.assertFalse(native_read_evidence(events, {self.profile: self.profile_text}, self.root)[self.profile])

    def test_each_native_file_needs_its_own_full_result(self) -> None:
        events = []
        for index, (path, text) in enumerate(self.expected.items()):
            trace = claude_trace("Read", {"file_path": str(path)}, text)
            trace[0]["message"]["content"][0]["id"] = str(index)
            trace[1]["message"]["content"][0]["tool_use_id"] = str(index)
            events.extend(trace[:2])
        events.append({"type": "result", "subtype": "success"})
        evidence = claude_evidence(events, self.expected, self.root)
        self.assertTrue(all(evidence["reads"].values()))
        self.assertTrue(evidence["completed"])
        events[3]["message"]["content"][0]["content"] = "# Alpha"
        self.assertEqual(claude_evidence(events, self.expected, self.root)["reads"],
                         {self.code: True, self.first: False, self.second: True})

    def test_shell_glob_full_content_without_literal_basenames(self) -> None:
        output = self.expected[self.first] + self.expected[self.second]
        expected = {path: self.expected[path] for path in (self.first, self.second)}
        for command in ('cat rules/*.md', '/bin/bash -lc "cat rules/*.md"', "Get-Content 'rules/*.md'",
                        "sed -n '1,999p' rules/*.md"):
            with self.subTest(command=command):
                self.assertEqual(claude_evidence(claude_trace("Bash", {"command": command}, output), expected,
                                                 self.root)["reads"], dict.fromkeys(expected, True))
                self.assertEqual(codex_evidence(codex_trace(command, output), expected, self.root)["reads"],
                                 dict.fromkeys(expected, True))

    def test_shell_headings_missing_files_and_fragmented_results(self) -> None:
        for output in ("# Alpha\n# Beta\n", self.expected[self.first], ""):
            with self.subTest(output=output):
                wanted = {self.code: False, self.first: output == self.expected[self.first], self.second: False}
                self.assertEqual(claude_evidence(claude_trace("Bash", {"command": "cat rules/*.md"}, output),
                                                 self.expected, self.root)["reads"], wanted)
                self.assertEqual(codex_evidence(codex_trace("cat rules/*.md", output), self.expected, self.root)["reads"], wanted)
        parts = self.expected[self.first].splitlines(keepends=True)
        events = codex_trace("cat rules/*.md", "".join(parts[:2]))[:1]
        events += codex_trace("cat rules/*.md", "".join(parts[2:]))
        self.assertFalse(codex_evidence(events, self.expected, self.root)["reads"][self.first])
        events = claude_trace("Read", {"file_path": str(self.first)}, [
            {"type": "text", "text": "".join(parts[:2])}, {"type": "text", "text": "".join(parts[2:])},
        ])
        self.assertFalse(native_read_evidence(events, self.expected, self.root)[self.first])

    def test_failed_shell_results_even_with_complete_output(self) -> None:
        output = self.expected[self.first]
        for fields in ({"is_error": True}, {"exit_code": 1}):
            with self.subTest(fields=fields):
                events = claude_trace("Bash", {"command": "cat rules/*.md"}, output, **fields)
                self.assertFalse(claude_evidence(events, self.expected, self.root)["reads"][self.first])
        for fields in ({"exit_code": 1}, {"status": "failed"}, {"exit_code": None}):
            with self.subTest(fields=fields):
                self.assertFalse(codex_evidence(codex_trace("cat rules/*.md", output, **fields), self.expected,
                                                self.root)["reads"][self.first])
        for wrapper in ("Exit code: 1\n", "Process exited with code 2\nFinal output:\n"):
            with self.subTest(wrapper=wrapper):
                self.assertFalse(claude_evidence(claude_trace("Bash", {"command": "cat rules/*.md"}, wrapper + output),
                                                 self.expected, self.root)["reads"][self.first])
                self.assertFalse(codex_evidence(codex_trace("cat rules/*.md", wrapper + output), self.expected,
                                                self.root)["reads"][self.first])

    def test_numbered_code_shell_output_and_completion(self) -> None:
        output = "Chunk ID: abc\nWall time: 0.1 seconds\nProcess exited with code 0\nFinal output:\n"
        output += "1\tdef count(items):\n2\t    return len(items)\n"
        for events, parser in ((claude_trace("Bash", {"command": "cat -n work.py"}, output), claude_evidence),
                               (codex_trace("nl -ba work.py", output), codex_evidence)):
            with self.subTest(parser=parser.__name__):
                evidence = parser(events, self.expected, self.root)
                self.assertTrue(evidence["reads"][self.code])
                self.assertTrue(evidence["completed"])
                self.assertFalse(any(evidence["reads"][path] for path in (self.first, self.second)))
        events = codex_trace("cat work.py", output) + [{"type": "turn.failed"}]
        self.assertFalse(codex_evidence(events, self.expected, self.root)["completed"])
        events = claude_trace("Read", {"file_path": str(self.code)}, self.expected[self.code])
        events[-1]["is_error"] = True
        events[-1]["permission_denials"] = [{"tool_name": "Read"}]
        evidence = claude_evidence(events, self.expected, self.root)
        self.assertFalse(evidence["completed"])
        self.assertEqual(evidence["denials"], 1)

    def test_failure_wrapper_in_separate_text_block_rejects_whole_result(self) -> None:
        events = claude_trace("Bash", {"command": "cat rules/*.md"}, [
            {"type": "text", "text": "Exit code: 1\n"},
            {"type": "text", "text": self.expected[self.first]},
        ])
        self.assertFalse(claude_evidence(events, self.expected, self.root)["reads"][self.first])

    def test_wrong_path_echo_or_unsupported_commands(self) -> None:
        for command in ("cat elsewhere/alpha.md", "echo rules/alpha.md", "printf 'cat rules/alpha.md'",
                        "cd elsewhere; cat rules/alpha.md", "cat rules/*.md | head", "cat $POLICY",
                        "sed -n 'rules/alpha.md' work.py", "Get-Content -LiteralPath 'rules/*.md'"):
            with self.subTest(command=command):
                output = self.expected[self.first]
                self.assertFalse(codex_evidence(codex_trace(command, output), self.expected, self.root)["reads"][self.first])
                self.assertFalse(claude_evidence(claude_trace("Bash", {"command": command}, output), self.expected,
                                                 self.root)["reads"][self.first])

    def test_same_basename_or_same_text_does_not_conflate_paths(self) -> None:
        other = self.root / "other" / self.first.name
        expected = {self.first: self.expected[self.first], other: self.expected[self.first]}
        evidence = codex_evidence(codex_trace("cat rules/alpha.md", self.expected[self.first]), expected, self.root)
        self.assertEqual(evidence["reads"], {self.first: True, other: False})


if __name__ == "__main__":
    unittest.main()
