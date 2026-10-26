# LEGACY_LIST

Legacy files removed from the active scheme. During setup or migration, if any of these files still exist locally, remove them after preserving any still-needed durable policy content in the proper tracked files.

- `~/agent/MEMORY.md`
- `~/agent/assets/MEMORY_INIT.md`
- `~/agent/SOUL.md`
- `~/agent/TOOLS.md`
- `~/agent/MEMORY_SYSTEM.md`
- `~/agent/modules/PAPER_FIGURE_STYLE.md`
- `~/agent/modules/PAPER_READING.md`

- `~/agent/tools/src/pira_team/code_fix.md` — replaced by `implementation.md`.
- `~/agent/tools/src/pira_team/nav_policy.md` — removed; tool instructions reside in `main.md`.

- `~/agent/tools/src/pira_team/policy.md` — renamed and rewritten as `main.md`.
- `~/agent/tools/src/pira_team/code_review.md` — renamed and broadened as `review.md`.

- `~/agent/tools/crates/pira_team/build.rs` — removed; the three worker policies are embedded directly.

- `~/agent/assets/scripts/setup_codex_audio_mode.py` — audio installation retired; use `retire_pira_audio.py` for narrowly validated, backed-up cleanup.
- `~/agent/assets/scripts/setup_codex_audio_mode.sh` — retired audio installer wrapper.
- `~/agent/assets/scripts/setup_codex_audio_mode_windows.ps1` — retired audio installer wrapper.
- `~/agent/assets/AUDIO_CUSTOMIZATION_GUIDE.md` — retired audio customization support.

Tracked default audio assets were removed from the repository. Local/custom media are not cleanup targets; do not delete `PIRA_Voice` directories or external hook/configuration paths based on this list.
