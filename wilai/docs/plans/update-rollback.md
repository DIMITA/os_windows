# Plan — `/update` rollback

**Status:** proposal, not implemented.

## What exists today

v4.0 shipped `sys.wilai.update` + `/update` slash command. The
backing script `wilai-selfupdate`:
1. `pacman -Sy` to refresh databases
2. compares installed vs candidate version
3. `pacman -S wilai` to upgrade
4. `systemctl restart wilai-daemon.service` (and hud)

There is no rollback. If a release ships a broken daemon that
crashes on boot or breaks a tool, the operator is left with:
- a running `wilai-shell` that cannot reach the daemon any more
- a `pacman -U <old-pkg>` command they have to type blind without
  the agent's help

## Three options

### A. "pacman clone" of the installed version before upgrade

Simplest. Before `pacman -S`, snapshot the currently-installed
`.pkg.tar.zst` with `pacman -Sw --cachedir <staging>` or by
pulling it from `/var/cache/pacman/pkg/` if still present. On
rollback, `pacman -U` from the staged file.

**Pros:** no new infrastructure. Works for all pacman-managed
installs. Reuses pacman's deps + hooks.

**Cons:** doesn't help on apt installs. Only recovers the
previous version, not an arbitrary one. If the previous version
is already gone from the cache and the mirrors have moved on, we
have nothing to rollback to.

### B. Snapshot `/usr/bin/wilai-*` before upgrade

The selfupdate script copies every wilai binary to
`/var/lib/wilai/rollback/<version>/` before running `pacman -S`.
On rollback, we copy back in place, `systemctl restart`.

**Pros:** package-manager-agnostic. Works for Arch, Debian, and
future paths. Minimal: one `install -D` loop.

**Cons:** doesn't rollback tool YAMLs or systemd units or the
helper scripts in `/usr/bin/wilai-*`. Needs expanding to cover
`/usr/share/wilai/` + `/usr/lib/systemd/*/wilai*`. Starts to
feel like a mini btrfs snapshot by hand.

### C. btrfs/zfs snapshot of the whole root

Take a btrfs snapshot of `@root` before every upgrade. On
rollback, boot into the snapshot via grub-btrfs or by swapping
the default subvolume.

**Pros:** the Right Thing. Rollback is atomic, covers everything
(binaries, YAMLs, units, config), works for any package manager
in theory.

**Cons:** requires a btrfs/zfs root. WilOS-Core defaults to ext4.
Adds a hard runtime dependency for one feature. Interacts badly
with the atomic-image stories (ostree-ish) if we ever add them.

### Recommendation

Option **B+** (snapshot binaries AND tool YAMLs AND units):

```
/var/lib/wilai/rollback/
  current -> 1.0.0                    # symlink
  1.0.0/
    bin/        # /usr/bin/wilai*
    tools/      # /usr/share/wilai/tools/
    systemd/    # /usr/lib/systemd/{system,user}/wilai*
    manifest.json  { version, pkg_mgr, timestamp, checksum }
  1.0.1/
    …
```

- Keep the three most recent snapshots. Prune older.
- A snapshot is maybe 500 MiB (release binaries are ~140 MiB
  each in debug, ~20 MiB stripped release × a few binaries, plus
  YAMLs). Three snapshots: ~60 MiB stripped release — fine.
- Add two tools:
  - `sys.wilai.rollback` — risk=high, confirmed. Lists snapshots,
    accepts a version, swaps files + restarts units.
  - `sys.wilai.rollback_list` — read-only.
- Shell `/rollback` slash command seeds the agent with a prompt
  that asks it to list snapshots and offer to roll back to one.

The selfupdate script learns a `snapshot` subcommand called before
the pacman/apt upgrade step. If the snapshot fails, the upgrade
aborts (no silent fallback — "no rollback possible" is worse than
"no upgrade this time").

### Flow

```
# ok path
/update
  → sys.wilai.update dry_run=true
  → "0.9 → 1.0 available"
  → user says go
  → sys.wilai.update dry_run=false
    → wilai-selfupdate snapshot
    → pacman -S wilai (or apt install new deb)
    → systemctl restart
  → shell reconnects to the new daemon
  → "upgraded to 1.0."

# rollback
/rollback
  → sys.wilai.rollback_list
  → "last three: 1.0 (now), 0.9, 0.8"
  → user picks 0.9
  → sys.wilai.rollback version=0.9
    → wilai-selfupdate restore 0.9
    → systemctl restart
  → shell reconnects to 0.9
  → "rolled back to 0.9. Previous 1.0 kept as 1.0."
```

### Edge cases to handle

- **Rollback across major versions** that changed on-disk formats
  (memory schema, audit entry layout): the plan is `pacman -S` is
  responsible for migrating up, rollback is a user escape hatch
  that may leave the DB too new for the old binary. Document loudly;
  surface a warning in the rollback tool when `memory.sqlite` has
  a newer schema than the target version expects.
- **Running daemon mid-rollback**: use the same restart dance as
  `/update`. The shell's auto-reconnect already handles the
  interruption.
- **Disk full during snapshot**: pre-flight `df` on
  `/var/lib/wilai/rollback`, require 2× the current install size
  free. On failure, refuse the upgrade with a clear nudge.

### Rollout order

1. Extend `wilai-selfupdate` with `snapshot` and `restore` subs.
2. Call `snapshot` from the upgrade path. Pre-flight free-space
   check.
3. New YAML tools `sys.wilai.rollback_list`, `sys.wilai.rollback`.
4. `/rollback` slash command in the shell.
5. Daily prune task: keep latest 3, delete the rest.

### Size estimate

Roughly 1 commit (snapshot/restore in bash + 2 YAMLs + a shell
slash). The btrfs-ish option C would be 3–4 commits plus a
documentation story about which filesystems are supported.

Lean toward B+.
