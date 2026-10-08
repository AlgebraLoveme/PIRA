# Publication and workspace compatibility

Decision publication requires filesystem support and permission for atomic hard
links within the store. Failure is reported without a replacing rename fallback;
the add operation attempts to remove its staged file. Move the store to a filesystem
that supports hard links if this error occurs. Readers never intentionally observe
a partially copied destination.

Unaffected UTF-8 workspace paths retain their existing namespace. Paths containing
non-UTF-8 native data or the Unicode replacement character use `native-v1-` plus a
SHA-256 native-path identity (Unix bytes; Windows UTF-16 little-endian code units).
Workspace roots are resolved as before: nearest Git root, otherwise cwd, canonicalized
when possible.

## Explicit recovery of ambiguous legacy records

The old lossy-path hash may combine records from several distinct workspaces.
Records do not contain enough provenance to assign them automatically. Reads and
writes fail visibly if the old namespace contains `.piradec` entries, even if a
new namespace already exists. No automatic merge, migration or migration CLI is
provided. The diagnostic names both namespace locations.

1. Stop all writers, including old-version binaries, before migration. Do not run
   old versions against affected workspaces afterward: they recreate the ambiguous
   namespace. Concurrent old-version writers are not coordinated by the new lock.
2. Back up the complete old and any new namespace outside the active store.
3. Manually establish ownership of every legacy record. If ownership cannot be
   established, retain the backup and do not guess or copy records into multiple
   workspaces. Resolve attribution before proceeding.
4. Explicitly relocate attributed `.piradec` files to their intended new namespace's
   `records` directory, preserving names, contents and private permissions. Never
   overwrite a destination: compare duplicate IDs and investigate discrepancies.
   Check supersedes/related targets as part of attribution; references do not prove
   that all records originated in the same workspace.
5. Keep the backup; once no legacy `.piradec` entries remain in the old namespace,
   verify list/show in each intended workspace. Corrupt records also block automatic
   use and require operator investigation rather than silent skipping during migration.

This is a deliberate fail-visible compatibility boundary, not a guarantee that
ambiguous historical ownership can be reconstructed.

## Physical store paths

Runtime store access rejects all symlink path components, including ancestors,
components before `..`, and platform aliases such as macOS `/var` and `/tmp`.
Configure the physical store path instead; runtime does not silently canonicalize
store paths. Resolving the selected location during setup changes configuration,
not stored records, and requires no record migration or export. Checks occur
before store directory creation or permission changes. They reject stable links;
they do not guarantee resistance to concurrent pathname replacement.

A `..` component after any missing prefix is rejected before creation: later
components cannot be inspected safely until that prefix exists. Ordinary missing
nested paths without such traversal remain supported, as does `..` through
existing physical directories.

## Write-platform and privacy contract

Windows add/forget/export use native private creation and DACL validation,
owner locks, atomic no-overwrite hard-link publication and flushed file contents.
Windows does not promise crash persistence of created/deleted file or directory
entries, including new ancestors. No directory-sync equivalence is claimed.

New Windows objects have protected process-user-only DACLs at creation, avoiding
permissive inherited grants. Existing objects are never repaired: their owners
and active grants must be the current process user, SYSTEM or Administrators.
Privileged actors are outside the privacy threat model. Basic deny and inactive
inherit-only ACEs are accepted; unknown ACE forms, untrusted owners and null or
broad DACLs fail visibly. Reparse points are rejected. Native UTF-16/long paths
are preserved. Focused Windows tests are ready for private CI; no execution or
power-loss claim follows merely from their presence.

On Unix, all missing store ancestors are created with mode 0700, without chmod of existing
non-managed ancestors. Managed directories use 0700 and new files use 0600.
macOS deny-only ACLs and zero-permission allow entries are preserved. Nonzero
allow entries (including inherited/export ACLs) and unknown ACL types fail before
sensitive bytes are written. Every entry is checked: a safe first entry cannot
hide a later grant. Principal membership and deny/allow cancellation are not
approximated, so even owner-only or effectively cancelled grants can be rejected.
No ACLs or historical records are automatically stripped or repaired. Linux relies
on POSIX permission masks, which bound named ACL grants by the group-class bits.
These claims assume a filesystem that enforces those semantics and trusted paths.

On macOS/Linux, setup syncs the managed directories and their ancestor chain before staging a
record, including retries after partially failed setup. Publication still syncs
file contents before hard linking, then syncs records after publication. Failure
after publication names the published ID; inspect it before retrying. Temporary
empty directories may remain after setup failure. Export syncs contents but does
not claim crash-durable publication of its output directory entry.

Owner-held locks, native workspace identities and record encoding are unchanged.
No universal power-loss, remote-filesystem, hostile-path-race or privileged-user
protection is claimed. Existing readable stores are not retrospectively certified
private. macOS regression checks do not establish Linux or Windows runtime behavior.
