# Role registry

Roles are practical, policy-narrowing profiles for agent and workflow work. They influence instructions, tool exposure, autonomy expectations, evidence, verification, model routing, and child-work eligibility. A role never widens global/project/run policy.

Implementation: `crates/imp-core/src/roles.rs`.

## Built-in roles

| Role | Intended use | Default write posture |
|---|---|---|
| `planner` | Decompose work, acceptance criteria, risks, and verification. | Read-only. |
| `coder` | Make focused changes and run narrow verification. | Write-capable within policy. |
| `verifier` | Run relevant checks and report pass/fail/blocked evidence. | Read-only files; command-capable. |
| `reviewer` | Review correctness, safety, maintainability, and product fit. | Read-only. |
| `researcher` | Gather repository/external evidence with trust labels. | Read-only. |
| `integrator` | Synthesize child outputs and resolve integration concerns. | Write-capable within policy. |

Compatibility aliases:

- `worker` resolves to `coder`;
- `explorer` resolves to `researcher`;
- `reviewer` is already canonical.

## Selection

```sh
imp --role planner "break this into verifiable steps"
imp --role coder "implement the parser fix"
imp --role verifier "run the required checks"
imp --role reviewer "review the current diff"
imp --role researcher "find the relevant API behavior"
imp --role integrator "combine the child results"
```

An explicit `--model` override wins over role routing. Tool exposure is intersected with outer policy.

## Configuration model

User/project config can override built-ins or add lowercase kebab-case role ids:

```toml
[roles.coder]
instructions = "Make focused changes and verify them."
readonly = false
tools = ["read", "scan", "edit", "write", "bash", "git", "workflow"]

[roles.coder.autonomy]
can_modify_files = true
can_run_commands = true
stop_on_verification_failure = true
max_consecutive_tool_calls = 20

[[roles.coder.required_evidence]]
kind = "diff-summary"
required = true
description = "Files changed and rationale"

[roles.coder.verification]
required = true
suggested_commands = ["cargo test"]

[roles.coder.model_routing]
thinking = "medium"
model_classes = ["code"]
capability_hints = ["code-editing", "test-debugging"]

[roles.coder.output_schema]
name = "implementation-summary"
required_sections = ["changed", "verified", "concerns"]
```

Important fields include:

- model/thinking preferences;
- `tools` or structured `tool_policy`;
- `readonly`;
- instructions and purpose;
- autonomy constraints;
- required evidence;
- verification metadata;
- model classes/capability hints;
- output-section metadata;
- child-workflow eligibility.

## Validation

Configuration fails validation for:

- invalid role names;
- missing instructions;
- unknown tools;
- write-capable edit/write tools on readonly roles;
- readonly roles that claim file-mutation autonomy;
- readonly verifier roles with `bash` but no command autonomy;
- aliases that target unknown roles.

Readonly does not mean “no process execution”: the verifier role may run approved checks through `bash`, but cannot turn failures into edits without switching roles or starting separate coder work.

## Tool policy

Role tool policy supports:

- all tools;
- only a named set;
- all except a named set.

The registry validates against imp's known tool vocabulary. Final availability also depends on actual tool registration, extension loading, agent mode, run policy, and autonomy.

## Evidence and output metadata

Roles can require evidence kinds such as plan, diff summary, test output, review findings, citations, decisions, and verification results.

Output schemas are prompt/runtime metadata. They describe expected sections but do not currently force structured model decoding.

## Child work

Workflow-generated child contracts can select roles and project their evidence/output expectations into bounded subagent input. Role selection does not itself spawn an agent. The workflow/subagent path still requires a concrete contract, write scope, resource limits, and outer policy approval.

## Model routing

When no model is pinned, the registry can filter and score model metadata using preferred/fallback models, thinking level, latency/cost preference, model classes, and capability hints. Routing is provider-neutral and best-effort; it does not bypass model availability or authentication.
