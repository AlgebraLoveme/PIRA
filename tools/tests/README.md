# PIRA Tool Tests

This directory contains public functional, security, correctness, and performance
tests for PIRA tools. Tests use inert fixtures and do not execute inspected
repository source.

## `pira_ctx`

Run the test runner from the repository root:

```bash
python3 tools/tests/run_pira_ctx_tests.py
```

The runner verifies the distribution bundle, installs the current-platform
binary at `tools/bin/pira_ctx/pira_ctx[.exe]`, and uses temporary result stores
that are removed on success. Set `PIRA_CTX_BIN` to test another executable.

`pira_ctx_test_cases.md` is the human-readable test plan. Public Rust source is
under `tools/src/pira_ctx/`; universal release artifacts are under
`tools/dist/pira_ctx/`.

## `pira_nav`

Build the local debug executable and run the black-box suites from `tools/`:

```bash
cargo build -p pira_nav
python3 tests/test_pira_nav.py
python3 tests/test_pira_nav_security.py
python3 tests/benchmark_pira_nav_correctness.py \
  --pira target/debug/pira_nav --data tests/resources/pira_nav
```

`PIRA_NAV_BIN` selects another executable for the black-box tests. Real and
synthetic fixtures, provenance, hashes, and adjacent licenses are under
`tests/resources/pira_nav/`. `benchmark_pira_nav.py` measures assertion-checked
subcommands; `benchmark_pira_nav_repositories.py` measures repository-scale
maps. Public Rust source is under `tools/src/pira_nav/`.

## Distribution and setup

Run the cross-tool distribution, selection, setup, and builder tests from the
repository root:

```bash
python3 tools/tests/test_pira_tool_distribution.py
```
