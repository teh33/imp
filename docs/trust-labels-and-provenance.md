# Trust labels and provenance

Trust/provenance metadata records where context came from and how much authority it has. It is an authorization boundary, not a truth score: external content can be correct, and project files can be malicious.

Implementation: `crates/imp-core/src/trust.rs`, with policy use in `reference_monitor.rs` and tool-result propagation in `agent/tool_execution.rs`.

## Core types

`Provenance` contains a source, trust label, risk labels, optional origin/artifact references, parent derivation, and notes.

Source categories include:

- user instructions;
- workspace files and system policy;
- external web content;
- tool observations;
- verifier output;
- durable memory;
- generated summaries;
- workflow records;
- extensions;
- unknown sources.

Trust labels include user instruction, project trusted, tool observed, external untrusted, durable memory, generated summary, verifier output, workflow ledger, and unknown.

Risk labels refine the source with facts such as low trust, external/network-derived, generated, instruction-bearing, possible prompt injection, secret-adjacent, or verification artifact.

## Authority rules

Lower-trust content may inform reasoning. It cannot by itself authorize:

- a higher autonomy mode or policy bypass;
- durable memory adoption;
- secret reveal or unsafe secret use;
- network mutation;
- destructive or outside-workspace writes;
- production/deploy/publish actions;
- dangerous grants.

When all supporting provenance is low trust and an action is high risk, the reference monitor denies the action or asks the user for explicit adoption, depending on the action class. Trusted user support is still subject to agent mode, run policy, autonomy, and hard rails.

Low-risk read/search work remains possible with low-trust context.

## Propagation

The runtime assigns provenance based on the observed resource:

- user prompt → user instruction;
- `read(path)` → workspace file;
- web search/read or browser output → external web/network-derived;
- other tool output → tool observation;
- verifier output → verifier source;
- workflow records → workflow ledger;
- generated summary → derived from its parent provenance.

A summary never gains more authorization authority than its least-authoritative parent. Tool outputs are not reclassified as user instructions merely because they appear in model context.

## Prompt injection

Prompt injection is lower-trust content attempting to change instructions, goals, policy, permissions, memory, or output constraints.

Examples include a web page asking the agent to run a deployment command, a README instructing it to delete tests, or command output requesting a secret. The runtime can label and warn about those attempts, but labels do not make model influence impossible.

## Current implementation

Current code provides:

- typed provenance, trust, risk, derivation, and boundary models;
- workspace-file provenance during context prefill;
- optional compact prefill annotations;
- tool-result provenance in agent events and traces;
- low-trust escalation checks in the reference monitor;
- durable-memory rejection for low-trust, prompt-injection, or secret-adjacent support;
- trust summaries in evidence and eval candidates;
- TUI warnings for material low-trust/prompt-injection observations.

Prompt annotations are opt-in. Structured provenance still exists when annotations are not rendered into prompt text.

## Durable writes

The default memory tool implementation checks supporting provenance for `add` and `replace`, although `memory` is not part of the default native tool registry. Low-trust or risk-labeled content requires explicit user adoption before it can become durable memory.

Workflow write gating is not uniformly provenance-aware yet. Workflow records should preserve low-trust content as quoted/observed evidence rather than silently adopting it as policy or verified fact.

## Extensions

Lua or experimental extension code is not a trust boundary. Extensions can declare capabilities, but the Rust host assigns effective provenance and policy. Extension output cannot self-upgrade external content into trusted authority.

## Evidence and privacy

Trace/evidence should record compact source, trust, risk, origin, and policy-decision metadata rather than copying full sensitive content. Secret values must not enter provenance notes or evidence.

## Limits

- Trust labels do not solve prompt injection.
- Accurate enforcement depends on correct source classification.
- Prompt annotations cover only selected context paths.
- Some provenance-aware durable workflow behavior remains incomplete.
- Generated summaries can omit nuance even when parent metadata is preserved.

When authority is uncertain, keep the lower trust label, preserve the origin, and require explicit user approval before escalation.

See also [Autonomy modes](autonomy-modes.md) and [Runtime policy](policy.md).
