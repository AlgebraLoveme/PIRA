# Complete toolset releases

Every release contains six tools for all five supported platforms: 30 compressed
executables plus the schema-2 `pira-tools-release.json` index. Publication defaults
to a draft. Required review, native checks, provenance/byte verification and explicit
acceptance remain mandatory; existing published releases must not be replaced.
Do not dispatch during a user hold or active quota gate.

## Default: fresh builds, changed tools first

Without `retained_tools`, `build-pira-tool-bundles.yml` preserves the existing two
phases. Source-changed tools validate/build first; only after success do remaining
tools build and the single native Windows workspace gate run. Both phases use the
same per-tool workflow: package tests, setup checks and two independent builds of
all five targets. Tools are never built by overlapping matrices.

`baseline_sha` is a full ancestor source SHA for ordering only. Tool-local source,
help, manifests and tests select that tool. Lock changes limited to workspace-tool
versions select those tools. Shared, dependency, build, setup, policy or unknown
inputs, and invalid/missing/unavailable/nonancestor baselines select all six first.
An empty committed diff puts all six fresh builds in the second phase.

```sh
python3 -B -m tools.build.release_plan --base FULL_ANCESTOR_SHA
python3 -B -m unittest tools.build.test_release_plan tools.build.test_package_github_release tools.build.test_mixed_release
```

Freeze and commit reviewed source before dispatch. Uncommitted edits are not part
of a real source plan. Python 3.11+ and the existing GitHub CLI are required.

## Explicit mixed origins: build selected tools, retain pinned outputs

The optional `retained_tools` dispatch input is a JSON object. Each retained tool
must explicitly pin its published release tag, version and **actual build source**:

```json
{"pira_nav":{"tag":"pira-tools-ORIGIN","version":"VERSION","source_sha":"FULL_BUILD_SHA"}}
```

Replace placeholders with approved immutable identities. The pinned version must
match the assembly's Cargo manifest; at least one tool must build fresh. Every
non-retained tool builds in phase one by default. Optional `first_tools` is a JSON
list of distinct fresh tool names: those finish in phase one, then every other
fresh tool builds in phase two. Unknown, duplicate or retained names fail. An
empty list preserves automatic ordering. The single Windows gate tests only fresh
tool packages. Setup and packaging contract tests still run.
The release is complete, never a standalone or partial tool release.

```sh
gh workflow run build-pira-tool-bundles.yml --ref master -f retained_tools="$(cat APPROVED_PINS.json)" -f publish_release=false
```

This is **explicit output retention, not an automatic build cache**. No version,
source diff or checksum alone proves that current compiler/linker/SDK/features and
transitive inputs are equivalent. The old artifact's actual source is preserved;
current source-only changes are not represented as built. No claim that retained
inputs match the assembly is accepted or inferred. Omitting a tool from the JSON
means it builds fresh, so inspect the resulting build matrix before acceptance.

Planning authenticates the selected stable published releases through the GitHub
API, checks their tag/source, index digest, versions and exact five-target asset
inventory, and pins release/index/asset identities. Assembly rechecks those receipts,
rejects changed origins or fresh bundles for retained tools, and validates both
compressed and decompressed hashes/sizes. Retained gzip bytes are copied verbatim,
not recompressed. Source releases are read-only and remain untouched.

Retention supports complete schema-2 gzip releases. Rebuild older raw formats.
Transitive retained origins are checked against their prior authenticated indexes
and asset IDs; lineage is bounded to 16 releases. Pin the original build release to
flatten a longer chain. Missing, malformed or mismatched data fails; no fallback
silently rebuilds excluded tools or accepts unverifiable outputs.

## Backward-compatible source and provenance metadata

New schema-2 indexes explicitly set `source_sha_scope` to `assembly`. Root
`source_sha` identifies the reviewed assembly/manifest source; **it does not mean
all six binaries were built from that commit**. Every tool records
`build_source_sha`; fresh CI tools also record `build_run_id`. The source of each
binary is the per-tool build SHA, even if its current source tree has moved on.

Retained tools additionally record `retained_from` with the immediate origin's
release ID/tag, assembly SHA, index asset ID and index SHA-256. Each binary records
its authenticated `origin_asset_id`; its existing digest/size fields are unchanged.
The origin index binds any earlier provenance chain. Legacy schema-2 indexes
without `source_sha_scope` are single-build-source releases: root SHA is their
build source. An assembly-scope index missing a per-tool build SHA is invalid.

These fields are additive: asset names, versions, gzip format, hashes, sizes and
schema version remain compatible. Existing setup readers select/check the same
binary records, ignore extra metadata, and do not describe root SHA as a binary
build source. Release notes explicitly identify assembly semantics. Tools reading
source/provenance must use per-tool `build_source_sha`, not infer it from root SHA.

## Final acceptance and publication

Failures, cancellations and unexpected skips block assembly. Expected empty phases
are skipped explicitly. A fresh bundle still requires its checksum, all five
verified platform records and byte-identical two-build results. The packager checks
all packaged versions against the committed Cargo manifests and writes an index
only after all six tools validate. Retained origins are never rewritten.

Unit tests use synthetic releases/assets and authenticated-IO test doubles. They
do not replace scoped fix review, affected native Windows regressions, actual
reproducibility, run/source provenance or independent draft/public byte checks.
Bind exact release/asset IDs before publishing; then verify actual Git tag,
canonical anonymous public URLs, Latest, the complete index and every asset hash.
Keep prior published releases intact. Host/server refresh and rerun acceptance
must happen separately after active worker/store locks clear.
