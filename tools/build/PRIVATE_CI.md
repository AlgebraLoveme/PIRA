# Private native CI snapshots


Requires Python 3.11+ and Git locally; publishing additionally requires authenticated
`gh` and Git HTTPS access to an **existing private GitHub repository** with Actions
enabled. No repository is created and no release workflow is changed or invoked.
The repository must remain private throughout validation; visibility checks cannot
prevent an administrator from changing visibility concurrently or later.

Run offline regression tests (temporary fixtures and a local bare remote; no model
calls, network, installation, or production binary required):

```bash
python3 -B -m unittest discover -s tools/build -p test_private_ci.py -v
```

After source workers finish, stage the current working tree (including uncommitted
and eligible untracked source changes), then inspect the reported stage directory:

```bash
python3 -B tools/build/private_ci.py stage
# Optional CI subset (repeat --test-tool); default tests all five tools:
python3 -B tools/build/private_ci.py stage --test-tool pira_ctx --test-tool pira_team
```

Staging is local-only and prints JSON including `stage`, `original_head`, dirty
state/status fingerprint, counts, and tested tools. `--root PATH` selects another
source checkout. All five workspace members are staged even with a test subset,
because Cargo workspace resolution and setup tests need shared inputs. The independent
Linux/Windows setup job always runs, including when Team is excluded from the crate subset.

Only crate manifests, Cargo workspace/lockfile, Rust source, Rust/Python/SVG tests,
explicit Team runtime policies, policy-mirror test inputs, the SVG font configuration,
and setup regression dependencies are copied. Hidden/cache/store/auth/profile/debug/
generated directories, live smoke tests and benchmark artifacts are excluded; there
is no recursive copy of `tools/`. The allowlist lives in `selected()`; maintain it
when production/test dependencies change. Literal Rust `include!`, `include_str!`
and `include_bytes!` dependencies must be selected; dynamic includes and arbitrary
runtime file dependencies are not inferred. Source bytes are copied unchanged.

`SOURCE_SHA256SUMS` hashes every source and generated metadata/workflow file except
itself. Staging rereads the source inventory, bytes, HEAD and dirty status to reject
ordinary concurrent changes. This is not a hostile-filesystem sandbox or signed
attestation. Freeze source edits while staging. Review the stage before publishing;
hashes detect corruption, not deliberate edits accompanied by regenerated hashes.

For the actual smoke, explicitly enter the reviewed stage path, private `OWNER/REPO`,
and an unused branch. This shell block has no personal/repository defaults:

```bash
printf 'Reviewed stage path: '; read -r STAGE
printf 'Private OWNER/REPO: '; read -r PRIVATE_REPO
printf 'New validation branch: '; read -r NEW_BRANCH
python3 -B tools/build/private_ci.py verify --stage "$STAGE" &&
python3 -B tools/build/private_ci.py publish --stage "$STAGE" --repo "$PRIVATE_REPO" --branch "$NEW_BRANCH"
# Read-only follow-up:
gh run list --repo "$PRIVATE_REPO" --branch "$NEW_BRANCH"
```

Publishing rejects malformed/duplicate/traversing manifest entries, symlinks,
extraneous files, hashes that differ, nonprivate/mismatched destinations, and
existing branches. It verifies visibility again just before pushing. Git runs in a
separate disposable repository with hooks disabled and inherited Git repository/index
overrides removed; neither the source checkout nor reviewed stage is initialized,
indexed, committed or changed. An empty expected-value push lease prevents a racing
publisher from replacing an existing branch. No tags, releases or default-branch
updates are performed. The helper never force-replaces an existing branch.

If a push fails, its diagnostic includes the intended commit and exact read-only
`git ls-remote` recovery command. If the remote contains that commit, inspect Actions
instead of republishing. If absent, retry the unchanged stage with the same command.
If another commit exists, choose a new branch; do not delete/reset the existing one.
The stage remains reusable even after partial publication. Other pre-push failures
leave the stage unchanged too; fix authentication/visibility/input errors and retry.
Temporary publishing checkouts are automatically removed; staged snapshots remain
for review until the maintainer explicitly removes them.

The staged push workflow reuses the exact checkout pin from the source release
workflow, disables credential persistence, checks hashes, and runs locked crate
tests on native Linux and Windows. A separate Linux/Windows setup job runs all explicitly
staged setup tests: unified setup, tool/backend preflight, and store configuration.
That job first builds source Ctx with locked Cargo into `tools/target` (45-minute
job budget), then injects the absolute host `pira_ctx`/`pira_ctx.exe` path as
`PIRA_TEST_CTX_BINARY` into the isolated test environment. The exact native event
merge/post-use rerun regression must report success: missing executable, absent
test, skip, expected failure or actual failure fails the job. This gate applies
on both platforms even when the selected Rust test matrix excludes Ctx.
Its home/temp environment is disposable and stripped of runner authentication values;
unmocked Windows registry access is blocked, so tests must supply explicit registry
fixtures. File/path operations use the host platform, not an emulated filesystem.
Team's matrix jobs run fake-backend contracts on Linux only. SVG's Linux job
uses `apt-get download` plus `dpkg-deb --extract` into runner-local temporary storage
for a DejaVu-only font inventory, without sudo or host-font modification. These CI
jobs need their normal network dependencies; local helper tests do not. Actual
private Actions execution is a separate maintainer-approved smoke, not part of the
offline test suite.

The setup snapshot explicitly selects `setup_pira.py`, `setup_pira_tools.py`,
`setup_pira_stores.py`, `migrate_pira_stores.py`, their four test modules,
`assets/LEGACY_LIST.md`, and the platform selector, alongside existing Team
fixture/policy dependencies. The setup job discovers `test_*.py` only from this
reviewed snapshot. Maintain these exact inputs as dependencies change; never replace
them with recursive script-directory copying. Migration verification precedes
configuration publication in both entry points, including unified setup's
`--skip-tools` route. Retained Team histories remain subject to the migration
owner's fail-closed relocation checks; passing installer tests alone does not
validate those histories.

## Pinned native relocation job

The independent `native_relocation` job targets x64 Linux and Windows and provisions
only official Codex **0.161.0** under `RUNNER_TEMP/pira-relocation-codex`. The helper
constructs the `openai/codex/releases/download/rust-v0.161.0` URLs and verifies these
user-approved SHA-256 pins before extracting or executing any binary:

- `codex-x86_64-unknown-linux-musl.tar.gz`: `b1efb95097660d7f2e5a3887618a23f2ea1b0d548078bf92b0f7a5d229a0cef2`
- `codex-x86_64-pc-windows-msvc.exe.zip`: `a7493348634867c905f7298211923c57eb01dfe45400207a423c5453b190b11a`

Linux extraction accepts exactly the asset's named executable member. Windows
extraction accepts the exact 53-entry official 0.161.0 package inventory (47 files,
6 directories), validates its package manifest and preserves all sibling executables,
`codex-path` and `codex-resources` including voice DLLs/licenses. The manifest's
`codex-x86_64-pc-windows-msvc.exe` entrypoint is retained without renaming and passed
to the native fixture; resources/path directory relationships stay intact. No
archive paths outside this literal inventory, duplicate entries, links, encrypted
members or nonregular files are accepted. Per-file and total expanded sizes are
bounded; extraction uses a fresh empty directory and exclusive file creation. The
download is bounded too, an existing destination is never refreshed, and `--version` must return
`codex-cli 0.161.0` in an isolated credential-free home before publication. No PATH,
registry, login, privilege or developer-mode changes are made.

Staging now requires the maintained `team_store_relocation.py`,
`test_team_store_relocation.py`, and `team_relocation_fixture.py` inputs explicitly.
Missing inputs fail staging; managed artifacts are not copied into snapshots.
The required job runs the maintained deterministic suite (`test_team_store_relocation.py -v`)
and `test_team_store_relocation.py --native --completed-turns --scratch ABSOLUTE_PARENT
--binary ABSOLUTE_CODEX`. Both run even if the deterministic suite fails; either
failure fails the job. Each command has an eight-minute timeout. The fresh task-local
HOME/CODEX_HOME and allowlisted environment exclude runner authentication; inference
uses only the fixture's scripted localhost Responses server. No registry access,
login, real user stores, privilege changes or paid calls are requested.

The fixture checks three threads with two completed turns each, full paginated
history, alias-removed resume, original hashes, destination writes and rollback.
It also exercises the public setup transaction into a nonempty destination, one
real continuation turn, receipt reruns without initial repair, read-only modes and
the configuration-publication barrier. Windows uses directory junctions and
explicitly forbids symlink creation in the fixture. Missing required alias or
listener support fails visibly, never as a skip.

The fixture uses production admission on validated platforms. Candidate-only
admission is a reversible test override for freshly generated fixtures, reported
separately. A passing job does not change the production platform/format allowlist
or migrate user data; `verified_fixture_only` describes this test boundary.

Synthetic command output, result.json, repair/failure journals and native.stderr
are retained under `RUNNER_TEMP/pira-relocation-evidence` and replayed in private
Actions logs (JSON-escaped lines), including on failure. No home/auth files or
rollout records are uploaded. Runner-local files expire with the runner; the job
logs preserve these diagnostics. Forced job cancellation can prevent final replay.
Local offline tests check invocation, isolation, staging and failure propagation;
they do not claim a native Linux/Windows completed-turn pass.
