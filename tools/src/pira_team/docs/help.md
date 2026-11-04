pira_team — retained technical artifact workers

USAGE
  pira_team COMMAND [OPTIONS] [ARGUMENTS]
  pira_team help [TOPIC]
  pira_team COMMAND --help
  pira_team config SUBCOMMAND --help

COMMANDS
  run          Start an assignment with a task and completion gate.
  resume       Continue a retained conversation or replace its assignment.
  steer        Replace an active task and gate.
  interrupt    Stop a target, or settle its subtree with --recursive.
  tier         Publish standard|fast for subsequent local captures.
  read | path  Retrieve managed content or a local path.
  wait         Wait for retained settlement without a model turn.
  bookmark     Assign or move a session-scoped label.
  unbookmark   Remove a label, retaining its run and artifacts.
  bookmarks    List this session's labels.
  search       Search labels by case-insensitive literal keyword.
  config       Show, set, or reset store-wide worker defaults.
  resolve      Recover an ordinary run handle from a remote request ID.
  remote-keygen | remote-check | attach
               Remote trust setup, availability check, compatibility observation.

OPTIONS
  -h, --help   Show this overview; also accepted after a command alone.
  --help-all   Show the full retained manual (also: help all or help manual).
  --version    Print the package version.

TOPICS
  Command names above, config show|set|reset, and:
  safety       Execution permissions, ownership, guidance, and subteams.
  recovery     Interrupt, uncertain delivery, abandoned attempts, and resume.
  sessions     Run ownership, stores, labels, and legacy access.
  remote       Normal --host routing and legacy request-ID transport.
  all | manual Full reference, including remote trust and recovery details.

Run/resume wait without an execution deadline. Default output is a JSON receipt.
Inspect its status and handoff: completed, needs_decision, and incomplete differ.
Help and version need no caller session, store, backend, or authentication.
Help routing uses only the explicit forms above, including -h in place of --help.
Values and operands in other command lines keep their existing parsing.

EXAMPLES
  pira_team run --task 'Review src/parser.rs' --completion-gate 'Report actionable findings' --inject-review
  pira_team read RUN_ID
  pira_team help resume
  pira_team help remote

EXIT STATUS
  0  Command succeeded, including help/version. Inspect outcome/control receipts.
  1  Invalid arguments or an operation failed. Diagnostics go to stderr.
