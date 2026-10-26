pira_team — retained technical artifact workers for review and implementation
Usage: pira_team run --task TASK --completion-gate TEXT [--inject-review] [--inject-implement] [--inject-fig] [--inject-write] [OPTIONS]
       pira_team resume RUN_ID [--inject-review] [--inject-implement] [--inject-fig] [--inject-write] [--task TASK --completion-gate TEXT] [OPTIONS]
       pira_team steer RUN_ID --task TASK --completion-gate TEXT [--store DIR]
       pira_team interrupt [-r|--recursive] RUN_ID [--store DIR]
       pira_team read|path RUN_ID [RELATIVE_FILE] [--store DIR]
       pira_team bookmark RUN_REF --label LABEL [--store DIR]
       pira_team unbookmark LABEL [--store DIR]
       pira_team bookmarks [--limit N] [--store DIR]
       pira_team search QUERY [--limit N] [--store DIR]
       pira_team config show [--store DIR]
       pira_team config set --main MODEL --model WORKER --effort EFFORT [--store DIR]
       pira_team config reset --main MODEL [--store DIR]

Use one quoted --task (also --task=TEXT) or --task-file FILE; the file is INPUT.
Root-only --allow-subteam on run/resume authorizes one coordinator level; children cannot delegate.
Run options: --cwd DIR --store DIR --model MODEL --effort EFFORT --label LABEL
             --format auto|markdown|text|json|csv --schema FILE --columns JSON_ARRAY
             --output artifact|answer
Resume can override model/effort/output. Taskless resume retains the assignment's gate.
A replacement task or steer requires its dedicated --completion-gate; no semantic gate
validation is performed. Completed means the worker reports the gate satisfied.
Needs_decision and incomplete are distinct outcomes, not accepted completion.

Workers always receive main guidance including ctx/nav/dec.
Select only optional guidance directly required by the assigned task; never blanket-inject all policies. Keep applicable quality requirements intact.
--inject-review, --inject-implement, --inject-fig and --inject-write independently add guidance on run/resume.
--inject-fig adds public figure guidance only. --inject-write adds scientific-writing guidance
for explicitly assigned scientific writing and human-facing technical prose, including writing-only tasks.
No injection flag grants edit authority.
Combining --inject-review and --inject-implement on a fresh run reviews first, then
automatically appends implementation guidance in the same thread only after a successful
internal checkpoint; --inject-fig and --inject-write do not change these stages.
Resume continues the retained stage; standalone runs do not retroactively become staged.
Resume retains guidance and adds
only missing selections without replacing the cached base prefix.
`steer` changes only the active task and completion gate; it accepts no injection flags. To add required guidance during active work, interrupt, await the original invocation to finish, inspect partial work, then resume with the missing injection flags. Format repair retains selected guidance and accepts no new policy selection; steering is unavailable during repair. Native subagents and
global/project PIRA loading are disabled. All phases inherit verified caller execution permissions; REVIEW assignments protect
project artifacts, including tests, configuration, docs, lockfiles and expected outputs, by instruction.
Build/test outputs, authorized cache updates, scratch and managed handoffs remain permitted;
commands rewriting protected artifacts require check-only mode or a disposable copy. IMPLEMENTATION assignments authorize only their owned files. Fixes are implementation.
Secrets exclusion and file ownership are instructional, not hard read/file-level boundaries.
No rollback/merge is automatic; failure/interruption may leave partial edits.
--code-review and --allow-fix are deprecated aliases with no permission/eligibility gate;
explicit task instructions prevail. --navigation nav remains accepted; shell is deprecated.

The launcher gives the worker a direct file path in artifacts/ for its handoff. Each resume
has a new immutable-by-contract revision artifact. Default stdout is a JSON receipt with
only run_id, status, run_root and handoff_path. --output answer prints exact
handoff bytes. Nothing is automatically published into the project workspace.
read RUN_ID manifest.json retrieves usage, revisions, repairs, format, completion gate and
artifact/log references; path RUN_ID manifest.json returns its path. Earlier revision metadata
is available at revisions/NNNNNN/manifest.json. Repair and incomplete-accounting warnings
remain on stderr; routine diagnostic metadata stays in manifests.
path RUN_ID returns the handoff; path RUN_ID . returns the run root for scripts.
read/path allow safe relative diagnostic/artifact files and exclude backend credentials/state.
read without a relative filename requires a completed, needs_decision or incomplete outcome.
Store: --store > PIRA_TEAM_DIR > persistent PIRA parent/team.
Parent: macOS HOME/Library/Application Support/PIRA; Linux absolute, nonempty
XDG_DATA_HOME/pira or HOME/.local/share/pira; Windows LOCALAPPDATA/PIRA. Ctx and Dec default to ctx and
decision under the same parent; their explicit environment overrides remain supported.
Older temporary-store runs require --store OLD_STORE or PIRA_TEAM_DIR until explicitly
migrated. Retained runs embed absolute paths: do not simply move/merge run directories.
Use the same store across commands. Read/path never launch a worker.
Bookmarks are optional management metadata, never worker input. Labels are exact,
case-sensitive, main-session-scoped names (1..128 UTF-8 bytes, no edge whitespace,
control characters, / or \; no leading @ or . / .. names). --label attaches after
creation of the initial running manifest; later startup/worker failures retain the
bookmark on that failed/interrupted run. Pre-run validation never moves a bookmark.
Bookmark write failure fails the new run before worker launch. Errors before atomic
replacement preserve prior mappings; a later directory-sync error reports replacement.
The managed bookmarks.json index is bounded to 1 MiB; no automatic pruning is done.
Contended edits wait up to 5 seconds for the metadata lock, then fail busy; readers
observe complete snapshots. This budget never limits worker startup/inference.
bookmark RUN_REF --label LABEL atomically adds/moves a label; its JSON receipt reports
previous/current run references and changed. Same-target assignment is a no-write no-op.
unbookmark LABEL removes only the mapping, never runs/artifacts; absence is a no-op.
bookmarks lists labels; search performs case-insensitive literal keyword matching on
labels only. Both return JSON matches (label, run_id, run_root) and has_more, ordered
by exact label ascending, with --limit 1..1000 (default 20). Invalid/unreadable metadata
fails visibly, never resets or reports partial results. Run ownership is verified before
listing; logs are not searched. Inaccessible pointers are hidden from listings and
previous/current receipts, but own dangling mappings may still be removed.
Use @LABEL as RUN_REF for resume/steer/interrupt/read/path/bookmark. Aliases resolve once
at invocation; plain run IDs are never interpreted as labels. Missing labels fail closed.
All run and bookmark operations require verified main caller identity, including literal
IDs and explicit stores. A run belongs to its creating main session, not workspace or
worker native thread. Resume preserves that creator. Same labels in different sessions
are independent; no global fallback. Read/path and metadata operations launch no workers
and require no backend authentication. Config remains store-wide and session-free.
Legacy global bookmarks are preserved but hidden. Legacy runs require immutable creator
metadata or an original first revision snapshot; an actively locked initial revision is
also recognized. Ambiguous historical runs remain intact but inaccessible. Ownership
checks provide Team-interface isolation only, not OS/filesystem/process security.

--format auto lets the worker declare markdown/text/json/csv for its extensionless handoff.
Explicit formats assign a matching extension. --schema requires json; --columns requires csv
and a JSON array of ordered headers. External schema references are disabled. Validation
checks file delivery and format only, not findings or the completion gate. Decision/incomplete
outcomes bypass the report schema/columns. One file-format repair at most, preserving context
and substantive work; failure exits nonzero with diagnostic paths. Repair keeps current caller permissions
but is instructed to edit only the handoff. Invalid file candidates are retained in repair logs.

Run/resume block without an execution deadline. Stream stderr for run_id and active-turn
notice; await the original invocation, not log polling. A live hung worker can be interrupted.
Controls acknowledge acceptance, not completion; never blindly retry unknown delivery.
Steering is unavailable during startup/repair. Normal interruption retains the conversation.
Steering validates current caller permissions and requires the same represented sandbox policy
as the active endpoint; changed or unverifiable policies require interrupt then explicit resume.
Interruption remains available with verified caller identity even if execution policy changes.
Concurrent resumes and orphaned running state fail closed. Legacy ephemeral runs cannot resume.
Legacy retained runs need an explicit completion gate on first migration; their history is kept.
The original base prefix is retained; current worker instructions are directly injected on
migration, and current handoff/gate are supplied each turn. Cache hits are not guaranteed.

Execution permissions always inherit the matching caller session's latest recorded context,
including on resume; explicit model/effort do not bypass permission verification. Missing,
malformed or unrepresentable policies fail closed. Only approval-never is supported; Team
cannot handle interactive approvals. Read-only callers cannot supply required managed writes.
Fine-grained restricted permission profiles are unsupported by this native API. Workspace-write
callers must place managed Team/tool stores and configured build roots within their writable scope;
Team does not add filesystem/network authority. Full access does not expand task ownership.
On new runs, caller model exactly gpt-6-astra (at any effort) maps to gpt-6.1-sol/high.
Recognized callers gpt-6.1-sol, gpt-6-sol, gpt-6-luna and gpt-5.6-sol inherit model/effort
unchanged. Unknown or missing caller model identity falls back to gpt-6.1-sol/high, not the
caller's effort. These bundled defaults apply unless config supplies an exact caller-model
override; no aliases or network discovery are used. Config show displays bundled policy,
overrides and their file path. Set requires both fields; reset removes only that caller's
override. Config uses worker_defaults.json in the selected Team store, needs no Codex
session, and launches no worker. Invalid config fails visibly without being overwritten;
fix/restore the reported file before retrying. Caller discovery requires a matching Codex session
and verified execution context; fallback does not enable Claude/non-Codex session discovery.
Explicit --model and --effort independently override their respective fields, including
partial overrides. Resume retains stored model/effort unless explicitly overridden; it does
not reapply caller defaults. Missing effort for a recognized caller requires --effort;
missing or ambiguous caller execution context always fails, even with explicit profile flags.
Requires native Codex app-server with the published Team protocol and strict-config support,
plus existing local file auth or CODEX_API_KEY. Preflight generates native schemas without an execution deadline;
missing API fields/methods fail before model work. Sent requests are schema-validated.
Direct stdio is separate from the interactive shared daemon; its version/features are not
Team compatibility evidence. This checks published API inventory and outgoing request shapes,
not model access, notification semantics or actual shell sandbox behavior. Authentication
is launcher-managed and detached after each invocation. All raw logs remain tool-managed.
Worker shells inherit the invoking environment on launch/resume, except session/store identity,
authentication sockets and named credential patterns. Team supplies its own handoff/guard/stores.
Unknown variables pass through; this is not a secret detector. Existing absolute directories in
CARGO_HOME, CARGO_TARGET_DIR, PIP_CACHE_DIR, UV_CACHE_DIR, PYTHONPYCACHEPREFIX, MYPY_CACHE_DIR,
npm_config_cache, NPM_CONFIG_CACHE, YARN_CACHE_FOLDER, GOCACHE, GOMODCACHE, GRADLE_USER_HOME,
CCACHE_DIR, SCCACHE_DIR and XDG_CACHE_HOME are used for build/cache writes only within caller writable scope.
PIRA_TEAM_BUILD_ROOTS adds a JSON array of existing absolute directories for other build systems.
Invalid explicit roots fail; unusable ambient roots grant nothing. Filesystem roots are excluded;
physical paths are deduplicated. Run/resume/repair use current invoking configuration and record
build_roots in manifests. Other inherited variables grant no extra writes. No cache deletion is
authorized; task/ownership constraints still apply. Unix cancellation excludes detached descendants.
Subteams: exactly main -> coordinator -> ordinary children. Authorization is retained and
bound to the actual native backend ancestry and verified thread, not environment assertions.
A coordinator cannot grant --allow-subteam or change config, escape its retained store,
or access siblings, ancestors or root runs. Tags and child content belong to the immediate
parent only. Root read of a coordinator manifest includes descendant activity/usage summaries,
not content. usage is own cumulative usage; descendants.usage is separate, summed once per
retained child across resumes (usage_complete remains explicit).
interrupt is target-only and reports active_children_left_running; -r/--recursive closes a
subtree admission gate before stopping and confirms owner exit or reports explicit failures.
Root recursive interrupt may control descendants but grants no read/tag access. A later
explicit root resume clears the gate only after all owners settle; old children cannot clear
it. A crashed owner with an unconfirmed backend is an explicit failure, never a PID-only kill.
Coordinators must integrate and finish required child work; completed is rejected while any
child remains active. Incomplete/needs_decision preserve child lifecycle visibility.
Legacy runs gain no subteam permission without an explicit root grant. Original cached policy
prefixes are preserved; migration injects current rules. These are Team-interface controls,
not filesystem security against same-user manual tampering.
Windows backend scopes use owned job handles; child launchers explicitly break away from
the coordinator job without escaping caller-owned outer jobs. Backend authentication for
children uses a local OS-peer-verified, one-time launcher handoff and remains in launcher
memory/backend environments only. It is never a shell setting, task, policy, artifact or
control/event-log value; file-backed authentication and shell exclusions are preserved.
Delegated backend executable and provider/proxy/loader selectors remain coordinator-selected;
a worker cannot redirect its inherited authentication through environment overrides. Ordinary
task/build variables remain inherited.
