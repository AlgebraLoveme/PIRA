"""Offline evidence checks; callers supply paths and complete expected file text."""

from __future__ import annotations

import os
import re
import shlex
from collections import Counter
from collections.abc import Mapping
from pathlib import Path


def _path(value: str | Path, cwd: Path) -> str:
    return os.path.normcase(os.path.realpath(os.path.join(str(cwd), str(value))))


def _texts(content: object) -> list[str]:
    if isinstance(content, str):
        return [content]
    if isinstance(content, list):
        return [block["text"] for block in content if isinstance(block, dict)
                and block.get("type") == "text" and isinstance(block.get("text"), str)]
    return []


def _lines(text: str) -> list[str]:
    lines = text.replace("\r\n", "\n").split("\n")
    return lines[:-1] if lines[-1] == "" else lines


def complete_content(output: str, expected: str) -> bool:
    """Require one contiguous, exact full-file block, raw or numbered from one.

    Ignore surrounding CLI framing, not whitespace within the file. Never join
    fragments across results or text blocks. Empty files cannot prove a read.
    """
    lines = _lines(output)
    wanted = _lines(expected)
    if not wanted:
        return False
    numbered = [re.fullmatch(r"[ \t]*(\d+)(?:→|\t| )(.*)", line) for line in lines]
    for start in range(len(lines) - len(wanted) + 1):
        if lines[start:start + len(wanted)] == wanted:
            return True
        if all(match and int(match[1]) == index and match[2] == text
               for index, (match, text) in enumerate(
                   zip(numbered[start:start + len(wanted)], wanted), 1)):
            return True
    return False


def _blocks(event: dict) -> list[dict]:
    message = event.get("message")
    content = message.get("content") if isinstance(message, dict) else None
    return [block for block in content if isinstance(block, dict)] if isinstance(content, list) else []


def _claude_pairs(events: list[dict]) -> list[tuple[dict, dict]]:
    uses, results = [], []
    for position, event in enumerate(events):
        for block in _blocks(event):
            if event.get("type") == "assistant" and block.get("type") == "tool_use":
                uses.append((position, block))
            elif event.get("type") == "user" and block.get("type") == "tool_result":
                results.append((position, block))
    use_counts = Counter(block.get("id") for _, block in uses if isinstance(block.get("id"), str))
    result_counts = Counter(block.get("tool_use_id") for _, block in results
                            if isinstance(block.get("tool_use_id"), str))
    matched = {block.get("tool_use_id"): (position, block) for position, block in results
               if isinstance(block.get("tool_use_id"), str)}
    pairs = []
    for position, use in uses:
        identifier = use.get("id")
        if not isinstance(identifier, str) or not identifier:
            continue
        if use_counts[identifier] != 1 or result_counts[identifier] != 1:
            continue
        result_position, result = matched[identifier]
        if result_position > position and not result.get("is_error"):
            pairs.append((use, result))
    return pairs


def native_read_evidence(events: list[dict], expected: Mapping[Path, str], cwd: Path) -> dict[Path, bool]:
    """Count only correlated, non-error native Read results for the exact path."""
    reads = dict.fromkeys(expected, False)
    for use, result in _claude_pairs(events):
        inputs = use.get("input", {})
        if use.get("name") != "Read" or not isinstance(inputs, dict):
            continue
        path = inputs.get("file_path")
        if not isinstance(path, str) or not path or inputs.get("offset", 1) != 1:
            continue
        for target, text in expected.items():
            if _path(path, cwd) == _path(target, cwd):
                limit = inputs.get("limit")
                if limit is not None and (not isinstance(limit, int) or limit < len(_lines(text))):
                    continue
                if result.get("exit_code", 0) != 0:
                    continue
                reads[target] |= any(complete_content(output, text) for output in _texts(result.get("content")))
    return reads


def _reader_patterns(command: str, depth: int = 0) -> list[str]:
    # PIRA: conservative reader subset, not a shell interpreter. Unsupported
    # scripts, cwd changes, pipelines and substitutions fail closed; extend with
    # framing/command fixtures before adding a new reader or shell syntax.
    if depth > 2 or any(token in command for token in ("$", "`", "<", ">", "|", "\n")):
        return []
    try:
        lexer = shlex.shlex(command, posix=True, punctuation_chars=";&")
        lexer.whitespace_split = True
        tokens = list(lexer)
    except ValueError:
        return []
    if not tokens:
        return []
    executable = tokens[0].replace("\\", "/").rsplit("/", 1)[-1].lower()
    if executable in ("bash", "sh", "powershell", "powershell.exe", "pwsh", "pwsh.exe"):
        for flag in ("-c", "-lc", "-Command", "-command"):
            if flag in tokens and tokens.index(flag) == len(tokens) - 2:
                return _reader_patterns(tokens[-1], depth + 1)
        return []
    segments = [[]]
    for token in tokens:
        if token in (";", "&&"):
            segments.append([])
        else:
            segments[-1].append(token)
    patterns = []
    for segment in segments:
        if not segment:
            return []
        reader = segment[0].rsplit("/", 1)[-1].lower()
        allowed_flags = {
            "cat": {"-n", "-b", "-s", "--"},
            "nl": {"-ba", "-bt", "-bn", "--"},
            "sed": {"-n", "--"},
            "get-content": {"-raw", "-path", "-literalpath"},
        }
        if reader not in allowed_flags:
            return []
        args = segment[1:]
        if any(token.startswith("-") and token.lower() not in allowed_flags[reader] for token in args):
            return []
        operands = [token for token in args if not token.startswith("-")]
        if reader == "sed":
            operands = operands[1:]  # First operand is the script, not a file.
        if reader == "get-content" and any(token.lower() == "-literalpath" for token in args):
            if any(character in operand for operand in operands for character in "*?["):
                return []
        patterns.extend(operands)
    return patterns


def _shell_reads(command: object, output: str, expected: Mapping[Path, str], cwd: Path) -> dict[Path, bool]:
    patterns = _reader_patterns(command) if isinstance(command, str) else []
    return {target: any(Path(_path(target, cwd)).match(_path(pattern, cwd)) for pattern in patterns)
            and complete_content(output, text) for target, text in expected.items()}


def _shell_success(result: dict, output: str) -> bool:
    if result.get("is_error") or result.get("exit_code", 0) != 0:
        return False
    return not any(int(match[1]) != 0 for match in re.finditer(
        r"(?m)^(?:Process exited with code |Exit code[: ]+)(\d+)\s*$", output))


def claude_evidence(events: list[dict], expected: Mapping[Path, str], cwd: Path) -> dict:
    """Summarize native and supported Bash reads plus Claude completion status."""
    reads = native_read_evidence(events, expected, cwd)
    for use, result in _claude_pairs(events):
        inputs = use.get("input", {})
        if use.get("name") != "Bash" or not isinstance(inputs, dict):
            continue
        outputs = _texts(result.get("content"))
        if not all(_shell_success(result, output) for output in outputs):
            continue
        for output in outputs:
            for path, read in _shell_reads(inputs.get("command"), output, expected, cwd).items():
                reads[path] |= read
    report = next((event for event in reversed(events) if event.get("type") == "result"), {})
    return {"reads": reads, "denials": len(report.get("permission_denials") or []),
            "completed": report.get("subtype") == "success" and not report.get("is_error")}


def codex_evidence(events: list[dict], expected: Mapping[Path, str], cwd: Path) -> dict:
    """Summarize complete successful command outputs without merging fragments."""
    reads = dict.fromkeys(expected, False)
    commands = 0
    for event in events:
        item = event.get("item", {})
        if (event.get("type") != "item.completed" or not isinstance(item, dict)
                or item.get("type") != "command_execution" or item.get("exit_code") != 0
                or item.get("status") not in (None, "completed")):
            continue
        output = item.get("aggregated_output")
        if not isinstance(output, str) or not _shell_success(item, output):
            continue
        commands += 1
        for path, read in _shell_reads(item.get("command"), output, expected, cwd).items():
            reads[path] |= read
    return {"reads": reads, "denials": None, "command_outputs": commands,
            "completed": any(event.get("type") == "turn.completed" for event in events)
            and not any(event.get("type") in ("turn.failed", "error") for event in events)}
