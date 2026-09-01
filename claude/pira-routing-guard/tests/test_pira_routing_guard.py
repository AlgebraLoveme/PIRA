from __future__ import annotations

import importlib.util
import io
import json
import os
import tempfile
import unittest
from concurrent.futures import ThreadPoolExecutor
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from typing import Any
from unittest.mock import patch

SCRIPT = Path(__file__).parents[1] / "scripts" / "pira_routing_guard.py"
SPEC = importlib.util.spec_from_file_location("pira_routing_guard_test", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
guard = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(guard)


class RoutingGuardTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.state_dir = self.root / "state"
        self.agent_dir = self.root / "agent"
        for relative in guard.MODULE_FILES.values():
            path = self.agent_dir / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(f"policy for {relative}\n", encoding="utf-8")
        self.environment = patch.dict(
            os.environ,
            {
                "PIRA_ROUTING_STATE_DIR": str(self.state_dir),
                "PIRA_AGENT_DIR": str(self.agent_dir),
            },
        )
        self.environment.start()

    def tearDown(self) -> None:
        self.environment.stop()
        self.temporary.cleanup()

    def event(self, name: str, **values: Any) -> dict[str, Any]:
        return {"session_id": "session-1", "hook_event_name": name, **values}

    def route(self, arguments: str, tool_use_id: str = "route-1") -> None:
        decision = guard.dispatch(
            self.event(
                "PreToolUse",
                tool_name="Skill",
                tool_input={"skill": guard.ROUTE_SKILL, "args": arguments},
                tool_use_id=tool_use_id,
            )
        )
        self.assertIsNone(decision)
        loaded = guard.load_selected("session-1", arguments)
        self.assertIsInstance(loaded, str)
        result = guard.dispatch(
            self.event(
                "PostToolUse",
                tool_name="Skill",
                tool_input={"skill": guard.ROUTE_SKILL, "args": arguments},
                tool_use_id=tool_use_id,
            )
        )
        self.assertIsNotNone(result)

    def test_prompt_requires_route_before_tools_or_stop(self) -> None:
        context = guard.dispatch(self.event("UserPromptSubmit", prompt="implement a parser"))
        self.assertIn(guard.ROUTE_SKILL, json.dumps(context))
        denied = guard.dispatch(
            self.event("PreToolUse", tool_name="Read", tool_input={"file_path": "project.py"})
        )
        self.assertEqual(
            denied["hookSpecificOutput"]["permissionDecision"],
            "deny",
        )
        stopped = guard.dispatch(self.event("Stop"))
        self.assertEqual(stopped["decision"], "block")
        self.assertIsNone(guard.dispatch(self.event("Stop")))

    def test_route_expands_dependencies_and_injects_exact_modules(self) -> None:
        guard.dispatch(self.event("UserPromptSubmit", prompt="implement a parser"))
        decision = guard.dispatch(
            self.event(
                "PreToolUse",
                tool_name="Skill",
                tool_input={"skill": guard.ROUTE_SKILL, "args": "coding"},
                tool_use_id="route-1",
            )
        )
        self.assertIsNone(decision)
        state = guard.SessionState("session-1").read()
        self.assertEqual(state["required"], ["research", "coding"])
        injected = guard.load_selected("session-1", "coding")
        self.assertIn("Loaded PIRA module: research", injected)
        self.assertIn("Loaded PIRA module: coding", injected)
        guard.dispatch(
            self.event(
                "PostToolUse",
                tool_name="Skill",
                tool_input={"skill": guard.ROUTE_SKILL, "args": "coding"},
                tool_use_id="route-1",
            )
        )
        self.assertIsNone(
            guard.dispatch(self.event("PreToolUse", tool_name="Bash", tool_input={"command": "git status"}))
        )
        self.assertIsNone(guard.dispatch(self.event("Stop")))

    def test_loaded_modules_are_reused_until_changed_or_compacted(self) -> None:
        guard.dispatch(self.event("UserPromptSubmit", prompt="implement"))
        self.route("coding")

        guard.dispatch(self.event("UserPromptSubmit", prompt="continue implementation"))
        decision = guard.dispatch(
            self.event(
                "PreToolUse",
                tool_name="Skill",
                tool_input={"skill": guard.ROUTE_SKILL, "args": "coding"},
                tool_use_id="route-2",
            )
        )
        self.assertIsNone(decision)
        self.assertIn("already loaded", guard.load_selected("session-1", "coding"))
        guard.dispatch(
            self.event(
                "PostToolUse",
                tool_name="Skill",
                tool_input={"skill": guard.ROUTE_SKILL, "args": "coding"},
                tool_use_id="route-2",
            )
        )
        self.assertIsNone(
            guard.dispatch(self.event("PreToolUse", tool_name="Bash", tool_input={"command": "git status"}))
        )

        coding = self.agent_dir / guard.MODULE_FILES["coding"]
        coding.write_text("changed policy\n", encoding="utf-8")
        denied = guard.dispatch(
            self.event("PreToolUse", tool_name="Bash", tool_input={"command": "git status"})
        )
        self.assertIn("coding", denied["hookSpecificOutput"]["permissionDecisionReason"])

        self.route("coding", tool_use_id="route-3")
        guard.dispatch(self.event("PostCompact"))
        denied_after_compact = guard.dispatch(
            self.event("PreToolUse", tool_name="Bash", tool_input={"command": "git status"})
        )
        reason = denied_after_compact["hookSpecificOutput"]["permissionDecisionReason"]
        self.assertIn(guard.ROUTE_SKILL, reason)

    def test_followup_adds_new_modules_and_session_restart_requires_fresh_route(self) -> None:
        guard.dispatch(self.event("UserPromptSubmit", prompt="review source code"))
        self.route("coding")

        guard.dispatch(self.event("UserPromptSubmit", prompt="polish the technical explanation"))
        self.assertIsNone(
            guard.dispatch(
                self.event(
                    "PreToolUse",
                    tool_name="Skill",
                    tool_input={"skill": guard.ROUTE_SKILL, "args": "writing"},
                    tool_use_id="route-2",
                )
            )
        )
        injected = guard.load_selected("session-1", "writing")
        self.assertNotIn("Loaded PIRA module: research", injected)
        self.assertIn("Loaded PIRA module: writing", injected)
        guard.dispatch(
            self.event(
                "PostToolUse",
                tool_name="Skill",
                tool_input={"skill": guard.ROUTE_SKILL, "args": "writing"},
                tool_use_id="route-2",
            )
        )
        self.assertIsNone(
            guard.dispatch(self.event("PreToolUse", tool_name="Read", tool_input={"file_path": "draft.md"}))
        )

        context = guard.dispatch(self.event("SessionStart", source="resume"))
        self.assertIn(guard.ROUTE_SKILL, json.dumps(context))
        denied = guard.dispatch(
            self.event("PreToolUse", tool_name="Read", tool_input={"file_path": "draft.md"})
        )
        self.assertEqual(denied["hookSpecificOutput"]["permissionDecision"], "deny")

    def test_skill_completion_without_loader_does_not_unlock_tools(self) -> None:
        guard.dispatch(self.event("UserPromptSubmit", prompt="implement"))
        self.assertIsNone(
            guard.dispatch(
                self.event(
                    "PreToolUse",
                    tool_name="Skill",
                    tool_input={"skill": guard.ROUTE_SKILL, "args": "coding"},
                    tool_use_id="route-1",
                )
            )
        )
        result = guard.dispatch(
            self.event(
                "PostToolUse",
                tool_name="Skill",
                tool_input={"skill": guard.ROUTE_SKILL, "args": "coding"},
                tool_use_id="route-1",
            )
        )
        self.assertIn("has not completed", json.dumps(result))
        denied = guard.dispatch(
            self.event("PreToolUse", tool_name="Bash", tool_input={"command": "git status"})
        )
        self.assertEqual(denied["hookSpecificOutput"]["permissionDecision"], "deny")

    def test_loader_confirms_only_after_commit(self) -> None:
        guard.dispatch(self.event("UserPromptSubmit", prompt="implement"))
        self.assertIsNone(
            guard.dispatch(
                self.event(
                    "PreToolUse",
                    tool_name="Skill",
                    tool_input={"skill": guard.ROUTE_SKILL, "args": "coding"},
                    tool_use_id="route-1",
                )
            )
        )
        rendered, session, state, pending = guard.prepare_selected("session-1", "coding")
        self.assertIn("Loaded PIRA module", rendered)
        self.assertFalse(session.is_confirmed(state))
        self.assertEqual(len(pending), 2)
        guard.commit_selected(session, state, pending)
        self.assertTrue(session.is_confirmed(state))

    def test_none_route_allows_answer_without_module_reads(self) -> None:
        guard.dispatch(self.event("UserPromptSubmit", prompt="say hello"))
        self.route("none")
        self.assertIsNone(guard.dispatch(self.event("Stop")))

    def test_invalid_routes_are_denied(self) -> None:
        for arguments in ("", "none coding", "unknown"):
            with self.subTest(arguments=arguments):
                guard.dispatch(self.event("UserPromptSubmit", prompt="test"))
                denied = guard.dispatch(
                    self.event(
                        "PreToolUse",
                        tool_name="Skill",
                        tool_input={"skill": guard.ROUTE_SKILL, "args": arguments},
                        tool_use_id="bad-route",
                    )
                )
                self.assertEqual(denied["hookSpecificOutput"]["permissionDecision"], "deny")

    def test_subagent_route_is_isolated_and_required(self) -> None:
        context = guard.dispatch(
            self.event("SubagentStart", agent_id="subagent-1", agent_type="Explore")
        )
        self.assertIn(guard.ROUTE_SKILL, json.dumps(context))
        denied = guard.dispatch(
            self.event(
                "PreToolUse",
                agent_id="subagent-1",
                tool_name="Bash",
                tool_input={"command": "git status"},
            )
        )
        self.assertEqual(denied["hookSpecificOutput"]["permissionDecision"], "deny")

        decision = guard.dispatch(
            self.event(
                "PreToolUse",
                agent_id="subagent-1",
                tool_name="Skill",
                tool_input={"skill": guard.ROUTE_SKILL, "args": "coding"},
                tool_use_id="sub-route",
            )
        )
        output = decision["hookSpecificOutput"]
        self.assertEqual(output["permissionDecision"], "allow")
        scoped_arguments = output["updatedInput"]["args"]
        self.assertIn(guard.SCOPE_PREFIX, scoped_arguments)
        injected = guard.load_selected("session-1", scoped_arguments)
        self.assertIn("Loaded PIRA module: coding", injected)
        guard.dispatch(
            self.event(
                "PostToolUse",
                agent_id="subagent-1",
                tool_name="Skill",
                tool_input=output["updatedInput"],
                tool_use_id="sub-route",
            )
        )
        self.assertIsNone(
            guard.dispatch(
                self.event(
                    "PreToolUse",
                    agent_id="subagent-1",
                    tool_name="Bash",
                    tool_input={"command": "git status"},
                )
            )
        )
        parent = guard.dispatch(
            self.event("PreToolUse", tool_name="Bash", tool_input={"command": "git status"})
        )
        self.assertEqual(parent["hookSpecificOutput"]["permissionDecision"], "deny")
        self.assertIsNone(guard.dispatch(self.event("SubagentStop", agent_id="subagent-1")))

    def test_concurrent_subagents_keep_routes_and_parent_state_isolated(self) -> None:
        guard.dispatch(self.event("UserPromptSubmit", prompt="review source"))
        self.route("coding")
        parent = guard.SessionState("session-1")
        parent_before = parent.state_path.read_bytes()

        def route_agent(agent_id: str, arguments: str) -> tuple[str, list[str]]:
            guard.dispatch(
                self.event("SubagentStart", agent_id=agent_id, agent_type="general-purpose")
            )
            decision = guard.dispatch(
                self.event(
                    "PreToolUse",
                    agent_id=agent_id,
                    tool_name="Skill",
                    tool_input={"skill": guard.ROUTE_SKILL, "args": arguments},
                    tool_use_id=f"route-{agent_id}",
                )
            )
            scoped = decision["hookSpecificOutput"]["updatedInput"]["args"]
            guard.load_selected("session-1", scoped)
            guard.dispatch(
                self.event(
                    "PostToolUse",
                    agent_id=agent_id,
                    tool_name="Skill",
                    tool_input={"skill": guard.ROUTE_SKILL, "args": scoped},
                    tool_use_id=f"route-{agent_id}",
                )
            )
            state = guard.SessionState("session-1", agent_id=agent_id).read()
            return agent_id, state["required"]

        with ThreadPoolExecutor(max_workers=2) as executor:
            pairs = (("agent-a", "coding"), ("agent-b", "guidance"))
            results = dict(executor.map(lambda pair: route_agent(*pair), pairs))

        self.assertEqual(results["agent-a"], ["research", "coding"])
        self.assertEqual(results["agent-b"], ["guidance"])
        self.assertEqual(parent.state_path.read_bytes(), parent_before)

    def test_unrouted_subagent_stop_retry_is_bounded(self) -> None:
        guard.dispatch(self.event("SubagentStart", agent_id="subagent-1", agent_type="Explore"))
        first = guard.dispatch(self.event("SubagentStop", agent_id="subagent-1"))
        self.assertEqual(first["decision"], "block")
        self.assertIsNone(guard.dispatch(self.event("SubagentStop", agent_id="subagent-1")))

    def test_corrupt_state_fails_closed_and_route_recovers(self) -> None:
        session = guard.SessionState("session-1")
        session.directory.mkdir(parents=True)
        session.state_path.write_text("{not-json", encoding="utf-8")
        denied = guard.dispatch(
            self.event("PreToolUse", tool_name="Read", tool_input={"file_path": "project.py"})
        )
        self.assertEqual(denied["hookSpecificOutput"]["permissionDecision"], "deny")
        self.route("coding")
        self.assertIsNone(
            guard.dispatch(
                self.event("PreToolUse", tool_name="Read", tool_input={"file_path": "project.py"})
            )
        )

    def test_main_reports_malformed_input_without_traceback(self) -> None:
        with (
            patch.object(guard.sys, "stdin", io.StringIO("not-json")),
            redirect_stdout(io.StringIO()) as stdout,
            redirect_stderr(io.StringIO()) as stderr,
        ):
            result = guard.main()
        self.assertEqual(result, 1)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("PIRA routing guard error", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
