#!/usr/bin/env python3
"""Stage, verify, or publish a private native CI snapshot (Python 3.11+)."""
from __future__ import annotations

import argparse
from contextlib import ExitStack
import hashlib
import json
import os
import platform
import stat
import tarfile
import zipfile
from urllib.request import urlopen
from pathlib import Path, PurePosixPath
import re
import shlex
import subprocess
import tempfile
import sys

TOOLS = ("pira_ctx", "pira_dec", "pira_nav", "pira_svg_check", "pira_team")
POLICIES = ("main.md", "implementation.md", "review.md", "backend_contract.json", "worker_profiles.json")
BLOCKED = {"__pycache__", "target", "cache", "caches", "logs", "stores", "credentials", "node_modules", "debug", "generated", "profiles", "auth"}

NATIVE_CODEX_VERSION = "0.161.0"
NATIVE_CODEX_ASSETS = {
    "linux": ("codex-x86_64-unknown-linux-musl.tar.gz",
              "b1efb95097660d7f2e5a3887618a23f2ea1b0d548078bf92b0f7a5d229a0cef2",
              "codex-x86_64-unknown-linux-musl"),
    "win32": ("codex-x86_64-pc-windows-msvc.exe.zip",
              "a7493348634867c905f7298211923c57eb01dfe45400207a423c5453b190b11a",
              "codex-x86_64-pc-windows-msvc.exe"),
}
# Exact layout observed in the hash-pinned official Windows 0.161.0 ZIP.
WINDOWS_CODEX_MEMBERS = (
    'bin/',
    'codex-code-mode-host.exe',
    'codex-command-runner.exe',
    'codex-package.json',
    'codex-path/',
    'codex-path/rg.exe',
    'codex-resources/',
    'codex-resources/codex-command-runner.exe',
    'codex-resources/codex-windows-sandbox-setup.exe',
    'codex-resources/voice/',
    'codex-resources/voice/bin/',
    'codex-resources/voice/bin/codex-voice-host.exe',
    'codex-resources/voice/bin/gio-2.0-0.dll',
    'codex-resources/voice/bin/glib-2.0-0.dll',
    'codex-resources/voice/bin/gmodule-2.0-0.dll',
    'codex-resources/voice/bin/gobject-2.0-0.dll',
    'codex-resources/voice/bin/gstapp-1.0-0.dll',
    'codex-resources/voice/bin/gstapp.dll',
    'codex-resources/voice/bin/gstaudio-1.0-0.dll',
    'codex-resources/voice/bin/gstaudioconvert.dll',
    'codex-resources/voice/bin/gstaudioresample.dll',
    'codex-resources/voice/bin/gstbase-1.0-0.dll',
    'codex-resources/voice/bin/gstcoreelements.dll',
    'codex-resources/voice/bin/gstnet-1.0-0.dll',
    'codex-resources/voice/bin/gstopus.dll',
    'codex-resources/voice/bin/gstpbutils-1.0-0.dll',
    'codex-resources/voice/bin/gstreamer-1.0-0.dll',
    'codex-resources/voice/bin/gstrtp-1.0-0.dll',
    'codex-resources/voice/bin/gstrtp.dll',
    'codex-resources/voice/bin/gstrtpmanager.dll',
    'codex-resources/voice/bin/gsttag-1.0-0.dll',
    'codex-resources/voice/bin/gstvideo-1.0-0.dll',
    'codex-resources/voice/bin/intl-8.dll',
    'codex-resources/voice/bin/libffi-8.dll',
    'codex-resources/voice/bin/opus.dll',
    'codex-resources/voice/bin/pcre2-8.dll',
    'codex-resources/voice/bin/vcruntime140.dll',
    'codex-resources/voice/bin/z.dll',
    'codex-resources/voice/licenses/',
    'codex-resources/voice/licenses/LGPL-2.1.txt',
    'codex-resources/voice/licenses/libffi.txt',
    'codex-resources/voice/licenses/Opus.txt',
    'codex-resources/voice/licenses/PCRE2.md',
    'codex-resources/voice/licenses/proxy-libintl.txt',
    'codex-resources/voice/licenses/sljit.txt',
    'codex-resources/voice/licenses/zlib.txt',
    'codex-resources/voice/manifest.json',
    'codex-resources/voice/NOTICE.md',
    'codex-resources/voice/runtime.json',
    'codex-resources/voice/sources.json',
    'codex-resources/voice/windows-crt.json',
    'codex-windows-sandbox-setup.exe',
    'codex-x86_64-pc-windows-msvc.exe',
)
WINDOWS_CODEX_MANIFEST = {'layoutVersion': 1, 'version': '0.161.0', 'target': 'x86_64-pc-windows-msvc', 'variant': 'codex', 'entrypoint': 'codex-x86_64-pc-windows-msvc.exe', 'resourcesDir': 'codex-resources', 'pathDir': 'codex-path'}
MAX_CODEX_BYTES = 512 * 1024 * 1024
NATIVE_RELOCATION_INPUTS = (
    "assets/scripts/team_store_relocation.py",
    "assets/scripts/test_team_store_relocation.py",
    "assets/scripts/team_relocation_fixture.py",
)


def extract_windows_codex(archive: Path, directory: Path) -> None:
    """Retain the entire pinned relative layout, never extract archive-selected paths."""
    if directory.is_symlink() or not directory.is_dir() or any(directory.iterdir()):
        raise RuntimeError("Windows Codex extraction requires an empty plain directory")
    with zipfile.ZipFile(archive) as package:
        members = package.infolist()
        if (len(members) != len(WINDOWS_CODEX_MEMBERS)
                or {item.filename for item in members} != set(WINDOWS_CODEX_MEMBERS)):
            raise RuntimeError("Unexpected Codex archive member inventory")
        total = 0
        for item in members:
            directory_entry = item.filename.endswith('/')
            allowed_modes = (0, stat.S_IFDIR) if directory_entry else (0, stat.S_IFREG)
            if (stat.S_IFMT(item.external_attr >> 16) not in allowed_modes
                    or bool(item.external_attr & 0x10) and not directory_entry
                    or item.flag_bits & 1
                    or (directory_entry and item.file_size != 0)
                    or (not directory_entry and not 0 < item.file_size <= MAX_CODEX_BYTES)):
                raise RuntimeError("Unsafe Codex archive member")
            total += item.file_size
        if total > MAX_CODEX_BYTES:
            raise RuntimeError("Invalid expanded Codex size")
        if package.getinfo('codex-package.json').file_size > 4096:
            raise RuntimeError("Invalid Codex package manifest size")
        if json.loads(package.read('codex-package.json')) != WINDOWS_CODEX_MANIFEST:
            raise RuntimeError("Unexpected Codex package manifest")
        # All names have matched literal reviewed constants, including directories.
        for name in WINDOWS_CODEX_MEMBERS:
            target = directory / name
            if name.endswith('/'):
                target.mkdir()
                continue
            item = package.getinfo(name)
            received = 0
            with package.open(item) as source, target.open('xb') as output:
                while chunk := source.read(64 * 1024):
                    received += len(chunk)
                    if received > item.file_size:
                        raise RuntimeError("Expanded Codex exceeds declared size")
                    output.write(chunk)
            if received != item.file_size:
                raise RuntimeError("Truncated Codex member")


def extract_codex(archive: Path, member_name: str, destination: Path) -> None:
    """Extract the pinned platform layout without trusting archive paths or modes."""
    try:
        with ExitStack() as stack:
            if archive.name.endswith(".zip"):
                if member_name != WINDOWS_CODEX_MANIFEST['entrypoint'] or destination.name != member_name:
                    raise RuntimeError("Unexpected Windows Codex entrypoint")
                extract_windows_codex(archive, destination.parent)
                return
            else:
                package = stack.enter_context(tarfile.open(archive, "r:gz"))
                member = package.next()
                if member is None or member.name != member_name or not member.isfile():
                    raise RuntimeError("Unexpected Codex archive member")
                size = member.size
                if not 0 < size <= MAX_CODEX_BYTES:
                    raise RuntimeError("Invalid expanded Codex size")
                source = stack.enter_context(package.extractfile(member))
            received = 0
            with destination.open("xb") as output:
                while chunk := source.read(64 * 1024):
                    received += len(chunk)
                    if received > size:
                        raise RuntimeError("Expanded Codex exceeds declared size")
                    output.write(chunk)
            if received != size:
                raise RuntimeError("Truncated Codex member")
            if not archive.name.endswith(".zip") and package.next() is not None:
                raise RuntimeError("Expected exactly one Codex archive member")
    except (tarfile.TarError, zipfile.BadZipFile) as error:
        raise RuntimeError(f"Invalid Codex archive: {error}") from error


def provision_codex(directory: Path) -> Path:
    """Provision only the pinned CI binary in a new explicit task-local directory."""
    if sys.platform not in NATIVE_CODEX_ASSETS or platform.machine().lower() not in ("amd64", "x86_64"):
        raise RuntimeError("Native relocation CI requires x64 Linux or Windows")
    if directory.exists() or directory.is_symlink():
        raise RuntimeError(f"Refusing to overwrite Codex destination: {directory}")
    asset, expected, member = NATIVE_CODEX_ASSETS[sys.platform]
    url = f"https://github.com/openai/codex/releases/download/rust-v{NATIVE_CODEX_VERSION}/{asset}"
    directory.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".pira-native-codex-", dir=directory.parent) as temporary:
        root = Path(temporary)
        archive = root / asset
        checksum = hashlib.sha256()
        received = 0
        with urlopen(url, timeout=60) as response, archive.open("xb") as output:
            while chunk := response.read(64 * 1024):
                received += len(chunk)
                if received > MAX_CODEX_BYTES:
                    raise RuntimeError("Codex archive exceeds safety limit")
                checksum.update(chunk)
                output.write(chunk)
        if checksum.hexdigest() != expected:
            raise RuntimeError("Pinned Codex SHA-256 mismatch")
        package = root / "bin"
        package.mkdir()
        binary = package / (member if sys.platform == "win32" else "codex")
        extract_codex(archive, member, binary)
        binary.chmod(0o755)
        home = root / "home"
        home.mkdir()
        env = {key: value for key, value in os.environ.items()
               if key.upper() in {"PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT"}}
        env.update(HOME=str(home), USERPROFILE=str(home), LOCALAPPDATA=str(home),
                   CODEX_HOME=str(home), TMP=str(root), TEMP=str(root), TMPDIR=str(root))
        try:
            result = subprocess.run([str(binary), "--version"], env=env, stdin=subprocess.DEVNULL,
                                    capture_output=True, text=True, encoding="utf-8", timeout=15, check=False)
        except subprocess.TimeoutExpired as error:
            raise RuntimeError("Pinned Codex version check timed out") from error
        if result.returncode or result.stdout.strip() != f"codex-cli {NATIVE_CODEX_VERSION}":
            raise RuntimeError("Pinned Codex binary version check failed")
        if directory.exists() or directory.is_symlink():
            raise RuntimeError(f"Codex destination appeared during provisioning: {directory}")
        package.rename(directory)
    return directory / binary.name


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()

def selected(root: Path) -> list[Path]:
    # Runtime policy-mirror inputs used by Team Python tests (not user profiles).
    paths = [Path("AGENTS.md"), Path("modules/CODING_STYLE.md"),
             Path("tools/Cargo.toml"), Path("tools/Cargo.lock")]
    for tool in TOOLS:
        paths.append(Path(f"tools/crates/{tool}/Cargo.toml"))
        for directory, extensions in ((f"tools/src/{tool}", {".rs"}),
                                       (f"tools/crates/{tool}/tests", {".rs", ".py", ".svg"})):
            base = root / directory
            if not base.is_dir() or base.is_symlink():
                raise RuntimeError(f"missing or linked allowed directory: {directory}")
            for parent, dirs, files in os.walk(base, followlinks=False):
                dirs[:] = sorted(d for d in dirs if not d.startswith(".") and d.lower() not in BLOCKED
                                 and "benchmark" not in d.lower())
                for name in dirs + files:
                    if (Path(parent) / name).is_symlink():
                        raise RuntimeError(f"symlink in allowed tree: {directory}/{name}")
                for name in sorted(files):
                    if (name.startswith(".") or name == "live_smoke.py" or "benchmark" in name.lower()
                            or Path(name).suffix not in extensions):
                        continue
                    paths.append((Path(parent) / name).relative_to(root))
    paths.extend(Path("tools/src/pira_team") / name for name in POLICIES)
    paths.append(Path("tools/crates/pira_svg_check/tests/fixtures/linux-dejavu-fonts.conf"))
    paths.extend(Path(name) for name in (
        "assets/scripts/setup_pira.py", "assets/scripts/test_setup_pira.py",
        "assets/scripts/retire_pira_audio.py", "assets/scripts/test_retire_pira_audio.py",
        "assets/scripts/setup_pira_tools.py", "assets/scripts/test_setup_pira_tools.py",
        "assets/scripts/setup_pira_stores.py", "assets/scripts/test_setup_pira_stores.py",
        "assets/scripts/setup_migration_choices.py", "assets/scripts/test_setup_migration_choices.py",
        "assets/scripts/migrate_pira_stores.py", "assets/scripts/test_migrate_pira_stores.py",
        "assets/LEGACY_LIST.md", "tools/select_tool_for_platform.py"))
    paths.append(Path("tools/build/private_ci.py"))
    paths.extend(Path(name) for name in NATIVE_RELOCATION_INPUTS)
    return sorted(set(paths))

def stage(root: Path, test_tools: tuple[str, ...] = TOOLS) -> dict:
    """Copy allowlisted working-tree bytes to a fresh temporary snapshot."""
    root = root.resolve()
    release = root / ".github/workflows/build-pira-tool-bundles.yml"
    pins = set(re.findall(r"actions/checkout@([0-9a-f]{40})", release.read_text()))
    if len(pins) != 1:
        raise ValueError("expected one pinned checkout revision in release workflow")
    workflow = Path(__file__).with_name("private_ci_workflow.yml").read_text().replace(
        "@CHECKOUT@", pins.pop())

    def dirty() -> bytes:
        return git(root, "status", "--porcelain=v1", "-z", "--untracked-files=all").stdout

    def head() -> str:
        return git(root, "rev-parse", "HEAD").stdout.decode().strip()

    if not test_tools or any(tool not in TOOLS for tool in test_tools):
        raise ValueError("select at least one known test tool")
    original_head = head()
    original_status = dirty()
    paths = selected(root)
    data = {}
    for path in paths:
        if any(part.is_symlink() for part in (root / path, *(root / path).parents)):
            raise RuntimeError(f"linked source: {path}")
        data[path.as_posix()] = (root / path).read_bytes()
    # Literal include dependencies must remain within the selected snapshot.
    for name, content in data.items():
        if not name.endswith(".rs"):
            continue
        for dependency in re.findall(r'include(?:_str|_bytes)?!\s*\(\s*"([^"]+)"', content.decode()):
            target = (root / name).parent / dependency
            relative = target.resolve().relative_to(root).as_posix()
            if relative not in data:
                raise RuntimeError(f"unselected include dependency: {name}: {dependency}")
    # Catch concurrent changes, including newly added/removed eligible files.
    if paths != selected(root) or original_head != head() or original_status != dirty() or any(
            (root / name).read_bytes() != content for name, content in data.items()):
        raise RuntimeError("source changed during staging; rerun after workers finish")
    counts = {tool: sum(f"/{tool}/" in name for name in data) for tool in TOOLS}
    provenance = {"original_head": original_head, "source": "current working tree, including uncommitted changes",
                  "original_dirty": bool(original_status), "original_status_sha256": digest(original_status),
                  "source_file_count": len(data), "per_tool_file_counts": counts, "tested_tools": list(test_tools)}
    data["SNAPSHOT_PROVENANCE.json"] = (json.dumps(provenance, indent=2, sort_keys=True) + "\n").encode()
    data[".github/workflows/private-pira-tests.yml"] = workflow.replace(
        "crate: [pira_ctx, pira_dec, pira_nav, pira_svg_check, pira_team]",
        "crate: " + json.dumps(list(test_tools))).encode()
    data[".gitattributes"] = b"* -text\n"
    manifest = "".join(f"{digest(content)}  {name}\n" for name, content in sorted(data.items()))
    data["SOURCE_SHA256SUMS"] = manifest.encode()
    destination = Path(tempfile.mkdtemp(prefix="pira-private-ci-"))
    for name, content in data.items():
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
    verify(destination)
    return {"stage": str(destination), **provenance, "total_files": len(data)}


def git(root: Path, *args: str) -> subprocess.CompletedProcess:
    """Run Git without inherited repository/index overrides or hooks."""
    env = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}
    env['GIT_TERMINAL_PROMPT'] = '0'
    env['GIT_OPTIONAL_LOCKS'] = '0'
    return subprocess.run(['git', '-c', f'core.hooksPath={os.devnull}',
                           '-c', 'core.autocrlf=false', '-c', f'core.attributesFile={os.devnull}', *args], cwd=root,
                          env=env, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

def verify(stage: Path) -> dict[str, bytes]:
    """Validate exact inventory and hashes; return immutable verified file bytes."""
    if stage.is_symlink() or not stage.is_dir():
        raise ValueError('expected an unlinked stage directory')
    inventory = set()
    for parent, dirs, files in os.walk(stage, followlinks=False):
        for name in dirs + files:
            path = Path(parent) / name
            if path.is_symlink() or name.lower() == '.git':
                raise ValueError(f'linked path or Git metadata in stage: {path}')
        for name in files:
            path = Path(parent) / name
            if not path.is_file():
                raise ValueError(f'not a regular file: {path}')
            inventory.add(path.relative_to(stage).as_posix())
    manifest = (stage / 'SOURCE_SHA256SUMS').read_bytes()
    data = {}
    folded = set()
    for line in manifest.decode('utf-8').splitlines():
        match = re.fullmatch(r'([0-9a-f]{64})  (.+)', line)
        if not match:
            raise ValueError('invalid manifest record')
        expected, name = match.groups()
        parts = PurePosixPath(name).parts
        if (not parts or PurePosixPath(name).is_absolute() or
                PurePosixPath(name).as_posix() != name or
                any(p in ('.', '..') or p.lower() == '.git' for p in parts) or
                any(c in name for c in '\\:\r\n\t') or
                name == 'SOURCE_SHA256SUMS' or name.casefold() in folded):
            raise ValueError(f'unsafe or duplicate manifest path: {name}')
        folded.add(name.casefold())
        if name not in inventory:
            raise ValueError(f'missing manifest file: {name}')
        content = (stage / name).read_bytes()
        if digest(content) != expected:
            raise ValueError(f'snapshot hash mismatch: {name}')
        data[name] = content
    required = {'SNAPSHOT_PROVENANCE.json', '.gitattributes',
                '.github/workflows/private-pira-tests.yml'}
    if not required <= data.keys() or inventory != data.keys() | {'SOURCE_SHA256SUMS'}:
        raise ValueError('missing required snapshot metadata or extraneous files')
    data['SOURCE_SHA256SUMS'] = manifest
    return data

def require_private(repo: str) -> None:
    """Fail closed on destination visibility, including API/authentication errors."""
    result = subprocess.run(['gh', 'api', '--hostname', 'github.com', f'repos/{repo}'],
                            check=True, stdout=subprocess.PIPE, text=True)
    info = json.loads(result.stdout)
    if info.get('private') is not True or info.get('full_name', '').lower() != repo.lower():
        raise ValueError('refusing non-private or mismatched destination')

def publish(stage: Path, repo: str, branch: str) -> dict:
    """Publish verified bytes to a new private branch, without modifying the stage."""
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*', repo):
        raise ValueError('expected explicit OWNER/REPO on github.com')
    if branch.startswith('-') or branch == 'HEAD':
        raise ValueError('expected a new branch name')
    data = verify(stage)
    require_private(repo)
    remote = f'https://github.com/{repo}.git'
    ref = f'refs/heads/{branch}'
    # PIRA: publication is GitHub-only; supporting other hosts requires explicit visibility APIs.
    with tempfile.TemporaryDirectory(prefix='pira-private-publish-') as tmp:
        work = Path(tmp)
        git(work, 'check-ref-format', ref)
        git(work, 'init', '--template=', '-b', branch)
        if git(work, 'ls-remote', '--heads', remote, ref).stdout.strip():
            raise ValueError('destination branch already exists; choose a new branch (never overwrite)')
        for name, content in data.items():
            path = work / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        git(work, 'add', '--force', '--', '.')
        git(work, '-c', 'user.name=Private CI snapshot', '-c',
            'user.email=private-ci@users.noreply.github.com', '-c', 'commit.gpgSign=false',
            'commit', '-m', 'Validate private native-platform snapshot')
        commit = git(work, 'rev-parse', 'HEAD').stdout.decode().strip()
        require_private(repo)
        try:
            # Empty lease requires nonexistence even if another publisher won the race.
            git(work, 'push', f'--force-with-lease={ref}:', remote, f'HEAD:{ref}')
        except subprocess.CalledProcessError as error:
            inspect_command = shlex.join(['git', 'ls-remote', '--heads', remote, ref])
            raise RuntimeError(
                f'Publish failed or outcome is uncertain; stage is unchanged: {stage}. '
                f'Expected commit {commit}. Inspect `{inspect_command}` '
                'and GitHub Actions. If that commit exists, do not republish; if absent, '
                'retry the same command; if a different commit exists, choose a new branch. '
                f'Never delete/force an existing branch. Git: {(error.stderr or b"").decode(errors="replace")}') from error
    return {'repo': repo, 'branch': branch, 'commit': commit}

def native_relocation(directory: Path, binary: Path, root: Path) -> None:
    """Run the owner's required fixture; retain synthetic evidence, never skip."""
    directory = directory.resolve()
    binary = binary.resolve(strict=True)
    root = root.resolve(strict=True)
    directory.mkdir()  # Require a fresh task-local destination, never reuse a home.
    home = directory / 'home'
    home.mkdir()
    environment = {key: value for key, value in os.environ.items()
                   if key.upper() in {'PATH', 'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'PATHEXT'}}
    environment.update(HOME=str(home), USERPROFILE=str(home), LOCALAPPDATA=str(home),
                       CODEX_HOME=str(home / 'codex'), TMP=str(home), TEMP=str(home),
                       TMPDIR=str(home), PYTHONDONTWRITEBYTECODE='1',
                       PIRA_RELOCATION_TEST_SCRATCH=str(directory))
    test = root / 'assets/scripts/test_team_store_relocation.py'
    commands = [
        ('deterministic', [sys.executable, '-B', str(test), '-v']),
        ('completed-turns', [sys.executable, '-B', str(test), '--native', '--completed-turns',
                             '--scratch', str(directory), '--binary', str(binary)]),
    ]
    failures = []
    try:
        for label, command in commands:
            log = directory / (label + '.log')
            with log.open('wb') as output:
                try:
                    result = subprocess.run(command, cwd=root, env=environment,
                                            stdin=subprocess.DEVNULL, stdout=output,
                                            stderr=subprocess.STDOUT, timeout=480)
                    if result.returncode:
                        failures.append(f'{label}: exit {result.returncode}')
                except subprocess.TimeoutExpired:
                    failures.append(f'{label}: timed out')
            print(f'{label}: evidence {log}', flush=True)
    finally:
        # Only synthetic diagnostics, not homes, auth, rollout databases or records.
        # Prefix/JSON-escape lines so subprocess text cannot become Actions commands.
        names = {'deterministic.log', 'completed-turns.log', 'result.json',
                 'repair.json', 'failure.json', 'native.stderr'}
        for parent, dirs, files in os.walk(directory, followlinks=False):
            dirs[:] = sorted(name for name in dirs if not (Path(parent) / name).is_symlink())
            for name in sorted(set(files) & names):
                path = Path(parent) / name
                if path.is_symlink() or not path.is_file():
                    continue
                print('Evidence: ' + str(path.relative_to(directory)), flush=True)
                with path.open(encoding='utf-8', errors='replace') as stream:
                    for line in stream:
                        print('fixture: ' + json.dumps(line.rstrip('\n')), flush=True)
        print('Retained disposable evidence: ' + str(directory), flush=True)
    if failures:
        raise RuntimeError('Required native relocation validation failed: ' + '; '.join(failures))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    staging = commands.add_parser('stage', help='create fresh local snapshot; no network')
    staging.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[2])
    staging.add_argument('--test-tool', choices=TOOLS, action='append',
                         help='repeat for a CI subset; all workspace sources are still staged')
    checking = commands.add_parser('verify', help='check exact inventory and hashes; no network')
    checking.add_argument('--stage', type=Path, required=True)
    publishing = commands.add_parser('publish', help='create a new branch in an explicit private repo')
    publishing.add_argument('--stage', type=Path, required=True)
    publishing.add_argument('--repo', required=True)
    publishing.add_argument('--branch', required=True)
    provisioning = commands.add_parser('provision-codex', help='download hash-pinned CI Codex into a new task-local directory')
    provisioning.add_argument('--directory', type=Path, required=True)
    native = commands.add_parser('native-relocation', help='run required isolated completed-turn fixture')
    native.add_argument('--directory', type=Path, required=True)
    native.add_argument('--binary', type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == 'stage':
            result = stage(args.root, tuple(dict.fromkeys(args.test_tool or TOOLS)))
        elif args.command == 'provision-codex':
            result = {'codex': str(provision_codex(args.directory))}
        elif args.command == 'native-relocation':
            native_relocation(args.directory, args.binary, Path(__file__).resolve().parents[2])
            result = {'status': 'verified_fixture_only', 'evidence': str(args.directory.resolve())}
        elif args.command == 'verify':
            result = {'verified_files': len(verify(args.stage))}
        else:
            result = publish(args.stage, args.repo, args.branch)
        print(json.dumps(result, indent=2, sort_keys=True))
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f'private_ci: {error}', file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr.decode(errors='replace'), file=sys.stderr)
        raise SystemExit(1)


if __name__ == '__main__':
    main()
