---
# kinora-0caz
title: 'kinora repack: commit + clone-to-temp + atomic swap'
status: todo
type: feature
priority: normal
created_at: 2026-04-19T14:51:31Z
updated_at: 2026-04-19T15:29:55Z
blocked_by:
    - kinora-b1mg
---

## Why

Composes `commit` + `clone` + directory swap into a single atomic operation, so users don't have to drive three commands and manage a temp directory by hand.

Name chosen to echo `git repack`: the command's primary purpose is to *rewrite* the content store into a cleaner form, not just remove things. Intentionally named with future pack-file support in mind (see memory: planned pack-file format for content store) — when we introduce pack files, generating them becomes part of what `repack` does, which matches user expectations from git.

## CLI shape

```
kinora repack
```

Respects the global `--repo-root` / `-C` flag. No other args for now.

## Flow

1. **Commit**: run the equivalent of `kinora commit` in the target repo. If there's nothing to commit, that's fine — skip silently.
2. **Clone to temp**: `kinora clone <repo>/.kinora <repo>/.kinora.repack-tmp`. Temp sibling (same parent dir) so the swap is a rename within one filesystem.
3. **Atomic swap**:
   - Rename `.kinora` → `.kinora.repack-old`
   - Rename `.kinora.repack-tmp` → `.kinora`
   - Remove `.kinora.repack-old`
   The two renames are the critical section; on Linux they're both `renameat` calls and either succeeds or fails cleanly. If the second rename fails, we roll back the first.
4. **Output**: report kinos rebuilt, blobs dropped, filenames rewritten (passed through from clone).

## Safety

- Refuse to run if a previous `.kinora.repack-tmp` or `.kinora.repack-old` is lingering — something went wrong last time and needs manual attention rather than silent overwrite.
- Refuse to run if the commit step leaves dirty state (shouldn't happen but guard against bugs).
- No partial states visible to other processes: between the two renames there's a window where `.kinora` doesn't exist, but that's brief and any concurrent kinora command would fail fast rather than corrupt.

## Future: pack-file integration

When pack files land, `repack` will be the natural place to trigger pack generation. The clone step already walks all reachable kinos, so bundling them into packs at that point is a store-layer concern, not a new CLI command. This bean doesn't implement packs — just leaves the name and code path set up for it.

## Depends on

- `kinora clone` bean (repack is a wrapper around clone)
- `--repo-root` flag bean (transitively, via clone)
- Rename bean (hot→staged, compact→commit) — soft dependency; the commit step needs to exist by either name

## Design decisions

- **Unlink old `.kinora` immediately** after swap. Content-addressed data is losslessly preserved in the new dir, and git history is the real safety net — no intermediate backup dir needed.
- **No pre-check for no-op**: always run commit + clone + swap. If the net result is byte-identical, git will show no diff on `.kinora/`. Simpler than adding a dirty-check fast path.
- **Safety rail: refuse to run if `.kinora/` has uncommitted git changes.** Repack rewrites the directory; users should checkpoint their work in git first so they can roll back via `git checkout` if something goes wrong. (Relies on `.kinora/` being git-tracked in the user's repo — document this expectation.)

## Acceptance

- `kinora repack` on a repo with legacy files produces a repo with no legacy files
- Repo is unchanged from caller's perspective (render/resolve output identical)
- No lingering temp dirs after success
- Clear error if previous repack left state behind (`.kinora.repack-tmp` / `.kinora.repack-old` exists)
- Zero warnings, all tests pass
