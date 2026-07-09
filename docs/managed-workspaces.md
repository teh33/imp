# Managed agent workspaces

Managed workspaces are imp-owned Git worktrees for isolating write-capable agent runs. The host runtime owns their registered lifecycle. The structured Git tool exposes only read-only worktree listing; Full-mode shell remains an explicit escape hatch and can create unmanaged worktrees, which doctor reports but never adopts or deletes.

This release provides the deterministic lifecycle service and CLI. Automatic agent launch and scheduling are not enabled yet. A coordinator can consume the same API once run ownership and task dispatch are connected.

## Safety model

Each managed workspace records:

- a typed workspace id and owning run id;
- repository and main-worktree identity;
- managed path and internal branch;
- target branch, base reference, and exact base commit;
- changed paths and clean/dirty state;
- an immutable candidate commit after readiness;
- integration and cleanup state;
- diagnostics for failed or interrupted operations.

The registry lives under `IMP_HOME/workspaces` or `~/.imp/workspaces`. Registry updates use an ownership lock and atomic replacement.

Imp creates at most four live managed workspaces per repository by default. Unknown Git worktrees are reported but never adopted or deleted.

## Lifecycle

```text
provisioning -> active -> ready_to_integrate -> integrating -> integrated
                       \                         -> cleanup_pending
                        -> retained/orphaned
                        -> cleaned (explicit discard)
```

`ready_to_integrate` requires:

- a clean managed worktree;
- at least one commit beyond the recorded base;
- a pinned candidate commit.

Integration requires:

- the main worktree on the recorded target branch;
- a clean main worktree;
- the managed branch still at the pinned candidate commit;
- a fast-forward from the target branch to that commit.

Imp does not rebase, merge divergent histories, resolve conflicts, or interpret semantic compatibility. A failed precondition leaves the workspace recoverable.

After a successful fast-forward, Imp removes the clean managed worktree and deletes the internal branch only if it still points at the integrated commit. Unexpected late changes produce `cleanup_pending` instead of data loss.

## CLI

```bash
imp workspace create --id eval-agent --run-id run-123 --task "compare against pi"
imp workspace list
imp workspace inspect eval-agent
imp workspace doctor
```

After the agent commits and verification passes:

```bash
imp workspace ready eval-agent
imp workspace integrate eval-agent
```

Discard is explicit and destructive:

```bash
imp workspace discard eval-agent --yes
```

Every command supports `--json` for host/TUI integration.

## Doctor findings

`imp workspace doctor` reconciles the registry with `git worktree list --porcelain` and reports:

- registered worktrees missing from Git;
- branch ownership mismatches;
- unmanaged Git worktrees;
- changed paths shared by multiple live managed workspaces.

Missing registered worktrees are marked orphaned. Doctor does not remove, adopt, or mutate unknown worktrees.

## Current boundary

Managed workspaces solve ownership, isolation, inventory, overlap visibility, deterministic integration, and conservative cleanup. They do not yet decide which agent should receive a workspace or automatically launch agents inside one. That scheduling layer must use stable run ids, this host-owned lifecycle, and an execution sandbox or command policy when it must prevent direct Git metadata mutation through arbitrary shell commands.
