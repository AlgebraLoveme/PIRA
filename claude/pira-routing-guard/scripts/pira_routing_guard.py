#!/usr/bin/env python3
"""Claude Code hook state machine for the opt-in PIRA routing pilot."""

from __future__ import annotations

import hashlib
import json
import os
import re
import secrets
import sys
import tempfile
from pathlib import Path
from typing import Any

ROUTE_SKILL = "pira-routing-guard:route"
MAX_STOP_BLOCKS = 1
SCOPE_PREFIX = "--pira-scope="
SCOPE_PATTERN = re.compile(r"[0-9a-f]{64}")
MODULE_ORDER = (
    "user_profile",
    "research",
    "paper_reading",
    "coding",
    "writing",
    "public_figure",
    "explain",
    "guidance",
    "maintenance",
)
MODULE_FILES = {
    "user_profile": "USER.md",
    "research": "modules/RESEARCH_POLICY.md",
    "paper_reading": "modules/PAPER_READING.md",
    "coding": "modules/CODING_STYLE.md",
    "writing": "modules/SCIENTIFIC_WRITING.md",
    "public_figure": "modules/PUBLIC_FIGURE_STYLE.md",
    "explain": "modules/EXPLAIN_STYLE.md",
    "guidance": "modules/GUIDANCE.md",
    "maintenance": "modules/MAINTENANCE.md",
}
IMPLIED = {
    "paper_reading": {"research"},
    "coding": {"research"},
    "writing": {"research"},
    "public_figure": {"research"},
}
ADAPTIVE_MODE_ENV = "PIRA_ROUTING_GUARD_MODE"
ADAPTIVE_MAX_MODULES = 4
# Cue lexicon for the opt-in adaptive mode, derived from the module descriptions in AGENTS.md.
# A cue that is ambiguous between modules maps to the union of the plausible modules, so the
# lexicon can over-select but is never used to exclude a module. English patterns are
# word-bounded and case-insensitive; Chinese patterns are substrings.
ADAPTIVE_CUES: tuple[tuple[str, frozenset[str]], ...] = (
    (
        r"\bmy (?:stored |saved )?(?:preferences?|profile|background|communication (?:style|preferences?)|"
        r"learning (?:needs|style|level))\b|\b(?:know about me|on my behalf|personali[sz]e\w*|"
        r"tailored? (?:to|for) me)\b|我的(?:偏好|背景|资料|档案|个人信息)|个性化|替我|以我的名义|了解我的",
        frozenset({"user_profile"}),
    ),
    (
        r"\b(?:evidence|verif(?:y|ied|ies|ication)|fact.?check\w*|corroborat\w*|sources?|citations?|"
        r"cite[sd]?|literature|systematic review|meta.?analysis|claims?|better supported|assess\w*|"
        r"investigat\w*)\b|证据|核实|查证|验证|来源|出处|文献|引用|调研|事实核查|可信|综述",
        frozenset({"research"}),
    ),
    (
        r"\b(?:papers?|preprints?|arxiv|excerpts?|th(?:is|e) (?:article|study)|summari[sz]e\w*|summary|"
        r"critique)\b|论文|文章|预印本|这篇|总结|概括|精读|解读",
        frozenset({"paper_reading"}),
    ),
    (r"\babstract\b|摘要", frozenset({"paper_reading", "writing"})),
    (
        r"\b(?:code|coding|codebase|scripts?|functions?|bugs?|debug\w*|implement\w*|refactor\w*|compil\w*|"
        r"tests?|testing|traceback|stack ?trace|exceptions?|errors?|regex|api|library|libraries|packages?|"
        r"repo(?:sitory|sitories)?|commits?|git|pull request|diff|patch|python|rust|java(?:script)?|typescript|"
        r"golang|bash|shell|sql|c\+\+|c#|html|css|docker|dependenc(?:y|ies))\b|"
        r"\.(?:py|rs|js|ts|tsx|jsx|go|java|c|cpp|h|hpp|cs|sh|ps1|sql|ipynb|toml|yaml|yml)\b|"
        r"代码|脚本|函数|调试|报错|异常|实现|重构|编译|测试|仓库|提交|依赖|接口|编程|程序|抛出|崩溃",
        frozenset({"coding"}),
    ),
    (r"\b(?:write|writing|written)\b|写", frozenset({"writing", "coding"})),
    (
        r"\b(?:draft\w*|polish\w*|proofread\w*|rewrite|reword\w*|rephrase|paraphrase|prose|introduction|"
        r"related work|conclusion|manuscript|rebuttal|reviewers?|cover letter|grammar|wording|readability|"
        r"academic (?:english|writing)|scientific writing|technical writing|latex)\b|\.(?:tex|bib)\b|"
        r"撰写|起草|润色|改写|措辞|引言|结论|相关工作|审稿|语法|学术英文|学术写作|科技写作|写作",
        frozenset({"writing"}),
    ),
    (
        r"\b(?:figures?|plots?|plotting|charts?|graphs?|diagrams?|schematics?|tikz|svg|posters?|slides?|"
        r"infographics?|visuali[sz]\w*|histograms?|heatmaps?|axis|axes|legend|colou?r ?(?:map|scheme|palette)|"
        r"dpi|png|matplotlib|ggplot|seaborn)\b|图表|配图|绘图|画图|作图|示意图|海报|幻灯片|可视化|图片|图像|插图|"
        r"柱状图|折线图|热图|散点图|配色",
        frozenset({"public_figure", "coding"}),
    ),
    (
        r"\b(?:explain\w*|explanation|why|intuition|intuitive(?:ly)?|walk me through|teach|difference between|"
        r"clarify|in simple terms|understand)\b|解释|为什么|为何|原理|讲解|讲讲|通俗|区别|直观|教我|看不懂|"
        r"不理解|什么意思|怎么理解|是什么",
        frozenset({"explain"}),
    ),
    (
        r"\b(?:overwhelm\w*|overload\w*|practical|stress\w*|anxious|anxiety|burn\w*out|motivat\w*|procrastinat\w*|"
        r"feel(?:ing)? (?:stuck|lost|down|tired|behind)|advice|should i|cope|coping|work.life|habits?|routines?|"
        r"chores?|career|relationship|advisor|supervisor|mentor|colleague|conflict with|time management)\b|"
        r"焦虑|压力|迷茫|拖延|动力|情绪|心累|怎么办|该不该|要不要|如何应对|平衡|习惯|导师|同事|职业|人际",
        frozenset({"guidance"}),
    ),
    (
        r"\bpira\b|agents\.md|user\.md|claude\.md|\.claude[\\/]pira|~[\\/]agent\b|"
        r"\b(?:instruction files?|policy files?|routing (?:rules?|policy|guard)|module[- ]loading)\b|"
        r"指令文件|路由规则|模块规则|策略文件|模块加载",
        frozenset({"maintenance"}),
    ),
)


def emit(value: dict[str, Any] | None = None) -> None:
    if value:
        print(json.dumps(value, ensure_ascii=False, separators=(",", ":")))


def hook_context(event: str, message: str) -> dict[str, Any]:
    return {
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": message,
        }
    }


def deny_tool(reason: str) -> dict[str, Any]:
    return {
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    }


def block_stop(reason: str) -> dict[str, Any]:
    return {"decision": "block", "reason": reason}


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def normalized_path(value: str) -> Path:
    expanded = os.path.expanduser(value)
    if os.name == "nt":
        match = re.match(r"^/([A-Za-z])/(.*)$", expanded)
        if match:
            expanded = f"{match.group(1)}:/{match.group(2)}"
    return Path(os.path.abspath(expanded))


def selected_modules(arguments: str) -> tuple[list[str] | None, str | None]:
    tokens = [token for token in re.split(r"[\s,]+", arguments.strip().lower()) if token]
    tokens = [token.replace("-", "_") for token in tokens]
    if not tokens:
        return None, "Route arguments are required; use module names or `none`."
    if "none" in tokens:
        if len(tokens) != 1:
            return None, "`none` cannot be combined with PIRA modules."
        return [], None
    unknown = sorted(set(tokens) - set(MODULE_FILES))
    if unknown:
        return None, "Unknown PIRA module name(s): " + ", ".join(unknown)
    expanded = set(tokens)
    for token in tuple(expanded):
        expanded.update(IMPLIED.get(token, set()))
    return [name for name in MODULE_ORDER if name in expanded], None


def skill_call(tool_input: dict[str, Any]) -> tuple[str, str]:
    raw_name = next(
        (tool_input.get(key) for key in ("skill", "name", "command") if isinstance(tool_input.get(key), str)),
        "",
    ).strip()
    raw_args = next(
        (tool_input.get(key) for key in ("args", "arguments") if tool_input.get(key) is not None),
        "",
    )
    if isinstance(raw_args, list):
        arguments = " ".join(str(value) for value in raw_args)
    else:
        arguments = str(raw_args).strip()
    if " " in raw_name:
        raw_name, embedded = raw_name.split(None, 1)
        arguments = " ".join(part for part in (embedded, arguments) if part)
    return raw_name, arguments


def split_scope(arguments: str) -> tuple[str, str | None, str | None]:
    tokens = [token for token in arguments.split() if token]
    scoped = [token[len(SCOPE_PREFIX) :] for token in tokens if token.startswith(SCOPE_PREFIX)]
    if len(scoped) > 1 or (scoped and not SCOPE_PATTERN.fullmatch(scoped[0])):
        return arguments, None, "Invalid internal PIRA route scope metadata."
    clean = " ".join(token for token in tokens if not token.startswith(SCOPE_PREFIX))
    return clean, scoped[0] if scoped else None, None


class SessionState:
    def __init__(self, session_id: str, agent_id: str | None = None, scope_key: str | None = None) -> None:
        root_override = os.environ.get("PIRA_ROUTING_STATE_DIR")
        root = Path(root_override) if root_override else Path(tempfile.gettempdir()) / "pira-claude-routing"
        if scope_key is not None:
            if not SCOPE_PATTERN.fullmatch(scope_key):
                raise ValueError("invalid PIRA route scope")
            self.scope_key = scope_key
        else:
            identity = session_id if agent_id is None else f"{session_id}\0{agent_id}"
            self.scope_key = hashlib.sha256(identity.encode("utf-8")).hexdigest()
        self.directory = root / self.scope_key
        self.state_path = self.directory / "route.json"

    def read(self) -> dict[str, Any] | None:
        """Return the persisted state as a dict with sane counters, or None when absent.

        Undecodable or non-object content is reported as corrupt so callers fail closed;
        malformed retry counters are reset so bounded retries stay bounded instead of raising.
        """
        try:
            loaded = json.loads(self.state_path.read_text(encoding="utf-8"))
        except FileNotFoundError:
            return None
        except (json.JSONDecodeError, UnicodeError, OSError):
            loaded = None
        if not isinstance(loaded, dict):
            return {"status": "corrupt", "stop_blocks": 0}
        for counter in ("stop_blocks", "tool_denials"):
            if counter in loaded and not (type(loaded[counter]) is int and loaded[counter] >= 0):
                loaded[counter] = 0
        return loaded

    def write(self, state: dict[str, Any]) -> None:
        self.directory.mkdir(parents=True, exist_ok=True)
        temporary = self.directory / f"route.{secrets.token_hex(8)}.tmp"
        temporary.write_text(json.dumps(state, sort_keys=True), encoding="utf-8")
        os.replace(temporary, self.state_path)

    def reset_route(self, hold_adaptive: bool = False) -> None:
        state: dict[str, Any] = {"status": "pending", "stop_blocks": 0}
        if hold_adaptive:
            state["adaptive_hold"] = 1
        self.write(state)

    def consume_stop_block(self, state: dict[str, Any] | None) -> bool:
        current = state or {"status": "pending"}
        count = int(current.get("stop_blocks", 0))
        if count >= MAX_STOP_BLOCKS:
            return False
        current["stop_blocks"] = count + 1
        self.write(current)
        return True

    def clear_loaded(self) -> None:
        for name in MODULE_ORDER:
            try:
                self.module_marker(name).unlink()
            except FileNotFoundError:
                pass

    def confirmed_marker(self, nonce: str) -> Path:
        return self.directory / f"route-{nonce}.confirmed"

    def module_marker(self, module: str) -> Path:
        return self.directory / f"module-{module}.sha256"

    def confirm(self, nonce: str) -> None:
        self.directory.mkdir(parents=True, exist_ok=True)
        self.confirmed_marker(nonce).touch()

    def is_confirmed(self, state: dict[str, Any]) -> bool:
        nonce = state.get("nonce")
        return isinstance(nonce, str) and self.confirmed_marker(nonce).exists()

    def mark_loaded(self, module: str, path: Path) -> None:
        self.directory.mkdir(parents=True, exist_ok=True)
        self.module_marker(module).write_text(file_sha256(path), encoding="ascii")

    def is_loaded(self, module: str, path: Path) -> bool:
        try:
            recorded = self.module_marker(module).read_text(encoding="ascii").strip()
            return recorded == file_sha256(path)
        except (FileNotFoundError, OSError):
            return False


def policy_dir() -> Path:
    return normalized_path(
        os.environ.get("PIRA_POLICY_DIR", str(Path.home() / ".claude" / "pira"))
    )


def module_paths(required: list[str]) -> dict[str, Path]:
    root = policy_dir()
    return {name: normalized_path(str(root / MODULE_FILES[name])) for name in required}


def missing_modules(session: SessionState, state: dict[str, Any]) -> list[tuple[str, Path]]:
    required = state.get("required")
    if not isinstance(required, list):
        return []
    names = [name for name in required if isinstance(name, str) and name in MODULE_FILES]
    paths = module_paths(names)
    return [(name, paths[name]) for name in names if not session.is_loaded(name, paths[name])]


NO_SKILL_FALLBACK = (
    "If the Skill tool is not available to you, do not stop to ask for it: state that in one line and "
    "continue the task without routing."
)


def route_instruction() -> str:
    return (
        f"PIRA routing is pending. Invoke the `{ROUTE_SKILL}` skill before any task tool or answer, "
        f"passing the applicable module names or `none`. {NO_SKILL_FALLBACK}"
    )


def loading_instruction(missing: list[tuple[str, Path]]) -> str:
    rendered = ", ".join(name for name, _ in missing)
    return (
        "PIRA route context is missing or changed for: "
        + rendered
        + f". Invoke `{ROUTE_SKILL}` again with the current module selection."
    )


def readiness(session: SessionState, state: dict[str, Any] | None) -> tuple[bool, str]:
    if not state or state.get("status") != "selected":
        return False, route_instruction()
    if not session.is_confirmed(state):
        return False, "The PIRA route skill has not completed successfully; invoke it again."
    missing = missing_modules(session, state)
    if missing:
        return False, loading_instruction(missing)
    return True, "PIRA routing is complete for this turn."


def adaptive_enabled() -> bool:
    return os.environ.get(ADAPTIVE_MODE_ENV, "").strip().lower() == "adaptive"


def cue_modules(prompt: str) -> list[str]:
    """Modules whose cues occur in the prompt, expanded with canonical dependencies."""
    hits: set[str] = set()
    for pattern, modules in ADAPTIVE_CUES:
        if re.search(pattern, prompt, re.IGNORECASE):
            hits.update(modules)
    for module in tuple(hits):
        hits.update(IMPLIED.get(module, set()))
    return [name for name in MODULE_ORDER if name in hits]


def adaptive_selection(prompt: str, previous: list[str] | None) -> list[str] | None:
    """Return a conservative module superset, or None when the turn must use the strict Skill route."""
    hits = cue_modules(prompt)
    if previous:
        # Continuation: reuse the confirmed route unless a cue points outside it (task switch).
        return previous if set(hits) <= set(previous) else None
    if not hits or len(hits) > ADAPTIVE_MAX_MODULES:
        return None
    return hits


def previous_route(session: SessionState, state: dict[str, Any] | None) -> list[str] | None:
    if not state or state.get("adaptive_hold"):
        return None
    ready, _ = readiness(session, state)
    required = state.get("required")
    if ready and isinstance(required, list) and required and all(isinstance(name, str) for name in required):
        return required
    return None


def adaptive_select(session: SessionState, selection: list[str]) -> dict[str, Any]:
    state = {
        "status": "selected",
        "nonce": secrets.token_hex(12),
        "required": selection,
        "tool_use_id": "",
        "stop_blocks": 0,
        "source": "adaptive",
    }
    session.write(state)
    rendered, pending_markers = render_modules(session, selection)
    commit_selected(session, state, pending_markers)
    context = "PIRA adaptive routing selected: " + ", ".join(selection) + ". "
    if pending_markers:
        context += "Apply the module context below to this turn.\n\n" + rendered + "\n"
    else:
        context += "All selected modules are already loaded and unchanged in this session. "
    context += (
        f"If this turn also needs another PIRA module, invoke `{ROUTE_SKILL}` with the complete module "
        "list before any task tool; otherwise do not invoke it. PIRA routing is complete for this turn."
    )
    return hook_context("UserPromptSubmit", context)


def handle_user_prompt(data: dict[str, Any], session: SessionState) -> dict[str, Any]:
    if adaptive_enabled():
        state = session.read()
        prompt = data.get("prompt")
        if isinstance(prompt, str) and not (state and state.get("adaptive_hold")):
            selection = adaptive_selection(prompt, previous_route(session, state))
            if selection is not None:
                return adaptive_select(session, selection)
    session.reset_route()
    return hook_context("UserPromptSubmit", route_instruction())


def handle_pre_tool(data: dict[str, Any], session: SessionState) -> dict[str, Any] | None:
    tool_name = str(data.get("tool_name", ""))
    tool_input = data.get("tool_input") if isinstance(data.get("tool_input"), dict) else {}
    if tool_name == "Skill":
        name, arguments = skill_call(tool_input)
        if name == ROUTE_SKILL:
            clean_arguments, supplied_scope, scope_error = split_scope(arguments)
            if scope_error or supplied_scope is not None:
                reason = scope_error or "Internal PIRA route scope metadata is reserved."
                return deny_tool(reason + " " + route_instruction())
            required, error = selected_modules(clean_arguments)
            if error:
                return deny_tool(error + " " + route_instruction())
            nonce = secrets.token_hex(12)
            session.write(
                {
                    "status": "selected",
                    "nonce": nonce,
                    "required": required,
                    "tool_use_id": str(data.get("tool_use_id", "")),
                    "stop_blocks": 0,
                }
            )
            if data.get("agent_id"):
                updated_input = dict(tool_input)
                updated_input["args"] = " ".join(
                    part for part in (clean_arguments, f"{SCOPE_PREFIX}{session.scope_key}") if part
                )
                return {
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "allow",
                        "permissionDecisionReason": "Allow the validated PIRA route skill with isolated subagent state.",
                        "updatedInput": updated_input,
                    }
                }
            return None

    state = session.read()
    ready, reason = readiness(session, state)
    if ready:
        return None
    if data.get("agent_id"):
        # A subagent's tool set is fixed by its definition and may omit Skill, so it can be
        # unable to route. Deny once so a Skill-bearing subagent routes; then fail open.
        current = state if isinstance(state, dict) else {"status": "pending", "stop_blocks": 0}
        denials = int(current.get("tool_denials", 0))
        if denials >= 1:
            return None
        current["tool_denials"] = denials + 1
        session.write(current)
    return deny_tool(reason)


def handle_post_tool(data: dict[str, Any], session: SessionState) -> dict[str, Any] | None:
    state = session.read()
    if not state or state.get("status") != "selected":
        return None
    tool_name = str(data.get("tool_name", ""))
    tool_input = data.get("tool_input") if isinstance(data.get("tool_input"), dict) else {}
    if tool_name == "Skill":
        name, _ = skill_call(tool_input)
        if name != ROUTE_SKILL or str(data.get("tool_use_id", "")) != state.get("tool_use_id"):
            return None
    _, message = readiness(session, state)
    return hook_context("PostToolUse", message)


def prepare_selected(
    session_id: str, arguments: str
) -> tuple[str, SessionState, dict[str, Any], list[tuple[str, Path]]]:
    clean_arguments, scope_key, scope_error = split_scope(arguments)
    if scope_error:
        raise RuntimeError(scope_error)
    session = SessionState(session_id, scope_key=scope_key) if scope_key else SessionState(session_id)
    state = session.read()
    if not state or state.get("status") != "selected":
        raise RuntimeError("no pending PIRA route selection exists for this session")
    required, error = selected_modules(clean_arguments)
    if error:
        raise RuntimeError(error)
    if required != state.get("required"):
        raise RuntimeError("route arguments do not match the validated hook selection")
    rendered, pending_markers = render_modules(session, required or [])
    return rendered, session, state, pending_markers


def render_modules(session: SessionState, required: list[str]) -> tuple[str, list[tuple[str, Path]]]:
    """Render the exact text of modules not yet loaded and return their pending markers."""
    paths = module_paths(required)
    sections: list[str] = []
    pending_markers: list[tuple[str, Path]] = []
    for module in required:
        path = paths[module]
        if session.is_loaded(module, path):
            continue
        content = path.read_text(encoding="utf-8")
        sections.append(f"### Loaded PIRA module: {module}\n\n{content.rstrip()}\n")
        pending_markers.append((module, path))
    if not sections:
        return "All selected PIRA modules were already loaded and unchanged.", pending_markers
    return "\n".join(sections), pending_markers


def commit_selected(
    session: SessionState, state: dict[str, Any], pending_markers: list[tuple[str, Path]]
) -> None:
    for module, path in pending_markers:
        session.mark_loaded(module, path)
    session.confirm(str(state["nonce"]))


def load_selected(session_id: str, arguments: str) -> str:
    rendered, session, state, pending_markers = prepare_selected(session_id, arguments)
    commit_selected(session, state, pending_markers)
    return rendered


def dispatch(data: dict[str, Any]) -> dict[str, Any] | None:
    session_id = data.get("session_id")
    if not isinstance(session_id, str) or not session_id:
        raise ValueError("hook input is missing session_id")
    event = data.get("hook_event_name")
    agent_id = data.get("agent_id")
    if agent_id is not None and (not isinstance(agent_id, str) or not agent_id):
        raise ValueError("hook input contains an invalid agent_id")
    session = SessionState(session_id, agent_id=agent_id)
    if event == "SessionStart":
        session.clear_loaded()
        session.reset_route(hold_adaptive=data.get("source") != "startup")
        return hook_context("SessionStart", route_instruction())
    if event == "SubagentStart":
        session.clear_loaded()
        session.reset_route()
        return hook_context("SubagentStart", route_instruction())
    if event == "UserPromptSubmit":
        return handle_user_prompt(data, session)
    if event == "PreToolUse":
        return handle_pre_tool(data, session)
    if event == "PostToolUse":
        return handle_post_tool(data, session)
    if event in {"Stop", "SubagentStop"}:
        state = session.read()
        ready, reason = readiness(session, state)
        if ready or not session.consume_stop_block(state):
            return None
        return block_stop(
            reason
            + " The guard will request at most one automatic retry this turn; if you cannot invoke the "
            "skill, give your final answer directly now."
        )
    if event == "PostCompact":
        session.clear_loaded()
        session.reset_route(hold_adaptive=True)
        return None
    return None


def main() -> int:
    try:
        if hasattr(sys.stdout, "reconfigure"):
            sys.stdout.reconfigure(encoding="utf-8")
        if hasattr(sys.stderr, "reconfigure"):
            sys.stderr.reconfigure(encoding="utf-8")
        if len(sys.argv) > 1:
            if len(sys.argv) != 4 or sys.argv[1] != "load":
                raise ValueError("usage: pira_routing_guard.py load SESSION_ID MODULES")
            rendered, session, state, pending_markers = prepare_selected(sys.argv[2], sys.argv[3])
            print(rendered, flush=True)
            commit_selected(session, state, pending_markers)
            return 0
        data = json.load(sys.stdin)
        if not isinstance(data, dict):
            raise ValueError("hook input must be a JSON object")
        emit(dispatch(data))
        return 0
    except Exception as exc:  # Fail visibly without making an unavailable pilot brick Claude Code.
        print(f"PIRA routing guard error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
