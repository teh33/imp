# Worktree-auto

`worktree-auto` is the autonomy mode intended for isolated implementation in a secondary git worktree.

The safety invariant is implemented and should not be weakened:

> A worktree-auto request must not silently fall back to editing the main checkout.

## Current status

`imp-core` contains tested low-level support for:

- detecting the repository and main worktree;
- refusing a dirty main checkout by default;
- planning a branch and sibling worktree path;
- creating a worktree;
- switching an `AgentBuilder` to worktree cwd/scope when a host supplies a plan and metadata;
- capturing status, diff-stat, and binary patch artifacts;
- keep, apply, and discard closeout helpers;
- conservative patch application to a clean main checkout;
- trace/evidence models for worktree state.

The normal `imp --autonomy worktree-auto` startup path does **not yet automatically call** those planning/creation helpers, and the CLI does not currently expose standalone worktree closeout subcommands. Until that host wiring lands, worktree-auto can fail closed with `autonomy_worktree_required` rather than creating a worktree for the user.

## Planned runtime contract

When a host wires a worktree run, it must:

1. detect the repository and main checkout;
2. reject dirty-main, branch-collision, and path-collision cases;
3. create a secondary worktree from the requested start point;
4. pass `WorktreeRunPlan` and `WorktreeRunMetadata` through `AgentBuilder::worktree_run`;
5. execute with the worktree as cwd and `WorkspaceScope::Worktree`;
6. capture diff artifacts at closeout;
7. ask the user to keep, apply, or discard the result.

No step may downgrade to current-workspace execution without an explicit mode change.

## Branch and path shape

The helper generates branch names in the form:

```text
imp/<workflow-or-run-id>/<slug>
```

The default worktree root is a sibling `.imp-worktrees` directory near the main checkout. Inputs are sanitized, and existing branches or paths are rejected rather than reused silently.

## Artifacts

For a wired run, closeout artifacts live under the run directory:

```text
.imp/runs/<run-id>/worktree/
  worktree-metadata.json
  status.txt
  diff.stat
  diff.patch
```

Metadata records the main checkout, isolated path, branch, start point, run/workflow id, artifact paths, and clean/dirty state.

## Closeout semantics

### Keep

Leaves the worktree and branch available for manual review.

### Apply

The helper:

1. requires a clean main checkout;
2. generates a binary-safe patch from the worktree;
3. runs `git apply --check --binary -` in the main checkout;
4. applies only after the check succeeds;
5. preserves the worktree if apply fails.

It does not resolve conflicts automatically.

### Discard

Removes the worktree and deletes its branch only through explicit closeout. Dirty removal requires the force path supplied by an already-confirmed host action; user-facing integrations should ask before invoking it.

## Failure behavior

| Failure | Required behavior |
|---|---|
| Not a git repository | Fail closed; suggest `local-auto` or `safe`. |
| Dirty main checkout | Fail closed by default. |
| Branch or path collision | Fail closed; preserve existing state. |
| Worktree creation failure | Report git stderr; do not run in main. |
| Agent or verification failure | Preserve the worktree and capture artifacts when possible. |
| Apply conflict | Preserve the worktree and patch. |
| Uncertain cleanup | Keep rather than destroy. |

## Related manual tools

The model-facing `git` tool supports `worktree_list`, `worktree_add`, and `worktree_remove`. Those explicit git operations are separate from automatic worktree-auto orchestration and remain subject to policy and user intent.
