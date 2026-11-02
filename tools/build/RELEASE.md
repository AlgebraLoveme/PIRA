# Full toolset release builds

`build-pira-tool-bundles.yml` has two phases. First it validates and builds the
source-changed tools. Only after that phase succeeds does it build the remaining
tools and run the single native Windows workspace gate. Both phases call the
same per-tool workflow: package tests, setup compatibility checks and two
independent builds of each of the five supported targets remain required.
Each tool builds once per workflow (with the existing two-build reproducibility
check), not in two overlapping matrices.

The optional `baseline_sha` dispatch input is a full ancestor source commit SHA
used only for ordering. Tool-local source, help, manifest and test paths select
that tool. A lockfile change selects only its tools when it changes nothing but
workspace-tool versions. Other dependency, workspace, build, setup, policy or
unknown changes select all six tools first. Missing/invalid/unavailable or
nonancestor baselines also select all six. A valid empty diff puts all six fresh
builds in phase two. The planner uses local Git data only, never fetches and never
consults installed tools. The workflow's planning checkout supplies history.

Inspect a committed-source plan locally, without running builds or networking:

```bash
python3 -B -m tools.build.release_plan --base FULL_ANCESTOR_SHA
python3 -B -m unittest tools.build.test_release_plan tools.build.test_package_github_release
```

Python 3.11+ is required, as for the existing release workflow. Local uncommitted
edits are not part of the commit diff; freeze and commit the reviewed release
source before using its plan for a real build.

## Artifact reuse boundary

Phase-one artifacts are retained and assembled directly with phase-two artifacts
from the **same workflow run and source commit**. No old release binaries, cache
hits or other runs' bundles substitute for a fresh build or test.

Cross-release reuse is deliberately disabled. Current bundle records retain a
Rust toolchain label, target/minimum-OS/linkage and binary hashes, but not a complete
identity of transitive source, locked dependencies/features, compiler/linker/SDK
bytes, build settings and validated provenance. A matching version or SHA-256
alone does not establish that identity. Adding a trustworthy cross-release cache
would be disproportionate here; uncertain/unchanged tools are freshly rebuilt.
Changed-tool selection is an ordering optimization, not proof of build equivalence.

## Complete assembly and approval

Expected empty phases are explicitly skipped; failures, cancellations and
unexpected skips block downstream assembly/publication. The packager still requires
exactly one checksum-verified bundle per tool and all five verified binaries in
each. It writes the unchanged schema-2 setup index only after all six succeed:
30 compressed executables plus the index. No phase can publish a partial toolset.
Publication still defaults to a draft and requires explicit acceptance approval;
existing published tags cannot be replaced.

Planner/assembly unit tests use synthetic metadata and assets. They do not replace
required adversarial review, final native platform checks, reproducibility,
provenance review, public HTTP/checksum verification or published-install/setup
rerun acceptance. Do not dispatch builds while a user hold or quota gate is active.
