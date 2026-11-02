# Navigator routing and provider effects

`map`, `outline`, `symbols` and structural item/selector `show` use bundled
native parsers by default. Only an explicit existing `--lsp` selection requests
a server inventory; a dirty native parse never automatically discovers or starts
one. `--native` remains a compatible native-only selection.

Map's native landmarks are optional: unavailable summaries retain bounded file
and topology rows, marked `landmarks_unavailable=1`, with errors and `complete=0`.
Other native structural operations report parser failures rather than inventing
an inventory or requiring an implicit server. Plain file/range/window reads and
lexical search remain parser/server-free (search `--owners` may add native-only
owner annotations).

Identity-sensitive semantic commands and dependency resolution still use actual
language servers, explicitly selected or discovered on PATH. Dirty named
semantic targets may use that server's inventory. Use matching explicit server
configuration when consuming server-generated structural selectors.

Semantic and explicit-inventory providers run normally with original source
roots, CWD and file URIs. Navigator rejects workspace edits and retains its
source/root/identity checks, bounded protocol and catchable cancellation with
owned-process cleanup. External servers/setup/build helpers may execute project
code and refresh ordinary caches. Navigator does not provide OS confinement,
strict cache immutability, project snapshots, source views or special tokens.
