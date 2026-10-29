pira_team — retained technical artifact workers for review and implementation
Usage: pira_team run --task TASK --completion-gate TEXT [--inject-review] [--inject-implement] [--inject-fig] [--inject-write] [OPTIONS]
       pira_team resume RUN_ID [--inject-review] [--inject-implement] [--inject-fig] [--inject-write] [--task TASK --completion-gate TEXT] [OPTIONS]
       pira_team tier RUN_ID standard|fast [--store DIR]
       pira_team steer RUN_ID --task TASK --completion-gate TEXT [--store DIR]
       pira_team interrupt [-r|--recursive] RUN_ID [--store DIR]
       pira_team read|path RUN_ID [RELATIVE_FILE] [--store DIR]
       pira_team wait RUN_ID [--store DIR]
       pira_team resolve REQUEST_ID [--store DIR]
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
Run/resume accept --service-tier standard|fast. New runs default to standard; resume retains
the chosen run tier. Tier controls require a new-feature active local owner, including repair.
They publish settings for subsequent captures in the current turn, never interrupt, replay,
restart, inject input or create a turn. Captured steps and frozen consumers are unchanged.
Receipts distinguish desired tier/publication from actual tier (unknown), not billed speed.
Only an applied acknowledgement updates run intent; failed persistence reports unknown durability.
Rejected/unconfirmed publication leaves future intent unchanged. Never blindly retry unknown delivery.
No active turn or unsupported owner is an error; no migration or automatic next-turn fallback.
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
Concurrent resumes and unsettled running state fail closed; resume never replays an
abandoned attempt automatically. Use interrupt -r RUN_ID for guarded settlement,
then inspect retained diagnostics and external side effects before deliberate resume.
Recovery requires the exclusive owner lock, current-attempt identity evidence, verified
launcher/backend exit and an empty owned process group or Windows job. Missing or stale
evidence (including a crash before backend binding) fails closed. An unavailable endpoint
or missing PID alone is insufficient. No persisted PID/group is signalled for termination.
Recovered abandoned attempts are interrupted, never completed, with incomplete usage.
Legacy ephemeral runs cannot resume.
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
Worker shell PATH starts with the canonical invoking Team launcher's directory, so bare
pira_team child commands use that same launcher, including staged local coordinators.
Other inherited PATH entries retain their order. Backend process PATH and the selected
Codex executable/provider remain unchanged; executable-equality authentication is retained.
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


Remote execution (opt-in)
Only a verified root main chooses --host. It may launch an individual worker or a
complete --allow-subteam coordinator tree on one Linux machine. All children stay
on that host and use native server-local admission, ownership, usage and recursive
controls. Coordinators/ordinary workers cannot select a host or make remote hops.
Default local commands, stores and old runs keep their existing semantics; no
local run is automatically migrated. Remote filesystem paths grant no local scope.

Normal remote launch selects the endpoint ONCE:
  pira_team run --host HOST --remote-user USER --remote-endpoint-id PUBLIC_NONCE \
    --remote-exe ABS --remote-backend ABS --remote-store ABS --remote-cwd ABS \
    --authority-key LOCAL_ABS --task TASK --completion-gate TEXT [--allow-subteam] \
    [--store LOCAL_TEAM_STORE] [--label LABEL] [--detach]
The launcher durably retains a signed request and ordinary run_id before contacting
the server; stderr prints that ID before submission. Request IDs are automatic.
After launch use ordinary read/resume/steer/interrupt/wait RUN_ID (or @LABEL),
without endpoint, key or controller options. Use the same local Team store as at
launch. Explicit --remote-state remains a private transport controller location;
it does not change the ordinary route store or require that option on followups.
Bookmarks/list/search use the same main-session-scoped local index as local runs.

read RUN_ID returns raw handoff bytes; read RUN_ID manifest.json returns the raw
manifest, including descendant summaries; read RUN_ID artifacts/report.md returns
that raw UTF-8 file. Remote file reads are limited to 2 MiB. wait RUN_ID waits for
retained settlement without starting a model turn, for local or remote runs.
Normal remote receipts have run_id/status/run_root/handoff_path; the two path
fields are null because server files are not local filesystem paths. Remote path
fails explicitly: use read to retrieve files. Live remote tier control and remote
schema files are unsupported; launch/resume --service-tier remains supported.

After network loss, use ordinary read RUN_ID manifest.json or wait RUN_ID.
The server worker continues independently, stops normally and retains results.
An accepted operation is observed, not replayed. An absent initial submission may
be reconciled with its exact retained allocation/request and current equivalent
execution authority. An absent pending resume is NOT submitted by read/wait;
only an explicit matching resume may submit it. Pending controls/resumes bind
stable IDs, payload and target epoch before transport; changed uncertain intent
fails closed. A successful full-access observation settles accepted uncertain
resume delivery. It can separately retire an old control when the native epoch
has changed, retaining its signed identity and unknown outcome, not claiming an
acknowledgement or reissuing it. Reads remain available after policy tightening; no implicit
model turn is created to recover a result. No notification or app wakeup is
required for recovery, and the hosting app may not wake an offline MAIN.
Routes and authority remain bound to the original verified root main session;
an unrelated fresh session cannot claim a job. Local routing records are private
launcher metadata, never fabricated native manifests or worker input.

Explicit request-ID compatibility transport is retained below. Its launch also
registers an ordinary run_id in the default local Team store BEFORE submission,
even with an explicit --remote-state. If stdout/run_id is lost, public
  pira_team resolve REQUEST_ID [--store LOCAL_TEAM_STORE]
returns the retained ordinary handle without contacting the server or starting
work. Its status is unknown until read/wait observes the server. MAIN never needs
to inspect private routing records or reconstruct endpoint options to recover.

Lower-level compatibility transport (explicit --request-id; structured views):
Usage: pira_team remote-keygen --authority-key LOCAL_ABS
       pira_team run REMOTE --remote-cwd ABS --task TASK --completion-gate TEXT [--allow-subteam] [--detach]
       pira_team resume REMOTE --run-request ORIGINAL_REQUEST [--allow-subteam] [--detach] [--task TASK --completion-gate TEXT]
       pira_team attach REMOTE
       pira_team read REMOTE [--artifact | --file RELATIVE_FILE | --operation-id KEY]
       pira_team steer REMOTE --operation-id KEY --task TASK --completion-gate TEXT
       pira_team interrupt REMOTE --operation-id KEY [-r|--recursive]
       pira_team remote-check REMOTE
REMOTE means all of:
  --host HOST --remote-user USER --remote-endpoint-id PUBLIC_NONCE
  --remote-exe ABS --remote-backend ABS
  --remote-store ABS --remote-state LOCAL_ABS --authority-key LOCAL_ABS --request-id KEY
HOST is an SSH alias/hostname, not shell syntax; USER explicitly selects the SSH
account. Remote executable/backend/store/cwd are explicit absolute canonical Linux
paths, without . or .. components. --remote-exe is the actual candidate binary,
not a wrapper; --remote-backend selects the existing server-side Codex executable.
Candidate/backend directories enter the supervisor PATH for server-local tools.
Remote workers inherit the server launcher environment, not the local client's.
--remote-endpoint-id is the independently provisioned 64-hex PUBLIC endpoint nonce,
not a credential; use a different nonce for each independent host/candidate enrollment.
--remote-state and --authority-key are local launcher-private paths, never remote
workspace paths. --task-file is local input sent as task text; -- TASK is supported.
Lower-level --cwd/--store/--schema/--label are rejected. Remote run/resume accept --service-tier standard|fast with the same default/retention
rules as local runs. Live tier changes via --host are unsupported; no remote tier
control operation is defined. Endpoint-side bookmarks, config and path are unsupported. Lower-level read defaults to a structured manifest view; --artifact or --file
adds UTF-8 content up to 2 MiB, never credentials/backend state. --operation-id reads
a retained control receipt instead. Root cannot read grandchildren's
content; a coordinator's root-visible manifest retains descendant summaries only.

Provision trust separately, before submission: remote-keygen verifies the local
root identity and full-access/approval-never context, generates an Ed25519 key via
ssh-keygen, and prints the PUBLIC allowed-signers enrollment line. A trusted
operator/root installs only that public line in the fixed sibling file
<actual-remote-executable>.allowed-signers, owned by OS root/the SSH account with
safe ancestors and no group/other writes. Submit cannot enroll a key or choose a
trust file. Enroll ONE exact root principal per candidate; same-principal key
rotation is allowed, but wildcard/CSV/multiple-root enrollment is rejected.
Separately provision <actual-remote-executable>.remote-endpoint.json:
{"version":2,"endpoint_id":"64hex public nonce","user":"SSH username","exe":"canonical candidate path"}.
The server checks this fixed descriptor, not just a caller-supplied host alias.
Never send the private key, local sessions or credentials to the server,
task, environment or logs. Protect the unencrypted private authority key locally.
OpenSSH ssh and ssh-keygen signing/verification support are required. Transport
uses strict host-key checks, batch mode, no PTY and no agent forwarding; no public
listener or system service is installed. Backend authentication stays server-side
and launcher-managed. remote-check reports availability, not model/auth validity.
remote-check executes the selected backend's --version and requires fresh
full-access/approval-never. Before effects, clients separately negotiate signed
protocol/scope/capabilities using policy-agnostic metadata reads: these do not
probe the backend, allocate runs, or write controller state. Missing required
capabilities or protocol/scope mismatch fails before submission/control effects.
Resume checks authenticated retained topology, including coordinator promotion by
an earlier resume; omitting --allow-subteam later does not remove its capability requirement.

Signed protocol v2 grants bind root principal, host/user/executable/backend/store,
operation, request/run, task/argv and execution policy, with bounded validity.
The trust boundary is a protected trusted launcher/signing key and trusted SSH
endpoint, not remote attestation of a local process. Task data is untrusted; same
OS user is not isolation from reading/modifying private stores or key material.
Run/resume/steer require fresh full-access/approval-never; restricted filesystem
policy translation is unsupported, never elevated. Verified same-root read, attach
and interrupt remain available after caller policy tightening without local state
writes. Steer must match the active execution policy. Only the root can grant
--allow-subteam; children cannot inherit delegation authority for another level.

Submission persists its request identity before acknowledgement. Retry the SAME
--request-id and identical payload to reconcile uncertain delivery; a different
payload under that ID rejects. Resume uses a NEW --request-id plus --run-request
for the original submission. Attach/read never resume or inject a model turn.
Distinct request IDs share a controller directory without a global client lock.
Same-ID concurrent persistence can fail busy; its immutable signed request and
server ownership/deduplication rules remain unchanged. No network effect is retried
automatically, and server lifetime ownership locks are not relaxed.
--run-request must select that original run request, never another resume.
--detach returns durable acceptance, not worker completion; otherwise attach waits.
Lower-level results are structured JSON views/receipts, not local filesystem paths.
Dropping SSH/attach means unknown connectivity, not worker success or stoppage.
The detached supervisor retains state independently; the SSH server must permit
retained same-user processes. No system-service/cgroup-policy bypass is attempted.
Partial records/crashed claims fail closed without automatic replay; accepted
expired requests may reconcile only. Inspect history, errors, artifacts, usage and
external effects before deliberate resume; crash accounting can remain incomplete.
No exactly-once guarantee is made for arbitrary task side effects. Controls bind
one revision/turn epoch. After unknown control delivery, reconcile the SAME
--operation-id; do not reissue an effect under a new ID. A stale control cannot
act on a later resumed turn. The client reads the retained operation receipt first;
changed intent/epoch under the same operation ID rejects. Use read --operation-id
to inspect unknown delivery without resending an effect.
Acknowledgement is acceptance, not owner exit: await
settlement before resume. Control command exit success delivers a receipt; inspect
result.status and completion_confirmed, including failed_or_unknown/unknown states.
Remote interrupt targets only the authorized scope;
-r/--recursive applies the native host-local subtree boundary.
