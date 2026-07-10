# Autonomy modes

Autonomy controls how much otherwise-permitted work may proceed without another approval. It does not override role restrictions, per-run tool/write policy, provenance checks, or hard rails.

The canonical enum is `AutonomyMode` in `crates/imp-core/src/workflow/contract.rs`.

## Modes

| Mode | Intent |
|---|---|
| `suggest` | Read/inspect and propose mutable actions rather than executing them. |
| `safe` | Conservative compatibility default. Existing policy and approvals remain in force. |
| `local-auto` | Perform ordinary workspace edits and local checks; stop for higher-risk actions. |
| `worktree-auto` | Require an isolated worktree scope. Never downgrade silently to the main checkout. |
| `allow-all-local` | Reduce prompts for local workspace actions while preserving hard rails and audit evidence. |
| `allow-all` | Broader autonomy within configured policy; still not unrestricted machine or production access. |
| `ci` | Noninteractive, declared-scope execution that fails closed when approval is required. |

Select a mode with:

```sh
imp --autonomy safe -p "inspect this diff"
imp --autonomy local-auto -p "fix the parser and run its tests"
imp --autonomy ci --verify "cargo test -p imp-core" -p "apply the narrow fix"
```

The TUI also supports `/autonomy <mode>` for subsequent agent starts.

## Decision stack

An action proceeds only when every stricter layer permits it:

1. agent mode;
2. role tool policy;
3. per-run tool and write policy;
4. reference-monitor autonomy/resource/provenance decision;
5. hard rails;
6. tool-specific checks.

A permissive autonomy mode can reduce approvals but cannot convert another layer's deny into allow.

Reference-monitor outcomes include allow, deny, ask-user, dry-run-only, sandbox-only, and require-verification.

## Hard rails

Autonomy modes are not dangerous-action grants. Current hard-rail classifications cover:

- secret exfiltration;
- private-key reads;
- destructive writes outside the workspace;
- force pushes;
- global git configuration mutation;
- production deployment;
- cloud resource deletion;
- audit-log disabling.

These actions fail closed without a separate explicit dangerous grant. Extension code cannot self-authorize a grant.

`allow-all-local` remains project/worktree scoped. `allow-all` is broader but still respects secret, destructive, production, and audit boundaries.

## Resource scopes

Policy can distinguish:

- workspace and isolated worktree paths;
- outside-workspace paths;
- read-only versus mutating network access;
- mediated secret use versus secret reveal;
- system/global mutation;
- production state.

Accurate tool metadata matters. New tools that write files, execute processes, use the network, or access secrets must classify those effects for the reference monitor.

## Worktree-auto

The core runtime has worktree planning, creation, diff capture, and closeout helpers. The normal CLI startup path does not yet create one automatically. If the runtime lacks an explicit worktree scope, worktree-auto fails closed instead of modifying the main checkout.

See [Worktree-auto](worktree-auto.md) for the exact current wiring boundary.

## Verification and evidence

Autonomy removes prompts, not accountability. Runs can record:

- selected autonomy and workspace scope in the workflow contract;
- `policy.checked` trace events;
- approval decisions;
- verification obligations/results;
- hard-rail denial reason codes;
- project-local trace and evidence artifacts.

Code-changing autonomous runs should have a relevant verification command or workflow check. `ci` must never wait indefinitely for interactive approval.

## Maintainer requirements

- Preserve `safe` as the default.
- Keep worktree-auto fail-closed.
- Test pure decisions in reference-monitor tests.
- Test CLI/TUI parsing and display when changing mode UX.
- Keep tool policy metadata accurate.
- Do not weaken a verification gate to make a run pass.

See [Runtime policy](policy.md) and [Trust labels and provenance](trust-labels-and-provenance.md).
