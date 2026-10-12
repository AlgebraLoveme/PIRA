"""Guarded relocation of ONE PIRA-managed per-run Codex home (stdlib only).

Caller holds both run locks, validates manifest.thread_id/idle ownership, and
publishes an independent copy at its FINAL path. Caller owns lineage receipts.
This module never copies, merges, renames the destination, or runs inference.
Only validated platforms and the bounded native format are admitted.
"""
from contextlib import AbstractContextManager
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import re
import shutil
import signal
import stat
import subprocess
import threading
import time
import uuid

NATIVE_VERSION = 'codex-cli 0.161.0'
# Deliberately no environment/CLI bypass. Promotion requires reviewed evidence.
_VALIDATED_PLATFORMS = frozenset({'Darwin', 'Linux', 'Windows'})
_PAGE_SIZE = 100
_DATABASES = {'state_5', 'thread_history_1', 'goals_1', 'logs_2', 'memories_1', 'queue_1'}
_METHODS = {'initialize', 'thread/read', 'thread/resume', 'thread/items/list', 'thread/turns/list'}
_CONFIG = ['project_doc_max_bytes=0', 'approval_policy="never"', 'sandbox_mode="workspace-write"',
           'agents.enabled=false', 'features.multi_agent=false', 'features.memories=false',
           'features.apps=false', 'features.plugins=false', 'features.hooks=false',
           'features.skip_host_skill_discovery=true', 'features.shell_snapshot=false', 'web_search="disabled"',
           'model_provider="relocation_probe"',
           'model_providers.relocation_probe.name="No-inference relocation"',
           'model_providers.relocation_probe.base_url="http://127.0.0.1:0/v1"',
           'model_providers.relocation_probe.wire_api="responses"',
           'model_providers.relocation_probe.requires_openai_auth=false']


class RelocationError(RuntimeError):
    """code is stable; journal identifies recoverable state, never auto-discard it."""
    def __init__(self, code, message, *, journal=None):
        self.code, self.journal = code, str(journal) if journal else None
        super().__init__(message)


def capabilities():
    return {'api': 1, 'native_version': NATIVE_VERSION, 'platform': platform.system(),
            'admitted': platform.system() in _VALIDATED_PLATFORMS,
            'validated_platforms': sorted(_VALIDATED_PLATFORMS),
            'scope': 'one manifest-authoritative, nonarchived, nonforked managed thread'}


def _fail(code, message):
    raise RelocationError(code, message)


def _identity(path):
    s = path.lstat()
    return [s.st_dev, s.st_ino]


def _plain(path, directory=False):
    s = path.lstat()
    if stat.S_ISLNK(s.st_mode) or getattr(s, 'st_file_attributes', 0) & 0x400:
        _fail('unsafe_path', 'Symlink/reparse point is unsupported: '+str(path))
    if not (stat.S_ISDIR(s.st_mode) if directory else stat.S_ISREG(s.st_mode)):
        _fail('unsafe_path', 'Unexpected file type: '+str(path))
    if not directory and s.st_nlink != 1:
        _fail('unsafe_path', 'Hard-linked file is unsupported: '+str(path))
    return s


def _canonical_directory(path):
    p = Path(os.path.abspath(path))
    _plain(p, True)
    if p.resolve() != p:
        _fail('unsafe_path', 'Use canonical paths without aliased parents: '+str(p))
    return p


def _hash(path, length=None):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        remaining = length
        while remaining is None or remaining:
            part = stream.read(1024*1024 if remaining is None else min(remaining,1024*1024))
            if not part:
                if remaining: _fail('history_changed', 'Truncated native rollout')
                break
            digest.update(part)
            if remaining is not None: remaining -= len(part)
    return digest.hexdigest()


def _digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def _inventory(home):
    """Known native files only; no credential/config reads; never open a database."""
    all_files, payload = {}, {}
    for directory, dirs, files in os.walk(home, followlinks=False):
        parent = Path(directory)
        for name in dirs:
            path = parent/name
            _plain(path, True)
            relative = path.relative_to(home)
            if relative.parts[0] not in {'sessions', 'skills', 'thread-writer-locks', 'shell_snapshots'} and relative.as_posix() not in {'tmp','tmp/arg0'}:
                _fail('unsupported_contents', 'Unsupported directory (preserved): '+str(path))
        for name in files:
            path = parent/name
            relative = path.relative_to(home)
            key = relative.as_posix()
            # Refuse before opening or dereferencing credentials/configuration.
            if name in {'auth.json', 'config.toml', '.env'}:
                _fail('unsupported_contents', 'Credential/config file present; not read: '+str(path))
            _plain(path)
            root_db = len(relative.parts)==1 and any(name == db+'.sqlite'+suffix
                for db in _DATABASES for suffix in ['', '-wal', '-shm'])
            allowed = root_db or key in {'installation_id', '.sandbox_migration'} or (
                relative.parts[0]=='sessions' and path.suffix=='.jsonl') or (
                relative.parts[:2]==('skills','.system')) or (
                key=='thread-writer-locks/.coordination.lock') or (
                relative.parts[0]=='shell_snapshots' and (path.suffix in {'.sh','.ps1'} or
                    re.fullmatch(r'[0-9a-f-]{36}\.tmp-[0-9]+',name)))
            if not allowed:
                _fail('unsupported_contents', 'Unknown native payload; preserved, not omitted: '+str(path))
            all_files[key] = _hash(path)
            if not name.endswith('-shm') and relative.parts[0]!='thread-writer-locks':
                payload[key] = all_files[key]
    return all_files, payload


def _rollout(home, expected_id, payload):
    rollouts = [name for name in payload if name.startswith('sessions/')]
    if len(rollouts)!=1:
        _fail('unsupported_contents', 'Expected exactly one managed rollout; extra/missing sessions preserved')
    path = home/rollouts[0]
    with path.open(encoding='utf-8') as stream:
        first = json.loads(next(stream))
        meta = first.get('payload', {})
        if first.get('type')!='session_meta' or meta.get('id')!=expected_id:
            _fail('identity_mismatch', 'Manifest thread ID does not match native session metadata')
        if meta.get('cli_version')!='0.161.0' or meta.get('history_mode')!='paginated':
            _fail('unsupported_history', 'Only evidenced 0.161.0 paginated history is supported')
        if any(meta.get(k) for k in ('forked_from_id','parent_thread_id','forked_from')):
            _fail('unsupported_history', 'Fork/subagent history is unsupported; preserve source')
        # Do not interpret arbitrary path-like strings or rewrite native records.
        # Media payloads need a separately verified relocation contract.
        def media(value):
            if isinstance(value, dict):
                if value.get('type') in {'input_image','input_audio','input_file','input_video','image','audio','localImage','localAudio','localFile','attachment'}:
                    return True
                return any(media(v) for v in value.values())
            return isinstance(value, list) and any(media(v) for v in value)
        if media(first): _fail('unsupported_history', 'Media/attachment history is unsupported')
        for line in stream:
            record = json.loads(line)
            if media(record): _fail('unsupported_history', 'Media/attachment history is unsupported')
    return rollouts[0]


def _environment(root, home):
    env = {key: os.environ[key] for key in ('PATH','SystemRoot','WINDIR','COMSPEC','PATHEXT') if key in os.environ}
    for key, path in {'HOME':root/'user', 'USERPROFILE':root/'user', 'CODEX_HOME':home,
                      'XDG_DATA_HOME':root/'data', 'XDG_CACHE_HOME':root/'cache',
                      'LOCALAPPDATA':root/'data', 'TMPDIR':root/'tmp', 'TMP':root/'tmp', 'TEMP':root/'tmp'}.items():
        env[key] = str(path)
    return env


def _native_binary(codex_binary, cwd):
    binary = shutil.which(str(codex_binary))
    if binary is None: _fail('native_unavailable', 'Codex executable not found')
    # --version is nonmutating; do not inherit actual HOME or authentication.
    env = {key:os.environ[key] for key in ('PATH','SystemRoot','WINDIR','COMSPEC','PATHEXT') if key in os.environ}
    env.update({key:os.devnull for key in ('HOME','USERPROFILE','CODEX_HOME','XDG_DATA_HOME','XDG_CACHE_HOME','LOCALAPPDATA')})
    version = subprocess.run([binary,'--version'], env=env, cwd=cwd,
                             capture_output=True, text=True, timeout=15, check=True).stdout.strip()
    if version!=NATIVE_VERSION: _fail('native_version', 'Requires exactly '+NATIVE_VERSION+', found '+version)
    return str(Path(binary).resolve()),version


def _preflight_home(source_home, destination_home, expected_thread_id, *, codex_binary, journal_path):
    """Read-only. Caller locks and manifest authority are prerequisites, not inferred.

    Returns eligible plan even while admitted=False; no native app-server runs.
    Version-only subprocess uses no home/config/auth. Raises RelocationError for
    conflicting payload, unsupported history or unsafe paths. No dirs created.
    """
    source, dest = map(_canonical_directory, (source_home,destination_home))
    if source==dest or source in dest.parents or dest in source.parents:
        _fail('unsafe_path', 'Source and final destination must be disjoint')
    expected_id = str(uuid.UUID(expected_thread_id))
    journal = Path(os.path.abspath(journal_path))
    _canonical_directory(journal.parent)
    if any(home==journal.parent or home in journal.parents for home in [source,dest]):
        _fail('unsafe_path', 'Journal must be outside both native homes')
    if os.path.lexists(journal): _fail('journal_exists', 'Recover existing journal before starting another repair')
    source_all, source_payload = _inventory(source)
    _, dest_payload = _inventory(dest)
    rollout = _rollout(source, expected_id, source_payload)
    _rollout(dest, expected_id, dest_payload)
    if source_payload!=dest_payload:
        _fail('copy_conflict', 'Final native payload must be an independent identical copy before first repair')
    for name in source_payload:
        if os.path.samefile(source/name, dest/name):
            _fail('copy_conflict', 'Mutable native payload is hard-linked to original')
    binary, version = _native_binary(codex_binary, journal.parent)
    return dict(capabilities(), source=str(source), destination=str(dest), journal=str(journal),
                expected_thread_id=expected_id, binary=str(Path(binary).resolve()),
                observed_native_version=version, source_files=source_all, source_payload=source_payload,
                source_fingerprint=_digest(source_all), source_identity=_identity(source),
                destination_identity=_identity(dest), rollout=rollout,
                rollout_length=(source/rollout).stat().st_size)


def _save(journal, state, first=False):
    data = json.dumps(state, sort_keys=True, indent=2).encode()
    target = journal if first else journal.with_name(journal.name+'.pending-'+uuid.uuid4().hex)
    fd = os.open(target, os.O_WRONLY|os.O_CREAT|os.O_EXCL, 0o600)
    with os.fdopen(fd,'wb') as stream:
        stream.write(data); stream.flush(); os.fsync(stream.fileno())
    if not first: os.replace(target,journal)
    if os.name=='posix':
        fd=os.open(journal.parent, os.O_RDONLY)
        try: os.fsync(fd)
        finally: os.close(fd)


def _link_target_equal(actual, expected, windows=False):
    # Normalize only Windows' documented native namespace spelling. No alias
    # resolution, relative paths or dot components can pass this exact check.
    def native_spelling(value):
        if value.startswith('\\\\?\\UNC\\'): return '\\\\'+value[8:]
        if value.startswith('\\\\?\\'): return value[4:]
        return value
    return (native_spelling(actual)==native_spelling(expected)) if windows else actual==expected


def _alias_kind(path):
    info=path.lstat()
    tag=getattr(info,'st_reparse_tag',0)
    if tag==0xA0000003 and stat.S_ISDIR(info.st_mode): return 'junction'
    if stat.S_ISLNK(info.st_mode) and tag in (0,0xA000000C): return 'symlink'
    return None


def _exact_alias(source,dest,kind='symlink',identity=None):
    return (_alias_kind(source)==kind and (identity is None or _identity(source)==identity)
            and _link_target_equal(os.readlink(source),str(dest),os.name=='nt'))


def _remove_alias(source,dest,kind,identity=None):
    if not _exact_alias(source,dest,kind,identity):
        _fail('recovery_required','Alias type, identity or target changed; preserve source name and backup')
    # RemoveDirectory removes a junction itself, never its target. Unlink is
    # deliberately restricted to symlinks; neither operation traverses trees.
    if kind=='junction': source.rmdir()
    else: source.unlink()


def _junction_buffer(destination):
    """Documented REPARSE_DATA_BUFFER / IO_REPARSE_TAG_MOUNT_POINT layout."""
    import struct
    target=str(destination)
    if not re.fullmatch(r'[A-Za-z]:\\[^\x00]*',target) or any(p in ('.','..') for p in target.split('\\')):
        _fail('unsupported_alias','Windows junction requires a canonical local drive target; network homes unsupported')
    substitute=('\\??\\'+target).encode('utf-16-le'); display=target.encode('utf-16-le')
    names=substitute+b'\0\0'+display+b'\0\0'
    data=struct.pack('<HHHH',0,len(substitute),len(substitute)+2,len(display))+names
    if len(data)+8>16384: _fail('unsupported_alias','Junction target exceeds native reparse buffer limit')
    return struct.pack('<IHH',0xA0000003,len(data),0)+data


def _set_junction(path,destination):
    """Set mount-point reparse data on our empty directory; no symlink privilege."""
    import ctypes
    from ctypes import wintypes
    data=_junction_buffer(destination)
    kernel=ctypes.WinDLL('kernel32',use_last_error=True)
    kernel.CreateFileW.argtypes=[wintypes.LPCWSTR,wintypes.DWORD,wintypes.DWORD,ctypes.c_void_p,wintypes.DWORD,wintypes.DWORD,wintypes.HANDLE]
    kernel.CreateFileW.restype=wintypes.HANDLE
    kernel.DeviceIoControl.argtypes=[wintypes.HANDLE,wintypes.DWORD,ctypes.c_void_p,wintypes.DWORD,ctypes.c_void_p,wintypes.DWORD,ctypes.POINTER(wintypes.DWORD),ctypes.c_void_p]
    kernel.DeviceIoControl.restype=wintypes.BOOL
    kernel.CloseHandle.argtypes=[wintypes.HANDLE]; kernel.CloseHandle.restype=wintypes.BOOL
    # OPEN_EXISTING, OPEN_REPARSE_POINT|BACKUP_SEMANTICS; deny delete sharing.
    handle=kernel.CreateFileW(str(path),0x40000000,3,None,3,0x02200000,None)
    if handle==ctypes.c_void_p(-1).value: raise ctypes.WinError(ctypes.get_last_error())
    try:
        count=wintypes.DWORD(); buffer=ctypes.create_string_buffer(data)
        if not kernel.DeviceIoControl(handle,0x900A4,buffer,len(data),None,0,ctypes.byref(count),None):
            raise ctypes.WinError(ctypes.get_last_error())
    finally: kernel.CloseHandle(handle)


def _create_alias(source,dest,state,journal):
    if state['alias_kind']=='junction':
        _junction_buffer(dest)  # Reject unsupported paths before creating anything.
        source.mkdir()  # Exclusive; never adopt an existing directory.
        state.update(alias_identity=_identity(source),phase='alias_prepared'); _save(journal,state)
        _set_junction(source,dest)
    else:
        source.symlink_to(dest,target_is_directory=True)
        state['alias_identity']=_identity(source)
    if not _exact_alias(source,dest,state['alias_kind'],state['alias_identity']):
        _fail('path_conflict','Created alias identity/type/target mismatch')
    state['phase']='aliased'; _save(journal,state)


def _native_path_equal(actual, expected, windows=None):
    # Codex on Windows returns canonical \\?\ paths. Only that namespace
    # spelling may differ; do not resolve aliases, fold case or accept hardlinks.
    return _link_target_equal(str(actual),str(expected),os.name=='nt' if windows is None else windows)


def _pid_running(pid):
    if os.name=='nt':
        # os.kill(pid, 0) calls TerminateProcess on Windows: NEVER use it here.
        import ctypes
        from ctypes import wintypes
        kernel=ctypes.WinDLL('kernel32',use_last_error=True)
        kernel.OpenProcess.argtypes=[wintypes.DWORD,wintypes.BOOL,wintypes.DWORD]
        kernel.OpenProcess.restype=wintypes.HANDLE
        kernel.GetExitCodeProcess.argtypes=[wintypes.HANDLE,ctypes.POINTER(wintypes.DWORD)]
        kernel.GetExitCodeProcess.restype=wintypes.BOOL
        kernel.CloseHandle.argtypes=[wintypes.HANDLE]
        handle=kernel.OpenProcess(0x1000,False,pid)  # QUERY_LIMITED_INFORMATION only
        if not handle:
            if ctypes.get_last_error()==87: return False  # nonexistent PID
            _fail('recovery_required','Cannot query recorded native Windows process')
        try:
            code=wintypes.DWORD()
            if not kernel.GetExitCodeProcess(handle,ctypes.byref(code)):
                _fail('recovery_required','Cannot establish native Windows exit')
            return code.value==259  # STILL_ACTIVE
        finally: kernel.CloseHandle(handle)
    try: os.kill(pid,0)
    except ProcessLookupError: return False
    except PermissionError: _fail('recovery_required','Cannot establish native child exit')
    return True


def _recover(journal_path, *, native_stopped=False):
    """Restore original home under caller-held locks; never touch final payload.

    native_stopped is caller attestation after actual owner/process verification
    for a crashed helper. Never kill a PID found in a journal. Unknown/live child
    or path identity conflict => recovery_required; preserve journal and backup.
    Idempotent once source is restored. No inference or native process started.
    """
    journal=Path(os.path.abspath(journal_path)); _canonical_directory(journal.parent)
    info=_plain(journal)
    if os.name=='posix' and (info.st_uid!=os.getuid() or info.st_mode & 0o022):
        _fail('unsafe_journal', 'Journal ownership/permissions invalid')
    state=json.loads(journal.read_text())
    if state.get('schema')!=1: _fail('unsafe_journal', 'Unsupported journal schema')
    source,dest,backup=map(Path,(state['source'],state['destination'],state['backup']))
    token=uuid.UUID(state['transaction']).hex
    if backup!=source.with_name('.'+source.name+'.pira-relocate-'+token):
        _fail('unsafe_journal', 'Backup does not belong to journal transaction')
    for p in (source,dest,backup):
        if not p.is_absolute(): _fail('unsafe_journal','Journal paths must be absolute')
        _canonical_directory(p.parent)
    if source==dest or source in dest.parents or dest in source.parents:
        _fail('unsafe_journal', 'Overlapping journal homes')
    if state.get('native_uncertain') and not native_stopped:
        raise RelocationError('recovery_required','Verify native child shutdown before recovery',journal=journal)
    pid=state.get('backend_pid')
    if pid is not None:
        if type(pid) is not int or pid<=0: _fail('unsafe_journal','Invalid native PID')
        if _pid_running(pid):
            raise RelocationError('recovery_required','Recorded native PID still exists (or was reused)',journal=journal)
    kind=state.get('alias_kind','symlink')
    if kind not in ('symlink','junction'): _fail('unsafe_journal','Unknown alias kind')
    exists=os.path.lexists(source)
    original=exists and _alias_kind(source) is None and _identity(source)==state['source_identity']
    if original:
        _plain(source,True)
    else:
        _plain(backup,True)
        if _identity(backup)!=state['source_identity'] or _digest(_inventory(backup)[0])!=state['source_fingerprint']:
            raise RelocationError('recovery_required','Backup identity/content changed; preserve both homes',journal=journal)
        if exists:
            identity=state.get('alias_identity')
            if 'alias_kind' in state and identity is None:
                _fail('recovery_required','Unrecorded alias identity; preserve source name and backup')
            if (kind=='junction' and state['phase']=='alias_prepared' and _alias_kind(source) is None
                    and _identity(source)==identity):
                _plain(source,True); source.rmdir()  # Only our still-empty staged directory.
            else: _remove_alias(source,dest,kind,identity)
        if os.path.lexists(source): _fail('recovery_required','Source name occupied during recovery')
        backup.rename(source)
    if _digest(_inventory(source)[0])!=state['source_fingerprint']:
        raise RelocationError('recovery_required','Restored source fingerprint changed',journal=journal)
    state.update(phase='source_restored', native_uncertain=False, backend_pid=None)
    _save(journal,state)
    return {'source_restored':True,'journal':str(journal),'source_fingerprint':state['source_fingerprint']}


def recover(journal_path, *, native_stopped=False):
    """Recover only the journal-owned source under caller locks; see _recover."""
    try:
        return _recover(journal_path,native_stopped=native_stopped)
    except RelocationError as error:
        if error.journal is None: error.journal=str(journal_path)
        raise
    except (OSError,ValueError,KeyError,TypeError) as error:
        raise RelocationError('recovery_required','Recovery could not safely complete: '+str(error),journal=journal_path) from error


class _NativeSession(AbstractContextManager):
    methods = _METHODS
    def __init__(self, plan, runtime, on_pid):
        self.plan,self.runtime,self.on_pid=plan,runtime,on_pid
        self.process=None; self.closed=True; self.sequence=0; self.messages=queue.Queue()
    def __enter__(self):
        self.runtime.mkdir(mode=0o700)
        home=Path(self.plan['destination'])
        env=_environment(self.runtime,home)
        for key in ('HOME','XDG_DATA_HOME','XDG_CACHE_HOME','TMPDIR'): Path(env[key]).mkdir(parents=True,exist_ok=True)
        args=[self.plan['binary'],'app-server','--strict-config','--stdio']
        for setting in _CONFIG: args+=['-c',setting]
        self.stderr=(self.runtime/'native.stderr').open('wb')
        try:
            self.process=subprocess.Popen(args,cwd=self.runtime,env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,
                stderr=self.stderr,text=True,encoding='utf-8',start_new_session=os.name=='posix')
            self.closed=False; self.on_pid(self.process.pid)
            def read():
                try:
                    for line in self.process.stdout: self.messages.put(json.loads(line))
                except Exception as error: self.messages.put(error)
                finally: self.messages.put(None)
            self.reader=threading.Thread(target=read,daemon=True); self.reader.start()
            self.request('initialize',{'clientInfo':{'name':'pira_relocation','version':'1'},
                                      'capabilities':{'experimentalApi':False}})
            self.process.stdin.write('{"method":"initialized"}\n'); self.process.stdin.flush()
            return self
        except BaseException:
            self.close(); raise
    def request(self,method,params):
        if method not in self.methods: _fail('unsafe_rpc','RPC not allowed by no-inference relocation')
        self.sequence+=1
        self.process.stdin.write(json.dumps({'id':self.sequence,'method':method,'params':params})+'\n')
        self.process.stdin.flush()
        deadline=time.monotonic()+20
        while True:
            value=self.messages.get(timeout=max(.01,deadline-time.monotonic()))
            if not isinstance(value,dict): _fail('native_protocol','Native stdout ended or malformed')
            if 'id' in value and 'method' in value: _fail('native_protocol','Unexpected native request')
            if value.get('id')==self.sequence:
                if 'error' in value: _fail('native_protocol',method+': '+json.dumps(value['error']))
                return value['result']
            if time.monotonic()>=deadline: _fail('native_timeout',method+' timed out')
    def close(self):
        if self.process is not None and not self.closed:
            try: self.process.stdin.close()
            except BrokenPipeError: pass
            try: self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                if os.name=='posix': os.killpg(self.process.pid,signal.SIGKILL)
                else: self.process.kill()
                self.process.wait(timeout=5)
            self.closed=True
            if hasattr(self,'reader'): self.reader.join(timeout=2)
            self.process.stdout.close()
        if hasattr(self,'stderr'): self.stderr.close()
        if self.process is not None and self.closed and self.process.returncode != 0:
            _fail('native_exit','Native backend exited unsuccessfully: '+str(self.process.returncode))
    def __exit__(self,*_): self.close()


def _history(peer, thread_id):
    history={}
    for method in ('thread/turns/list','thread/items/list'):
        rows=[]; seen=set(); cursor=None
        for _ in range(100000):
            params={'threadId':thread_id,'limit':_PAGE_SIZE,'sortDirection':'asc'}
            if cursor is not None: params['cursor']=cursor
            reply=peer.request(method,params); rows.extend(reply['data']); cursor=reply.get('nextCursor')
            if cursor is None: break
            if not isinstance(cursor,str) or cursor in seen: _fail('native_protocol','Repeated/invalid history cursor')
            seen.add(cursor)
        else: _fail('unsupported_history','History exceeds verification bound; not truncated')
        history[method]=rows
    return history


def _metadata(reply, expected_id, target):
    t=reply['thread']
    if t.get('id')!=expected_id or t.get('historyMode')!='paginated': _fail('identity_mismatch','Native identity/history mode changed')
    if t.get('forkedFromId') or t.get('parentThreadId'): _fail('unsupported_history','Fork/subagent native metadata')
    if t.get('status',{}).get('type') not in ('idle','notLoaded'): _fail('active_native','Native thread is not idle')
    if not _native_path_equal(Path(t['path']).resolve(),target):
        # Failure-only evidence: Windows extended namespace spelling and a
        # stale source index are materially different. Never admit either on
        # string normalization or samefile alone (an alias can mask staleness).
        observed=Path(t['path'])
        detail={'native_path':str(observed),'expected_path':str(target)}
        for name,path in (('native',observed),('expected',target)):
            try:
                detail[name+'_resolved']=str(path.resolve(strict=True))
                stat=path.stat(); detail[name+'_identity']=[stat.st_dev,stat.st_ino]
            except OSError as error: detail[name+'_error']=repr(error)
        try: detail['samefile']=os.path.samefile(observed,target)
        except OSError as error: detail['samefile_error']=repr(error)
        _fail('identity_mismatch','Native rollout does not resolve to final destination: '+json.dumps(detail,sort_keys=True))


def _repair(plan, session_factory=_NativeSession):
    """Internal transaction; tests only until public admission is enabled."""
    source,dest,journal=map(Path,(plan['source'],plan['destination'],plan['journal']))
    token=uuid.uuid4().hex; backup=source.with_name('.'+source.name+'.pira-relocate-'+token)
    state=dict(plan,schema=1,transaction=token,backup=str(backup),phase='prepared',native_uncertain=False,backend_pid=None,
               alias_kind='junction' if os.name=='nt' else 'symlink')
    if os.path.lexists(backup): _fail('path_conflict','Backup path occupied')
    _save(journal,state,first=True)
    error=None; evidence=None
    try:
        if _identity(source)!=plan['source_identity'] or _identity(dest)!=plan['destination_identity']:
            _fail('copy_conflict','Home identity changed since preflight')
        if _inventory(source)[0]!=plan['source_files'] or _inventory(dest)[1]!=plan['source_payload']:
            _fail('copy_conflict','Payload changed since preflight')
        source.rename(backup); state['phase']='backed_up'; _save(journal,state)
        if os.path.lexists(source): _fail('path_conflict','Source name occupied before alias')
        _create_alias(source,dest,state,journal)
        expected=plan['expected_thread_id']; target=dest/plan['rollout']; baseline=None; model_identity=None
        for number,phase in enumerate(('alias_resume','alias_restart','without_alias','without_alias_restart')):
            if phase=='without_alias':
                _remove_alias(source,dest,state['alias_kind'],state['alias_identity'])
            state.update(phase=phase,native_uncertain=True,backend_pid=None); _save(journal,state)
            def on_pid(pid):
                state['backend_pid']=pid; _save(journal,state)
            peer=session_factory(plan,journal.parent/(journal.name+'.native-'+str(number)),on_pid)
            try:
                with peer:
                    if baseline is None:
                        initial=peer.request('thread/read',{'threadId':expected,'includeTurns':False})
                        _metadata(initial,expected,target)
                        model_identity=tuple(initial['thread'].get(k) for k in ('modelProvider','model'))
                        baseline=_history(peer,expected)
                    result=peer.request('thread/resume',{'threadId':expected})
                    _metadata(result,expected,target)
                    if tuple(result['thread'].get(k) for k in ('modelProvider','model'))!=model_identity:
                        _fail('identity_mismatch','Original model/provider changed during first repair')
                    if not _native_path_equal(result['thread']['path'],target): _fail('identity_mismatch','Native returned old lexical path')
                    if _history(peer,expected)!=baseline: _fail('history_changed','Full native history changed during repair')
            finally:
                if peer.closed:
                    state.update(native_uncertain=False,backend_pid=None); _save(journal,state)
            if phase.startswith('without_alias') and os.path.lexists(source): _fail('path_conflict','Native recreated source path')
            if _hash(target,plan['rollout_length'])!=plan['source_payload'][plan['rollout']]:
                _fail('history_changed','Original rollout bytes no longer preserved')
        evidence={'api':1,'thread_id':expected,'native_version':plan['observed_native_version'],
                  'platform':plan['platform'],'destination':str(dest),'rollout':str(target),
                  'source_fingerprint':plan['source_fingerprint'],'history_fingerprint':_digest(baseline),
                  'history_prefixes':{method:{'count':len(rows),'sha256':_digest(rows)} for method,rows in baseline.items()},
                  'alias_absent_restarts':2,'alias_kind':state['alias_kind'],'journal':str(journal)}
        state['phase']='verified'; _save(journal,state)
    except BaseException as exc: error=exc
    try:
        restored=recover(journal)
    except BaseException as exc:
        raise RelocationError('recovery_required','Source restoration incomplete; preserve final target and journal: '+str(exc),journal=journal) from error
    if error is not None:
        raise RelocationError('repair_failed','Native repair failed in '+state['phase']+'; original source restored: '+str(error),journal=journal) from error
    evidence.update(restored)
    return evidence


# Run-level integration API. Internal home APIs are fixture/implementation details.
def _managed_home(run, run_id, thread_id):
    run=_canonical_directory(run)
    manifest_path=run/'manifest.json'; _plain(manifest_path)
    manifest=json.loads(manifest_path.read_text(encoding='utf-8'))
    if run.name!=run_id or manifest.get('run_id')!=run_id or manifest.get('thread_id')!=thread_id:
        _fail('identity_mismatch','Run directory/manifest expected identity mismatch')
    if manifest.get('schema_version') not in (3,4) or manifest.get('transport')!='app-server':
        _fail('unsupported_run','Requires known Team schema 3/4 retained app-server run')
    if manifest.get('status') not in {'completed','needs_decision','incomplete','interrupted','timed_out','failed'}:
        _fail('active_run','Run status is active/ambiguous; caller must establish idle ownership')
    return _canonical_directory(run/'codex-home')


def inspect_source(source_run, expected_run_id, expected_thread_id, *, codex_binary):
    """Read-only planning before the destination exists; no copy/repair implied."""
    try:
        home=_managed_home(source_run,expected_run_id,expected_thread_id)
        files,payload=_inventory(home)
        rollout=_rollout(home,expected_thread_id,payload)
        binary,version=_native_binary(codex_binary,home.parent)
        return dict(capabilities(),run_id=expected_run_id,thread_id=expected_thread_id,
                    source=str(home),source_fingerprint=_digest(files),rollout=rollout,
                    observed_native_version=version,binary=binary)
    except RelocationError: raise
    except (OSError,ValueError,KeyError,StopIteration,subprocess.SubprocessError) as error:
        raise RelocationError('preflight_failed','Read-only source inspection failed: '+str(error)) from error


def preflight(source_run, final_destination_run, expected_run_id, expected_thread_id, *, codex_binary, journal_path):
    """Read-only first-copy preflight. Does not acquire caller-owned run locks.

    admitted=False is a hard production barrier, NOT a warning. No app-server,
    native resume, alias, journal or temporary directory is created by preflight.
    """
    try:
        source=_managed_home(source_run,expected_run_id,expected_thread_id)
        dest=_managed_home(final_destination_run,expected_run_id,expected_thread_id)
        journal=Path(os.path.abspath(journal_path))
        if any(run==journal.parent or run in journal.parents for run in (source.parent,dest.parent)):
            _fail('unsafe_path','Journal/runtime must be outside both managed runs')
        plan=_preflight_home(source,dest,expected_thread_id,codex_binary=codex_binary,journal_path=journal_path)
        plan.update(run_id=expected_run_id,source_run=str(source.parent),destination_run=str(dest.parent))
        return plan
    except RelocationError: raise
    except (OSError,ValueError,KeyError,StopIteration,subprocess.SubprocessError) as error:
        raise RelocationError('preflight_failed','Read-only preflight failed: '+str(error)) from error


def repair(source_run, final_destination_run, expected_run_id, expected_thread_id, *, codex_binary, journal_path):
    """Apply-only transactional first repair. Not for receipt reruns or dry-run.

    Success restores the original source and returns native identity/history
    evidence. Caller durably publishes its receipt only after this returns.
    Failure preserves the final target for caller quarantine, never a success.
    """
    plan=preflight(source_run,final_destination_run,expected_run_id,expected_thread_id,
                   codex_binary=codex_binary,journal_path=journal_path)
    if not plan['admitted']:
        _fail('capability_pending','Native platform admission pending; no mutation performed')
    evidence=_repair(plan)
    return dict(evidence,run_id=expected_run_id,source_run=plan['source_run'],destination_run=plan['destination_run'])


def validate_identity(source_run, final_destination_run, expected_run_id, expected_thread_id):
    """Read-only source-lineage evidence, suitable for planning/receipt checks.

    Rejects missing/replacement/truncated rollout; accepts destination-only
    append history. Does NOT prove private native indexes are healthy/resumable.
    Use apply-only verify_native_identity for that stronger check, never in
    --dry-run/--verify. Caller also validates its durable receipt/source identity.
    """
    try:
        source=_managed_home(source_run,expected_run_id,expected_thread_id)
        dest=_managed_home(final_destination_run,expected_run_id,expected_thread_id)
        source_all,source_payload=_inventory(source); _,dest_payload=_inventory(dest)
        rollout=_rollout(source,expected_thread_id,source_payload)
        if _rollout(dest,expected_thread_id,dest_payload)!=rollout:
            _fail('identity_mismatch','Original retained rollout replaced')
        for name in ('state_5.sqlite','thread_history_1.sqlite'):
            if name not in dest_payload or not (dest/name).stat().st_size:
                _fail('identity_mismatch','Required native index missing: '+name)
        length=(source/rollout).stat().st_size
        if _hash(dest/rollout,length)!=source_payload[rollout]:
            _fail('identity_mismatch','Original retained history prefix missing or replaced')
        return {'run_id':expected_run_id,'thread_id':expected_thread_id,'rollout':str(dest/rollout),
                'source_fingerprint':_digest(source_all),'native_verified':False,
                'verification':'read_only_rollout_lineage'}
    except RelocationError: raise
    except (OSError,ValueError,KeyError,StopIteration) as error:
        raise RelocationError('identity_mismatch','Read-only native identity check failed: '+str(error)) from error


def verify_native_identity(source_run, final_destination_run, expected_run_id, expected_thread_id, *,
                           evidence, codex_binary, runtime_dir):
    """Apply-only continuation verification; NO bridge and NO raw target equality.

    Caller supplies original repair evidence/history_prefixes from its protected
    receipt. Later destination-only turns are permitted; lost original native
    history is not. This mutates native runtime/index state and must NEVER be
    called by dry-run/verify. Production capability gate remains mandatory.
    """
    identity=validate_identity(source_run,final_destination_run,expected_run_id,expected_thread_id)
    if not capabilities()['admitted']: _fail('capability_pending','Native apply verification not admitted on this platform')
    return _verify_native_identity(source_run,final_destination_run,expected_run_id,expected_thread_id,
                                   evidence=evidence,codex_binary=codex_binary,runtime_dir=runtime_dir,identity=identity)


def _verify_native_identity(source_run, final_destination_run, expected_run_id, expected_thread_id, *,
                            evidence,codex_binary,runtime_dir,identity=None):
    if identity is None: identity=validate_identity(source_run,final_destination_run,expected_run_id,expected_thread_id)
    if evidence.get('thread_id')!=expected_thread_id or evidence.get('run_id')!=expected_run_id or evidence.get('source_fingerprint')!=identity['source_fingerprint']:
        _fail('identity_mismatch','Original receipt identity/source fingerprint mismatch')
    if evidence.get('destination')!=str(Path(final_destination_run)/'codex-home'):
        _fail('identity_mismatch','Receipt native destination mismatch')
    binary,version=_native_binary(codex_binary,Path(final_destination_run))
    runtime=Path(os.path.abspath(runtime_dir)); _canonical_directory(runtime.parent)
    if any(run==runtime or run in runtime.parents for run in (Path(source_run),Path(final_destination_run))):
        _fail('unsafe_path','Verification runtime must be outside both managed runs')
    if os.path.lexists(runtime): _fail('path_conflict','Verification runtime directory already exists')
    plan={'binary':binary,'destination':str(Path(final_destination_run)/'codex-home')}
    target=Path(identity['rollout'])
    with _NativeSession(plan,runtime,lambda pid:None) as peer:
        # Refuse stale source resolution BEFORE potentially mutating resume.
        _metadata(peer.request('thread/read',{'threadId':expected_thread_id,'includeTurns':False}),expected_thread_id,target)
        reply=peer.request('thread/resume',{'threadId':expected_thread_id}); _metadata(reply,expected_thread_id,target)
        if not _native_path_equal(reply['thread']['path'],target): _fail('identity_mismatch','Native identity resolves outside final destination')
        history=_history(peer,expected_thread_id)
        if set(evidence.get('history_prefixes',{}))!=set(history): _fail('identity_mismatch','Receipt history evidence missing')
        for method,rows in history.items():
            original=evidence['history_prefixes'][method]; count=original['count']
            if type(count) is not int or count<0 or len(rows)<count or _digest(rows[:count])!=original['sha256']:
                _fail('identity_mismatch','Original native paginated history missing/replaced')
    final=validate_identity(source_run,final_destination_run,expected_run_id,expected_thread_id)
    if final['source_fingerprint']!=identity['source_fingerprint']: _fail('identity_mismatch','Original source changed during native verification')
    return dict(final,native_verified=True,verification='native_same_id_original_history_prefix')
