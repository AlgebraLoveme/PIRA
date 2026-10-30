# pira_write_check — English lexical review markers

Native Rust tool. Availability follows the selected published release manifest;
a source checkout alone does not install it. The executable is `pira_write_check`
(`pira_write_check.exe` on Windows),
matching the package/source layout and CLI conventions of `pira_svg_check`.
It has no Python/shell runtime dependency, model calls, network, classifier or
rewriting. It reads only explicitly supplied regular UTF-8 files; symlinks to
regular files are followed. No recursion, extension filtering or stdin.

```sh
cargo build --locked --manifest-path tools/Cargo.toml -p pira_write_check
# Or use cargo run with the same package:
tools/target/debug/pira_write_check draft.md
tools/target/debug/pira_write_check --json draft.md
tools/target/debug/pira_write_check --lexicon custom.json draft.md
tools/target/debug/pira_write_check --help
tools/target/debug/pira_write_check --version
```

## Review and matching contract

Configured English marker occurrences only; review necessity in context. No
semantic verdict or clean-scan quality guarantee. Every configured occurrence
prompts review, including quotes, code, negation and necessary qualifications.
There is no context-based exemption or suppression. The inventory is English-
oriented, not a language detector. Categories organize review, not meanings.

- Case-insensitive literal words/phrases, never regex syntax. No normalization,
  stemming or inflection expansion. Phrase spaces match one or more ASCII
  spaces/tabs, never line breaks or other whitespace.
- Word boundaries exclude adjacent Unicode letters (`L`), numbers (`N`) and
  underscore. Apostrophes, hyphens and combining marks are not word characters.
  Matching uses Rust regex Unicode simple case folding, with Python-compatible
  dotted/dotless `I/i/İ/ı` equivalence. Unicode tables follow the locked regex
  dependency, not the host Python version. Unicode scalar values determine
  one-based lines/columns: not UTF-8 bytes, grapheme clusters or tab stops.
  A BOM and every combining mark count individually; an emoji counts once.
- Lines follow Python `str.splitlines()`: LF, CR, CRLF (one break), VT, FF,
  U+001C–U+001E, NEL, U+2028 and U+2029. Context preserves the complete original
  line, excluding its terminator. Raw Markdown is not parsed.
- Every occurrence is retained, including self-overlapping phrases. Strictly
  overlapping intervals group transitively; adjacent intervals stay separate.
  Each group has one neutral review prompt, its union span and individual hits.
  Order: arguments, source line, column, category, configured marker.
- `--lexicon FILE` (also `--lexicon=FILE`) selects literal JSON of shape
  `{"category": ["marker", "multi word"]}`. Categories/lists must be nonempty;
  markers must have word-character ends and single spaces between words, no
  other whitespace. Duplicates within one category are rejected under matching
  equivalence; cross-category occurrences remain distinct.

**Prototype difference:** duplicate detection now follows the matcher's simple
case equivalence instead of Python's full `str.casefold()`. Thus `ss` and `ß`
may coexist (they do not match each other); `is` and `İS` cannot coexist (they
match the same text). Python's prototype does the reverse for these duplicate
checks. Unicode versions may classify newly assigned characters differently.
Default English inventory and its tested matching behavior are unchanged.
Custom lexicons are parsed as strict Unicode JSON; lone surrogate escapes fail.

## Reports and errors

Warnings exit **0**, even with zero occurrences. File, UTF-8, lexicon and usage
errors exit **2**. Help/version exit **0**. Usage errors go to stderr; input
errors appear in the selected stdout report. An invalid lexicon stops scanning;
otherwise successful files still report alongside failed files. Paths preserve
the argument spelling. Use `--` for dash-prefixed filenames.

JSON stdout: `{"advisory": str, "warnings": [group], "errors": [error]}`.
A group has `path`, `line`, `column`, `span`, `context`, `matches`, `prompt`.
Each match has `column`, `span`, `marker`, `category` and inherits group path/line.
Spans preserve source spelling/whitespace; `marker` is the inventory entry.
An error has `path`, `reason`. Text output prints every group/hit, exact context,
a summary and the advisory; strings/control characters are escaped. JSON uses
ordinary UTF-8 strings, not Python's ASCII-only escape formatting. Native text
uses LF output line endings on every platform (Python uses host-native endings).
Error wording
is platform-dependent. Whole files are read; this is not a hardened service for
arbitrarily large or adversarial files.

## Source and integration

- `tools/src/pira_write_check/{lib.rs,main.rs,lexicon.json}`: implementation and
  separate tracked inventory, embedded at compile time with `include_str!`.
- `tools/crates/pira_write_check/`: Cargo manifest, README and native tests.
- The development prototype/oracle is excluded from this source checkout. The
  native implementation, embedded inventory and retained lexical fixtures are
  self-contained; no development oracle is required to build or test them.
- Release/build/setup integration is managed outside this crate. Adding these
  sources does not publish a release or install an executable.

```sh
cargo test --locked --manifest-path tools/Cargo.toml -p pira_write_check
cargo test --locked --release --manifest-path tools/Cargo.toml -p pira_write_check
python3 -B -m unittest discover -s tools/crates/pira_write_check/tests -p 'test_*.py'
```

Tests retain independent lexical expectations and cover general raw prose,
Unicode locations, all default entries, boundaries, whitespace, self-overlap,
custom literal inventory, output, unchanged files and input/usage failures.
Optional Python CLI parity checks use the retained development prototype when
present; clean source checkouts rely on the self-contained Rust fixtures.
Fixture agreement is lexical implementation evidence, not semantic precision
or recall. No paper-specific matching rule is implemented.
