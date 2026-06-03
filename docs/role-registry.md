# Role registry

## Goal

The role registry defines a small set of practical, role-scoped agent profiles
that workflow orchestration can select without hard-coding behavior into the Zig
or Rust core. Roles describe what kind of work an agent should do, which tools it
may use, how autonomous it may be, what evidence it must leave, how model routing
should prefer models, and what output shape downstream workflow steps can expect.

This is a configuration and workflow substrate, not a mythology layer. Core UX
uses boring role names: Planner, Coder, Verifier, Reviewer, Researcher, and
Integrator.

## Current compatibility

`crates/imp-core/src/roles.rs` currently supports a compact `RoleDef`:

- `model`
- `thinking`
- `tools`
- `readonly`
- `instructions`

Built-ins today are `worker`, `explorer`, and `reviewer`.

The new registry should preserve those names as compatibility aliases:

| Existing role | New practical role | Notes |
| --- | --- | --- |
| `worker` | `Coder` | General implementation role with write-capable tools. |
| `explorer` | `Researcher` | Read-only exploration/summarization role. |
| `reviewer` | `Reviewer` | Read-only review role with stronger reasoning. |

Aliases should resolve to the new role definitions unless overridden explicitly
by project config.

## Role model

A role registry is an ordered map keyed by stable role id. Role ids should be
lowercase kebab-case in config (`planner`, `coder`, `verifier`, `reviewer`,
`researcher`, `integrator`) and displayed in title case in UX.

Suggested data model:

```rust
pub struct RoleRegistry {
    pub roles: BTreeMap<String, RoleProfile>,
    pub aliases: BTreeMap<String, String>,
}

pub struct RoleProfile {
    pub id: String,
    pub display_name: String,
    pub purpose: String,
    pub prompt_template: String,
    pub instructions: Vec<String>,
    pub tools: RoleTools,
    pub readonly: bool,
    pub autonomy: RoleAutonomy,
    pub required_evidence: Vec<EvidenceRequirement>,
    pub verification: RoleVerification,
    pub model_routing: RoleModelRouting,
    pub output_schema: Option<RoleOutputSchema>,
    pub child_workflow: ChildWorkflowEligibility,
}
```

### Fields

- `id`: stable config key.
- `display_name`: user-facing role name.
- `purpose`: one-sentence reason this role exists.
- `prompt_template`: base prompt or template name for this role.
- `instructions`: role-specific behavioral constraints appended to the system or
  workflow prompt.
- `tools`: allowlist/denylist policy for tool exposure.
- `readonly`: hard safety flag; if true, write/destructive tools are unavailable
  even if listed by broader config.
- `autonomy`: how independently the role may proceed.
- `required_evidence`: artifacts or notes the role must produce before it can
  report done.
- `verification`: commands/checks or review gates expected for role completion.
- `model_routing`: model preference and routing hints, not provider-specific
  mandates.
- `output_schema`: optional structured output metadata for specialist outputs.
- `child_workflow`: whether this role may be delegated as a child workflow and
  under what constraints.

## Tool policy

```rust
pub enum RoleTools {
    All,
    Only(Vec<String>),
    AllExcept(Vec<String>),
}
```

Readonly roles are always restricted to read/search/analysis tools, regardless
of `RoleTools`. Initial readonly-safe tools:

- `read`
- `scan`
- `web`
- `recall`
- `git diff/status/log` style read-only git operations when represented as
  first-class tool permissions

Write-capable roles may use editing, shell, and workflow tools only when the
outer policy layer also allows them.

## Autonomy constraints

```rust
pub struct RoleAutonomy {
    pub can_modify_files: bool,
    pub can_run_commands: bool,
    pub can_create_workflows: bool,
    pub can_delegate_child_workflows: bool,
    pub requires_plan_before_write: bool,
    pub stop_on_verification_failure: bool,
    pub max_consecutive_tool_calls: Option<u32>,
}
```

Role autonomy never bypasses global/project policy. It only narrows what the
agent may do for a selected role.

## Evidence requirements

Evidence requirements describe what a role must leave behind for workflow
closeout and future eval capture:

```rust
pub struct EvidenceRequirement {
    pub kind: String,
    pub required: bool,
    pub description: String,
}
```

Common evidence kinds:

- `plan`
- `diff-summary`
- `test-output`
- `review-findings`
- `source-citations`
- `integration-summary`
- `verification-result`

## Verification requirements

```rust
pub struct RoleVerification {
    pub required: bool,
    pub suggested_commands: Vec<String>,
    pub requires_human_review: bool,
    pub accepts_manual_evidence: bool,
}
```

Role verification should feed existing workflow verification gates. Roles do not
execute verifiers by themselves; they declare expectations for the workflow
layer.

## Model routing

Model routing hints should be provider-neutral and optional:

```rust
pub struct RoleModelRouting {
    pub preferred_model: Option<String>,
    pub fallback_models: Vec<String>,
    pub thinking: Option<ThinkingLevel>,
    pub latency_preference: Option<LatencyPreference>,
    pub cost_preference: Option<CostPreference>,
    pub capability_hints: Vec<String>,
}
```

Examples of capability hints:

- `planning`
- `code-editing`
- `test-debugging`
- `review`
- `research`
- `summarization`
- `long-context`
- `structured-output`

The model registry can later map these hints to concrete providers/models.
Config may still pin `preferred_model` for predictable local behavior.

## Output schema metadata

Roles may declare output schema metadata so workflow steps can consume specialist
outputs predictably:

```rust
pub struct RoleOutputSchema {
    pub name: String,
    pub description: String,
    pub json_schema_ref: Option<String>,
    pub required_sections: Vec<String>,
    pub output_contract: Option<String>,
    pub example: Option<String>,
}
```

`required_sections`, `output_contract`, and `example` are prompt/runtime metadata.
They should guide role output shape, but the first implementation does not parse
or reject model output against them. This keeps child workflows useful without
adding brittle structured decoding too early.

Example verifier output schema metadata:

```json
{
  "name": "verification-result",
  "description": "Structured verification result role output metadata",
  "required_sections": ["status", "commands", "evidence", "failures"],
  "output_contract": "Include these sections in the final role output: status, commands, evidence, failures.",
  "example": "status: failed\ncommands: ...\nevidence: ...\nfailures: ..."
}
```

Initial implementation can store metadata only. Enforced structured decoding can
come later.

## User-facing selection

### CLI

Use `--role` to run a session with a practical role profile:

```sh
imp --role planner "break this into verifiable tasks"
imp --role coder "implement the parser fix"
imp --role verifier "run the required verification gates"
imp --role reviewer "review the current diff"
imp --role researcher "find the relevant API behavior"
imp --role integrator "combine child outputs and finish the response"
```

`--role` can be combined with `--model`; an explicit model override wins. When no
model is pinned, imp may use the role's model-routing hints to choose an
appropriate model from the model registry.

### TUI

The TUI stores the selected role on the app session and applies it when starting
agent work. The same practical role ids are used. Role selection affects prompt
instructions, tool filtering, workflow contract evidence, and model routing, but
it does not bypass policy or approvals.

### Compatibility aliases

- `worker` resolves to `coder`.
- `explorer` resolves to `researcher`.
- `reviewer` remains the practical reviewer role.

Prefer the practical names in new UX and docs.

## Configuration examples

Project/user config may override built-ins or add new roles. Keep role ids
lowercase kebab-case.

```toml
[roles.coder]
instructions = "Make focused code changes and verify them before reporting done."
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
output_contract = "Include changed files, verification, and unresolved concerns."
```

Readonly roles cannot allow write-capable tools such as `edit` or `write`, and a
role's autonomy cannot grant permissions broader than global/project policy.
Invalid tool names, invalid role ids, missing instructions, or unsafe readonly
settings fail validation before use.

## Config override behavior

Default built-ins are loaded first, then project/user config may override or add
roles.

Recommended merge rules:

1. Unknown role ids are accepted after validation.
2. Known role ids merge field-by-field with defaults.
3. `tools` replacement is explicit; tool lists are not appended implicitly.
4. `readonly = true` always narrows permissions.
5. `readonly = false` in config cannot make a built-in readonly role writeable
   unless the override also sets an explicit `allow_write_override = true` or the
   registry loader is called in a trusted config context.
6. Aliases may be added by config, but built-in aliases cannot be redirected to a
   role with broader permissions without validation warnings.
7. Invalid tool names, empty prompts, or unknown model aliases produce validation
   errors or warnings before use.

## Default roles

### Planner

Purpose: decompose work into safe, verifiable steps.

- Readonly: true
- Tools: `read`, `scan`, `web`, `recall`, read-only workflow/context tools
- Autonomy: may create/update workflow plans if workflow policy allows; should
  not edit product code.
- Required evidence: `plan`, `acceptance-criteria`, `risk-notes`
- Verification: not required to run tests; must identify likely verification
  gates.
- Model routing: prefer planning/long-context capability, medium or high
  thinking.
- Output schema: plan with goals, tasks, dependencies, risks, verification.
- Child workflow: eligible as parent coordinator; should not be delegated for
  code-writing child work.

### Coder

Purpose: implement focused code changes.

- Readonly: false
- Tools: edit/write/bash/read/scan/git status/diff plus project-approved tools
- Autonomy: can modify files and run narrow verification; should stop on
  repeated failure or expanding scope.
- Required evidence: `diff-summary`, `verification-result`, `open-concerns`
- Verification: required when code changes; suggested command comes from
  workflow contract.
- Model routing: prefer code-editing and test-debugging capability, medium
  thinking.
- Output schema: implementation summary, files changed, verification, concerns.
- Child workflow: eligible for implementation child workflows.

### Verifier

Purpose: independently check whether acceptance criteria are satisfied.

- Readonly: true by default, with command execution allowed for test/check
  commands when policy permits.
- Tools: read/scan/bash for approved verification commands/git diff
- Autonomy: can run verification commands; cannot make fixes unless explicitly
  escalated into Coder.
- Required evidence: `test-output`, `verification-result`, `failure-excerpts`
- Verification: this role is itself a verification gate producer.
- Model routing: prefer test-debugging/reasoning capability, medium thinking.
- Output schema: pass/fail/blocked, commands run, evidence refs, failure modes.
- Child workflow: eligible for verifier child workflows and post-implementation
  gates.

### Reviewer

Purpose: review changes for correctness, maintainability, safety, and product
fit.

- Readonly: true
- Tools: read/scan/git diff/web/recall
- Autonomy: cannot edit; may request changes or produce review findings.
- Required evidence: `review-findings`, `risk-notes`
- Verification: may recommend tests but does not need to run them.
- Model routing: prefer review/reasoning capability, high thinking.
- Output schema: findings by severity, positives, risks, suggested follow-ups.
- Child workflow: eligible as review gate.

### Researcher

Purpose: gather and summarize external or repository context.

- Readonly: true
- Tools: read/scan/web/recall
- Autonomy: cannot edit; must label external/untrusted sources.
- Required evidence: `source-citations`, `research-summary`, `trust-notes`
- Verification: not command-based; must cite sources or state missing evidence.
- Model routing: prefer research/long-context/summarization capability, low or
  medium thinking depending on task.
- Output schema: findings, citations, confidence, unresolved questions.
- Child workflow: eligible for research child workflows.

### Integrator

Purpose: combine outputs from planner/coder/verifier/reviewer/researcher into a
coherent final result.

- Readonly: false only when explicitly assigned to resolve integration conflicts;
  otherwise readonly by default.
- Tools: read/scan/git diff plus edit/write/bash when write-enabled by workflow.
- Autonomy: can reconcile child outputs, update evidence, and request follow-up
  child tasks; should not broaden scope.
- Required evidence: `integration-summary`, `decision-log`, `verification-result`
- Verification: must ensure required downstream gates are present or explain why
  blocked.
- Model routing: prefer long-context/synthesis/code-editing capability, medium
  thinking.
- Output schema: integrated summary, decisions made, unresolved conflicts,
  verification status.
- Child workflow: eligible as parent aggregator for delegated workflows.

## Child workflow use

Child workflow delegation selects a role explicitly. The helper layer resolves
that role through the registry, rejects unknown or non-delegable roles, and
projects role metadata into the child workflow contract.

A verifier child workflow, for example, receives:

- `role = "verifier"` on the workflow contract
- required evidence such as `test-output`, `verification-result`, and
  `failure-excerpts`
- suggested verification commands when present
- output schema metadata such as `verification-result` sections
- a handoff summary the parent can use when integrating child results

Example conceptual child plan:

```json
{
  "id": "child-verify-1",
  "role": "verifier",
  "contract": {
    "role": "verifier",
    "title": "Verify parser fix",
    "closeout_criteria": [
      "required evidence: test-output: Commands run and output refs",
      "required evidence: verification-result: Pass/fail/blocked status"
    ]
  },
  "evidence_handoff": {
    "role": "verifier",
    "required_evidence": ["test-output", "verification-result", "failure-excerpts"],
    "output_schema": "verification-result",
    "output_required_sections": ["status", "commands", "evidence", "failures"]
  }
}
```

Delegation is not autonomous spawning. A parent workflow still needs an explicit
task contract, and child roles still inherit sandbox, tool, and approval policy.

## Relationship to child workflow delegation

Child workflow delegation should select a role explicitly. The child workflow
inherits global/project policy, then applies the role profile as a narrowing
layer:

1. Parent workflow chooses role id and task contract.
2. Registry resolves role plus aliases and config overrides.
3. Tool exposure is intersected with global policy.
4. Prompt is built from parent task, role prompt template, and role instructions.
5. Required evidence and verification expectations are added to the child
   workflow contract.
6. Child closeout returns structured role output and artifact refs to the parent.

## Non-goals for the first implementation

- No mythology role names in core UX.
- No autonomous role spawning without workflow contract.
- No provider-specific routing logic in the role registry itself.
- No mandatory structured decoding until output schemas are wired into the LLM
  layer.
- No role permission that can bypass policy, sandbox, or user approval.
