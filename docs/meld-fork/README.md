# Meld Fork — Read Me

This branch (`feat/meld-fork`) of `Teddy563/arnis` carries five
opt-in additions over upstream `louis-e/arnis` v2.7.0. They unlock
country-scale Minecraft world generation through the
[Meld](https://github.com/Teddy563/meld) scheduler.

## What's in this folder

| File | Purpose |
|---|---|
| `CLI_REFERENCE.md` | Every Meld-added flag with intent, examples, source files touched. **Start here** if you want to use the fork. |
| (this file) | Why the fork exists + how to keep it in sync with upstream. |

## Branch layout

| Branch | Tracks | Use |
|---|---|---|
| `main` | Upstream `louis-e/arnis:main` | Keep clean. `git pull upstream main` to follow upstream. Never push fork-only commits here. |
| `feat/meld-fork` | This branch | Holds the five Meld-fork patches on top of an upstream tag (currently `v2.7.0`). Push to `origin` so the Meld scheduler can pull this branch. |

## How to use

```bash
git clone https://github.com/Teddy563/arnis.git
cd arnis
git checkout feat/meld-fork
cargo build --release
./target/release/arnis --help | grep -E "master-origin|elevation-|overpass-url|road-detail"
# Expect 6 flags shown.
```

The Meld scheduler points at `target/release/arnis.exe` (renamed to
`arnis-windows.exe` and copied to the Meld repo root by
`start.bat`).

## How to refresh against upstream 2.8.0+

```bash
# 1. Pull new upstream tag.
git fetch upstream --tags

# 2. Take the new tag onto main.
git checkout main
git merge --ff-only upstream/v2.8.0
git push origin main

# 3. Rebase the fork branch.
git checkout feat/meld-fork
git rebase v2.8.0

# 4. Resolve conflicts file by file. Each Meld-fork commit
#    documents its intent in its commit message — re-apply the
#    intent rather than the line numbers if upstream moved code.
#    See `meld_arnis_fork/REFRESH.md` in the Meld repo for the
#    detailed playbook.

# 5. Verify all six flags still register.
cargo check --release
./target/release/arnis --help | grep -E "master-origin|elevation-|overpass-url|road-detail"

# 6. Push the rebased branch.
git push --force-with-lease origin feat/meld-fork

# 7. In the Meld repo, copy this fork's source back into
#    arnis-source/, rebuild, copy arnis.exe → arnis-windows.exe.
```

## Submitting upstream

Five separate PRs into `louis-e/arnis`. Order: 1 → 2 → 3 → 4 → 5.

Per-PR markdown bodies + reference patches live in:

- `meld_arnis_fork/pr-bodies/PR_0X_*.md` (Meld repo)

Each body has a suggested commit message + PR title + intent
description. Paste into the GitHub PR description field.

## Compatibility with Meld

| Meld feature | Fork PR | Hard requirement? |
|---|---|---|
| Multi-tile worlds | PR 1 (master-origin) | YES |
| Cross-tile elevation | PR 2 (elevation-lock) | YES |
| Tile-invariant buildings | PR 3 | NO (cosmetic) |
| Self-hosted Overpass mirror | PR 4 (overpass-url) | NO (advanced) |
| Road-detail toggle | PR 5 (road-detail) | NO (low-scale fix) |

Detailed table in `meld_arnis_fork/MELD_COMPAT.md` (Meld repo).

## License

This fork inherits upstream's Apache-2.0. Meld-fork additions are
released under the same terms.

## Credits

- Upstream Arnis: [@louis-e](https://github.com/louis-e) and
  contributors.
- Meld scheduler + fork patches: [@Teddy563](https://github.com/Teddy563).
