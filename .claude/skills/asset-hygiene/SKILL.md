---
name: asset-hygiene
description: Keep retail MW2 data, converted packs and leaked/private source out of the modernwiifare repo. Use before any commit, when adding test fixtures, when writing docs that quote game data, or when deciding whether an outside codebase may be referenced.
---

# Asset hygiene

The project only stays defensible if the repository holds **zero** Activision material. Users build
everything locally from their own copy.

## Before every commit
1. Run `tools/check_no_assets.sh` (or install it as `.git/hooks/pre-commit`).
2. Read the staged diff. The script catches file types, but it does not catch pasted content. Watch for:
   - GSC script bodies, weapon tuning tables, string tables or localized strings pasted into code or docs.
   - Hard-coded arrays of map geometry, spawn points, or texture/sound bytes.
   - Unit-test fixtures cut from real fastfiles. Write **synthetic** fixtures instead, built by our own writer code.
3. Images in `docs/` must be our own diagrams, never screenshots of game content.

## Numbers and names
- Read weapon and movement constants **from the user's data at build/convert time**. Do not hard-code them.
  If the runtime needs a fallback default, choose a neutral placeholder and mark it `// placeholder, overwritten by converter`.
- Asset *names* (for example `mp_terminal`) are fine in code as lookup keys. Asset *content* is not.

## Which code may be used
| Source | Allowed? | Condition |
|---|---|---|
| IW4L (Apache-2.0) | yes | keep NOTICE and attribution; list it in docs/references.md |
| mw2-recompiled (GPL-3.0) | as behaviour reference; copying code makes us GPL-3.0 | record in references; decide via ADR |
| OpenAssetTools, ZoneTool, gsc-tool | check each license first | record in references |
| Quake III Arena GPL source (pmove lineage) | yes, GPL-2.0+ | same GPL considerations |
| Leaked IW source, private decomps, dumped-script repos | **never** | — |

## Distribution
Never publish a built `.dol` that has converted data inside it, and never publish a data pack. Releases are
source-only, plus build instructions.
