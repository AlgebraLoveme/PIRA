Read-only English lexical marker review for explicit regular UTF-8 files.

USAGE
  pira_write_check [OPTIONS] FILE...
  pira_write_check -h|--help
  pira_write_check -V|--version

OPTIONS
  --json             Emit JSON on stdout (default: text report).
  --lexicon FILE     Use a custom category-to-marker-list JSON file.
  --lexicon=FILE     Equivalent form (default: embedded English lexicon).
  --                 End options; all following arguments are filenames.
  -h, --help         Show help and exit 0 when encountered before --.
  -V, --version      Show version and exit 0 when encountered before --.

  Options may follow filenames. --lexicon consumes the next argument as its
  path, even if it starts with a dash. The last lexicon selection wins.
  Help/version return immediately, before later arguments or file validation;
  earlier argument errors still fail. After --, help/version are filenames.

INPUT AND REVIEW
  Supply one or more regular UTF-8 files; symlinks to regular files are followed.
  No stdin, recursion, extension filtering, network access or rewriting.
  Custom lexicon shape: {"category": ["marker", "multi word"]}. Categories and
  lists must be nonempty; markers need word-character ends and single spaces
  between words. Matching-equivalent duplicates within a category are errors.

  Case-insensitive literal matching scans raw source, including code and quotes.
  Phrase spaces match ASCII spaces/tabs within a line. Locations use one-based
  lines and Unicode scalar columns, not bytes, grapheme clusters or tab stops.
  Context preserves the source line without its terminator. Overlapping hits
  form review groups; each occurrence is retained.
  Configured English marker occurrences only; review necessity in context. No semantic verdict or clean-scan quality guarantee.
  Keep markers needed for meaning, accuracy, safety or quotation.

EXAMPLES
  pira_write_check draft.md notes.txt
  pira_write_check --json draft.md
  pira_write_check --lexicon custom.json draft.md
  pira_write_check --lexicon=custom.json draft.md
  pira_write_check -- -draft.md

EXIT STATUS
  0  Scan completed, including scans with marker warnings or zero occurrences;
     also help/version.
  2  Usage, file, UTF-8 or lexicon error. Usage errors go to stderr; file and
     lexicon errors appear in the selected stdout report. An invalid lexicon
     stops scanning; otherwise readable inputs still report alongside errors.
