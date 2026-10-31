# run
pira_team run — start a retained assignment

USAGE
  pira_team run --task TASK --completion-gate TEXT [OPTIONS]
  pira_team run --task-file FILE --completion-gate TEXT [OPTIONS]
  pira_team run [OPTIONS] -- TASK

OPTIONS
  --task TEXT             One quoted task; also --task=TEXT or one positional task.
  --task-file FILE        Input file, mutually exclusive with a task argument.
  --completion-gate TEXT  Required nonempty gate; also --completion-gate=TEXT.
  --cwd DIR              Worker directory (default: invoking directory).
  --store DIR            Team store; see help sessions for defaults and ownership.
  --model MODEL          Override the worker model independently of effort.
  --effort EFFORT        none|minimal|low|medium|high|xhigh|max|ultra.
  --service-tier standard|fast
                         New runs default to standard.
  --label LABEL          Attach a session-scoped bookmark; also --label=LABEL.
  --output artifact|answer
                         Default artifact: JSON receipt. answer: exact handoff bytes.
  --format auto|markdown|text|json|csv
                         Default auto: worker declares the handoff format.
  --schema FILE          Local JSON schema, requires --format json. External references are disabled.
  --columns JSON_ARRAY   Ordered CSV headers, requires --format csv.
  --inject-review        Add review guidance.
  --inject-implement     Add implementation guidance.
  --inject-fig           Add public figure guidance.
  --inject-write         Add scientific-writing guidance.
  --allow-subteam        Root-only grant for one coordinator level.
  --navigation nav|shell Default nav; shell is deprecated. Ctx/Nav/Dec stay injected.
  --code-review, --allow-fix
                         Deprecated compatibility flags; task instructions prevail.
  --host HOST            Opt-in remote launch; see help remote for required options.

Task and gate must be nonempty. -- ends option parsing and requires exactly one
following task. Only the task, gate, and label support local --name=VALUE forms.
Model/effort defaults use exact caller-model mappings and store overrides; see
help config. Execution permissions always require verified caller context.
Guidance flags grant no edit authority. Select only required guidance. Combining
review and implementation on a fresh run creates a staged review/implementation
assignment. See help safety and help manual for retained guidance and constraints.

Run blocks without an execution deadline. --timeout is rejected. Await the original
invocation; use interrupt for a hung worker. Failure can leave partial edits.
The JSON receipt contains run_id, status, run_root, and handoff_path. Format checks
validate delivery/format, not findings or completion. At most one format repair
preserves substantive work. needs_decision/incomplete bypass schema/columns.

EXAMPLES
  pira_team run --task 'Review src/parser.rs' --completion-gate 'Report actionable findings' --inject-review
  pira_team run --task-file assignment.txt --completion-gate 'Focused tests pass' --inject-implement --format markdown
  pira_team run --completion-gate 'Answer delivered' -- '-task starting with a dash'

EXIT STATUS
  0  Report delivered, including needs_decision/incomplete. Inspect status/handoff.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# resume
pira_team resume — continue a retained conversation

USAGE
  pira_team resume RUN_ID [OPTIONS]
  pira_team resume RUN_ID --task TASK --completion-gate TEXT [OPTIONS]

OPTIONS
  --store DIR            Use the original Team store. RUN_ID may be @LABEL.
  --task TEXT            Replacement task; also --task=TEXT or second positional.
  --task-file FILE       Replacement input, mutually exclusive with task text.
  --completion-gate TEXT Replacement gate; also --completion-gate=TEXT.
  --model MODEL          Override retained model.
  --effort EFFORT        Override retained effort.
  --service-tier standard|fast
                        Override retained tier.
  --output artifact|answer
                        Override retained output mode.
  --inject-review, --inject-implement, --inject-fig, --inject-write
                        Add missing guidance, preserving retained selections.
  --allow-subteam       Explicit root grant for one coordinator level.
  --allow-fix           Deprecated compatibility continuation task; requires a gate.
  --                   End options; remaining arguments are positional.

A taskless resume continues the assignment and retains its gate. A replacement
task requires its own gate. Model/effort/output/tier retain stored values unless
overridden; caller defaults are not reapplied. Current verified caller permissions
still apply. Resume retains guidance and stage; standalone runs do not become
staged retroactively. Format, cwd, and label are not resume options.
Concurrent resumes or unsettled running state fail closed. --timeout is rejected.
Inspect partial work and external effects before explicit resume. See help recovery
for guarded settlement and legacy migration, and help remote for retained routes.

EXAMPLES
  pira_team resume RUN_ID
  pira_team resume @parser --task 'Fix the reported defect' --completion-gate 'Regression passes' --inject-implement

EXIT STATUS
  0  Report delivered, including needs_decision/incomplete. Inspect status/handoff.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# steer
pira_team steer — replace an active task and completion gate

USAGE
  pira_team steer RUN_ID --task TASK --completion-gate TEXT [--store DIR]
  pira_team steer RUN_ID --task-file FILE --completion-gate TEXT [--store DIR]

OPTIONS
  --task TEXT            Also --task=TEXT or second positional task.
  --task-file FILE       Mutually exclusive with task text.
  --completion-gate TEXT Required nonempty gate; also --completion-gate=TEXT.
  --store DIR            Original Team store. RUN_ID may be @LABEL.
  --                    End options; remaining arguments are positional.

Steer accepts no injection flags and is unavailable during startup/format repair.
It requires the same represented execution policy as the active endpoint. To
change guidance or policy, interrupt, await settlement, inspect, then resume.
Control acceptance is not completion. Inspect receipts before any retry.
For remote request-ID controls, see help remote and help manual.

EXAMPLES
  pira_team steer @parser --task 'Focus on malformed input' --completion-gate 'Report verified parsing failures'

EXIT STATUS
  0  Control receipt delivered. Inspect acceptance and settlement separately.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# interrupt
pira_team interrupt — stop a target or settle its subtree

USAGE
  pira_team interrupt [-r|--recursive] RUN_ID [--store DIR]

OPTIONS
  -r, --recursive  Close subtree admission and request guarded settlement.
  --store DIR      Original Team store. RUN_ID may be @LABEL.
  --               End options; exactly one run reference follows.

Default interrupt is target-only and reports active_children_left_running.
Recursive interrupt confirms owner exit or reports explicit failures. Root
recursive control grants no descendant content access. Current verified identity
is required even after execution policy changes. A missing PID alone never
proves exit. No persisted PID/group is signalled for termination.
Await the original run/resume invocation, then inspect partial work and external
side effects before explicit resume. See help recovery for abandoned attempts.
Control success can deliver a receipt with unresolved settlement: inspect it.
For remote uncertain delivery, see help remote and help manual.

EXAMPLES
  pira_team interrupt -r RUN_ID

EXIT STATUS
  0  Control receipt delivered. Inspect settlement and any reported failures.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# tier
pira_team tier — publish a live local service tier

USAGE
  pira_team tier RUN_ID standard|fast [--store DIR]

OPTIONS
  --store DIR  Original Team store. RUN_ID may be @LABEL.

Requires a new-feature active local owner, including repair. It publishes settings
for subsequent captures in the current turn. Captured steps and frozen consumers
stay unchanged. It never interrupts, replays, restarts, injects input, or creates a
turn. Receipts distinguish desired publication from actual tier (unknown).
Only an applied acknowledgement updates retained intent. Rejected/unconfirmed
publication leaves future intent unchanged. Failed persistence reports unknown
durability. Inspect receipts and never blindly retry unknown delivery.
An inactive run or unsupported owner is an error. Remote live tier is unsupported;
remote run/resume --service-tier remains supported.

EXAMPLES
  pira_team tier @parser fast

EXIT STATUS
  0  Tier acknowledged as applied. Actual tier/billed speed remains unknown.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# read
pira_team read — retrieve managed handoff or diagnostic content

USAGE
  pira_team read RUN_ID [RELATIVE_FILE] [--store DIR]

OPTIONS
  --store DIR  Original Team store. RUN_ID may be @LABEL.

Without a filename, requires completed, needs_decision, or incomplete and reads
the handoff. manifest.json exposes usage, revisions, repairs, gate, and artifact/
log references. A coordinator manifest includes descendant summaries, not content.
Safe relative managed files are allowed; backend credentials/state are excluded.
Verified session ownership is required. Local reads launch no worker and require
no backend authentication. Normal remote reads use the retained route and return
raw UTF-8 file content up to 2 MiB; see help remote for compatibility views.

EXAMPLES
  pira_team read @parser
  pira_team read RUN_ID manifest.json
  pira_team read RUN_ID revisions/000001/manifest.json

EXIT STATUS
  0  Managed content retrieved.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# path
pira_team path — locate a local managed artifact

USAGE
  pira_team path RUN_ID [RELATIVE_FILE] [--store DIR]

OPTIONS
  --store DIR  Original Team store. RUN_ID may be @LABEL.

Default is the handoff path, requiring completed, needs_decision, or incomplete.
Use . for the run root. Safe relative diagnostic/artifact files are allowed;
backend credentials/state are excluded. Verified session ownership is required.
This command launches no worker and requires no backend authentication.
Remote paths fail explicitly: retrieve remote content with read instead.

EXAMPLES
  pira_team path RUN_ID
  pira_team path @parser .

EXIT STATUS
  0  Local managed path returned.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# wait
pira_team wait — await retained settlement without a model turn

USAGE
  pira_team wait RUN_ID [--store DIR]

OPTIONS
  --store DIR  Original Team store. RUN_ID may be @LABEL.

Waits while retained status is running, then prints a JSON receipt. Inspect its
status for the outcome. Stopping the waiting client does not interrupt the worker.
Local wait requires verified session ownership; remote wait uses the retained
route. For uncertain delivery, see help recovery.

EXAMPLES
  pira_team wait @parser

EXIT STATUS
  0  Retained status settled. Inspect the receipt for the worker outcome.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# bookmark
pira_team bookmark — assign or move a session-scoped label

USAGE
  pira_team bookmark RUN_REF --label LABEL [--store DIR]

OPTIONS
  --label LABEL Required exact, case-sensitive label, 1..128 UTF-8 bytes.
  --store DIR   Team store.
  --            End options; exactly one run reference follows.

RUN_REF is a literal run ID or @LABEL. A label has no edge whitespace, controls,
/ or backslash, leading @, or . / .. names. Assignment atomically adds/moves it;
the JSON receipt reports previous/current references and changed. Same-target
assignment is a no-write no-op. Metadata never enters worker input. See help
sessions for ownership, visibility, failure, and retention rules.

EXAMPLES
  pira_team bookmark RUN_ID --label parser

EXIT STATUS
  0  Mapping assigned, or already points to the target.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# unbookmark
pira_team unbookmark — remove only a label mapping

USAGE
  pira_team unbookmark LABEL [--store DIR]

OPTIONS
  --store DIR  Team store.
  --           End options; exactly one label follows.

Use the exact label without @. Absence is a no-op. Runs and artifacts remain.
Labels are main-session-scoped. See help bookmark for label syntax and help
sessions for ownership and visibility rules.

EXAMPLES
  pira_team unbookmark parser

EXIT STATUS
  0  Mapping removed, or already absent.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# bookmarks
pira_team bookmarks — list this main session's labels

USAGE
  pira_team bookmarks [--limit N] [--store DIR]

OPTIONS
  --limit N    Integer 1..1000 (default 20).
  --store DIR  Team store.

Returns JSON matches (label, run_id, run_root) and has_more, sorted by exact label
ascending. Inaccessible pointers are hidden. Malformed/unreadable metadata fails
visibly. Runs/logs are not searched. See help sessions for ownership and stores.

EXAMPLES
  pira_team bookmarks --limit 10

EXIT STATUS
  0  Visible labels listed.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# search
pira_team search — search this main session's labels

USAGE
  pira_team search QUERY [--limit N] [--store DIR]

OPTIONS
  --limit N    Integer 1..1000 (default 20).
  --store DIR  Team store.
  --           End options; exactly one query follows.

QUERY is a nonblank case-insensitive literal keyword, at most 128 UTF-8 bytes,
without control characters. Only labels are searched. JSON matches and has_more
follow the same ordering and visibility rules as bookmarks. See help sessions.

EXAMPLES
  pira_team search parser --limit 10

EXIT STATUS
  0  Search succeeded, including zero matches.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# config
pira_team config — manage store-wide worker defaults

USAGE
  pira_team config show [--store DIR]
  pira_team config set --main MODEL --model WORKER --effort EFFORT [--store DIR]
  pira_team config reset --main MODEL [--store DIR]

OPTIONS
  --main MODEL    Exact caller model to override/reset; required for set/reset.
  --model WORKER  Worker model; required for set.
  --effort EFFORT Worker effort; required for set.
  --store DIR     Team store (see help sessions).

Show prints JSON with bundled policy, overrides, and path. Set requires both model
and effort. Reset removes only the selected caller override. Effort accepts
none|minimal|low|medium|high|xhigh|max|ultra. Options take separate values and each
may appear once. Uses worker_defaults.json, needs no Codex session or backend
authentication, and launches no worker. Invalid config fails visibly
without overwriting it; fix/restore the reported file before retrying.

For new runs, gpt-6-astra maps to gpt-6.1-sol/high. Recognized gpt-6.1-sol,
gpt-6-sol, gpt-6-luna, and gpt-5.6-sol inherit caller model/effort. Unknown or missing
caller model identity falls back to gpt-6.1-sol/high. Exact config overrides take
precedence over bundled defaults. Explicit run --model and --effort independently
win over their fields. Missing inherited effort requires --effort. Caller execution
verification remains required. Resume retains stored fields unless overridden.

EXAMPLES
  pira_team config show
  pira_team config set --main gpt-6-astra --model gpt-6.1-sol --effort high
  pira_team config reset --main gpt-6-astra

EXIT STATUS
  0  Config shown, set, or reset.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# resolve
pira_team resolve — recover an ordinary handle from a remote request ID

USAGE
  pira_team resolve REQUEST_ID [--store DIR]

OPTIONS
  --store DIR  Local Team store where the compatibility launch registered its route.

Requires the original verified root main session. Returns the retained ordinary
run handle without contacting the server or starting work. Its status is unknown
until read/wait observes the server. Use this if a compatibility launch lost its
stdout/run_id. See help remote and help manual for explicit request-ID transport.

EXAMPLES
  pira_team resolve REQUEST_ID
  pira_team read RUN_ID manifest.json

EXIT STATUS
  0  Retained handle returned. Server status remains unknown until observation.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.

# safety
pira_team — execution, guidance, and subteam safety

Execution inherits the matching caller session's latest verified context, including
on resume. Only approval-never is supported. Missing, malformed, ambiguous, or
unrepresentable permissions fail closed. Read-only callers cannot supply required
managed writes. Workspace-write callers must keep stores/build roots within their
writable scope. Full access does not expand task ownership. Fine-grained restricted
permission profiles are unsupported. No rollback/merge is automatic.

Workers always receive main Ctx/Nav/Dec guidance. Optional review, implementation,
figure, and writing guidance are independent; inject only what the task requires.
Flags grant no edit authority. Review protects source, tests, configuration, docs,
lockfiles, and expected outputs. Build outputs, authorized caches, scratch, and
managed handoffs remain permitted. Implementation authorizes only owned files.
No cache deletion is authorized. See help manual for build-root/environment rules.

Root-only --allow-subteam permits main -> coordinator -> ordinary children.
Children cannot delegate or select remote hosts. Native subagents are disabled.
Coordinator completion is rejected while children remain active. Recursive control
grants no descendant content access. These are Team-interface/instructional
boundaries, not OS isolation against same-user tampering. See help sessions for
ownership, help recovery for settlement, and help manual for full safety details.

# recovery
pira_team — interruption and deliberate recovery

Run/resume block without an execution deadline. Await the original invocation,
rather than polling logs. To stop a live worker, use interrupt. To settle a subtree
or abandoned attempt, use interrupt -r RUN_ID. Controls acknowledge acceptance;
inspect receipts and await settlement before resume. Never blindly retry unknown
delivery. Interruption can leave partial edits and external effects.

Concurrent resumes and unsettled running state fail closed. Recovery requires an
exclusive owner lock, current-attempt identity evidence, verified launcher/backend
exit, and an empty owned process group or Windows job. Missing/stale evidence or
a missing PID alone is insufficient. No persisted PID/group is signalled for
termination. Recovered abandoned attempts are interrupted with incomplete usage,
never completed. Inspect diagnostics, accounting, and external effects before
explicit resume. Legacy ephemeral runs cannot resume. Retained legacy runs need
an explicit completion gate on first migration; their history is preserved.

Steer is unavailable during startup/repair and requires the active endpoint's
represented execution policy. For new guidance or changed policy, interrupt,
await the original invocation, inspect, then resume explicitly.

After remote network loss, use ordinary read RUN_ID manifest.json or wait RUN_ID
in the original local store/session. The server continues independently. Accepted
work is observed without replay. An absent pending resume is submitted only by
an explicit matching resume, never by read/wait. For request/operation IDs, signed
intent, uncertain controls, and policy-tightened observation, read help manual.

EXAMPLES
  pira_team interrupt -r RUN_ID
  pira_team read RUN_ID manifest.json
  pira_team resume RUN_ID

# sessions
pira_team — run ownership, labels, and stores

All run and bookmark operations require verified main caller identity. A run belongs
to its creating main session; workspace identity or a fresh unrelated session does
not claim it. Resume preserves the creator. Config is store-wide and session-free.
Same labels in different sessions are independent. Legacy global labels are hidden.
Ambiguous historical runs remain intact but inaccessible. These checks provide
Team-interface isolation, not OS/filesystem/process security.

STORE SELECTION
  --store DIR > PIRA_TEAM_DIR > persistent PIRA parent/team.
  macOS: HOME/Library/Application Support/PIRA.
  Linux: absolute nonempty XDG_DATA_HOME/pira, else HOME/.local/share/pira.
  Windows: LOCALAPPDATA/PIRA.
Ctx/Dec use ctx and decision under that parent; their environment overrides remain
supported. Use the same Team store on followups. Older temporary runs require
--store OLD_STORE or PIRA_TEAM_DIR. Retained absolute paths prevent simple moving/
merging of run directories. No automatic store migration occurs.

Use @LABEL for resume/steer/interrupt/tier/read/path/wait/bookmark references.
Aliases resolve once; literal run IDs never become labels. Missing labels fail
closed. Inaccessible pointers are hidden from listings and receipts. Labels never
enter worker input. A run --label is attached after the initial running manifest;
later failure retains it. Pre-run validation never moves it. Bookmark write failure
stops launch. Metadata has a 1 MiB bound with no pruning and a 5-second edit-lock
budget. Invalid metadata fails visibly. See help manual for durability details.

# remote
pira_team — opt-in remote execution and retained routes

USAGE
  pira_team run --host HOST --remote-user USER --remote-endpoint-id PUBLIC_NONCE \
    --remote-exe ABS --remote-backend ABS --remote-store ABS --remote-cwd ABS \
    --authority-key LOCAL_ABS --task TASK --completion-gate TEXT [OPTIONS]
  pira_team remote-keygen --authority-key LOCAL_ABS
  pira_team resolve REQUEST_ID [--store LOCAL_TEAM_STORE]

OPTIONS
  --host HOST              SSH alias/hostname, not shell syntax. Root main only.
  --remote-user USER       Explicit SSH account.
  --remote-endpoint-id PUBLIC_NONCE
                           Independently provisioned 64-hex public endpoint ID.
  --remote-exe ABS         Actual canonical Linux candidate binary, not a wrapper.
  --remote-backend ABS     Existing canonical server-side Codex executable.
  --remote-store ABS       Canonical absolute server store.
  --remote-cwd ABS         Canonical absolute server workspace; only for run.
  --authority-key LOCAL_ABS Local private authority key; never sent to the server.
  --remote-state LOCAL_ABS Optional private controller location for normal launch.
  --store LOCAL_TEAM_STORE Local route/bookmark store for normal launch/followups.
  --label LABEL            Session label for normal launch.
  --detach                 Run/resume return durable acceptance before completion.
  --allow-subteam          Root grant for a host-local coordinator tree.
  --service-tier standard|fast
                           New runs default standard; resumes retain selection.

Provision trust separately before submission. OpenSSH ssh/ssh-keygen with signing
support and strict host-key checks are required. remote-keygen requires verified
root identity and full-access/approval-never. It generates a local key and prints
the PUBLIC allowed-signers enrollment line. Protect the private key locally. See
help manual for fixed trust files, endpoint descriptor, permissions, and enrollment.
Run/resume/steer require fresh full-access/approval-never. Remote paths grant no
local scope. Server paths exclude . and .. components. Server authentication remains server-side. All children stay on the
selected host; workers cannot make remote hops.

Normal launch durably retains an ordinary run_id before contacting the server.
Follow with ordinary read/resume/steer/interrupt/wait RUN_ID (or @LABEL), in the
same local store/session, without endpoint/key/controller options. Remote read
returns raw content up to 2 MiB; receipt paths are null. Remote path, live tier,
and schema files are unsupported. --task-file is local input sent as task text.
Use --remote-cwd instead of --cwd. --service-tier on launch/resume stays supported.
After network loss use read/wait: observation never implicitly creates a model
turn. Inspect unknown delivery before explicit resume. See help recovery.

LEGACY REQUEST-ID TRANSPORT
Explicit --request-id selects compatibility mode. Every invocation supplies all
endpoint options plus --remote-state LOCAL_ABS --authority-key LOCAL_ABS and
--request-id KEY. Local --cwd/--store/--schema/--label are rejected in this mode.
  run:          add --remote-cwd ABS --task TASK --completion-gate TEXT.
  resume:       NEW request ID plus --run-request ORIGINAL_REQUEST.
  attach:       observe/wait without resuming or injecting a turn.
  read:         structured manifest; --artifact | --file RELATIVE_FILE |
                --operation-id KEY select content or a control receipt.
  steer:        --operation-id KEY --task TASK --completion-gate TEXT.
  interrupt:    --operation-id KEY [-r|--recursive].
  remote-check: availability probe; runs backend --version, not a model/auth test.
Retry the SAME request ID and payload to reconcile uncertain submission. Unknown
controls require the SAME operation ID and intent. Read --operation-id observes
without resending. Acceptance is not owner exit: inspect result.status and
completion_confirmed, including failed_or_unknown/unknown. Dropping SSH means
unknown connectivity, not success/stoppage. No exactly-once task-effects guarantee.
Use help manual for complete compatibility syntax, signed scope, and recovery.

EXAMPLES
  pira_team help manual
  pira_team read RUN_ID manifest.json
  pira_team wait RUN_ID

EXIT STATUS
  0  Result/receipt delivered. Inspect status, errors, and completion confirmation.
  1  Invalid arguments or operation failure; inspect stderr and retained diagnostics.
