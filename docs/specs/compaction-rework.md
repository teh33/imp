# Compaction Rework Software Specification

## Intent and desired outcome

imp must preserve the operational state of long-running repository work when context is compacted. Compaction must be a validated, durable state transition rather than unchecked transcript summarization.

GPT-5.6 Luna is the default model-assisted compactor because it has a 1.05M-token context window, a 128k-token output limit, and lower input/output pricing than GPT-5.6 Sol and Terra. Luna improves rendering and compression quality; it does not replace runtime-owned authority.

## Users and actors

- The user invoking `/compact` manually.
- The agent runtime triggering compaction near the context threshold.
- GPT-5.6 Luna generating a compact continuation representation.
- The session runtime validating and persisting the transition.
- Host adapters displaying progress, failure, and resulting context usage.

## Scope

- One canonical compaction plan for manual and automatic triggers.
- Stable session-entry identity across repeated compaction.
- A typed continuation state containing authoritative work state and provenance.
- Selective, evidence-aware transcript reduction.
- A disk-backed rolling compaction checkpoint after each 128,000 uncovered input tokens, generated only when the turn is idle.
- GPT-5.6 Luna as the default configured summarizer model with `xhigh` thinking.
- Strict summarizer resolution, authentication, request, timeout, output, and validation failures.
- Durable, versioned compaction records with source boundaries and validation metadata.
- Repeated-compaction, restart, branch, failure, and quality regression tests.

## Non-goals

- Provider-native or remote compaction APIs.
- Re-enabling provider session/window protocols.
- Deleting raw transcript history.
- Treating model prose as authority over runtime task, workflow, effect, or verification state.
- Silently falling back to another model or deterministic summary after a configured model-assisted compaction begins.
- Reworking unrelated context assembly, provider transport, or UI architecture.

## Constraints and compatibility

- Existing session JSONL files and legacy `SessionEntry::Compaction` records remain readable.
- New compaction records are versioned and backward compatible.
- Raw branch messages remain intact and recoverable.
- Branch and fork behavior remains branch-local.
- Public model and provider resolution follows existing registry and auth routing rules.
- If `context.summarizer.model = "default"`, imp explicitly uses the active conversation model. The shipped default is `gpt-5.6-luna` with `thinking = "xhigh"`.
- `context.summarizer.checkpoint_interval_tokens` defaults to 128,000 and measures new input tokens not covered by the latest validated checkpoint.
- `context.summarizer.system_prompt` fully replaces the built-in compaction system prompt when configured.
- Luna requires an available route and valid credentials. imp does not silently use the active model when Luna is unavailable.

## Acceptance criteria

### Rolling checkpoint policy

Successful tool calls and other durable events mark checkpoint source state dirty but never independently call Luna. Once at least 128,000 uncovered input tokens have accumulated, imp schedules one checkpoint generation after the current turn is idle. Only one generation runs at a time.

Checkpoint N+1 consumes checkpoint N, the uncovered raw-entry delta, and refreshed authoritative state. Generation writes a temporary artifact, validates it, confirms its source fingerprint is still current, then atomically publishes it. Failed, cancelled, or stale generation leaves the prior checkpoint intact.

A session near one million input tokens should normally require approximately eight checkpoint generations, plus at most one final merge when compaction activates. The policy is token-based, not tool-call-based.

### Replacement, not parallel architecture

After the rolling checkpoint and canonical V2 transition are active, remove the deterministic fallback summary builder, request-local automatic message rewrite, retry wrapper built around optional summaries, provider-name strategy placeholder, and ad hoc TUI compaction agent path. No production path may retain the legacy implementation as a silent fallback. Legacy session records remain readable through isolated compatibility projection only.

### Stable repeated compaction

After any number of compaction cycles, the active branch contains exactly one current continuation representation plus the intended preserved raw tail. It never reintroduces compacted messages or loses intended kept messages because of projection-index drift.

### Authoritative continuation state

Before lossy transcript reduction, imp captures typed state for:

- objective and acceptance criteria;
- active user constraints, preferences, and corrections;
- decisions and rationale;
- repository, branch, and session facts;
- edited paths and recorded effects;
- verification commands and outcomes;
- pending obligations;
- blockers, risks, and open questions;
- workflow/task identifiers and status;
- artifact and evidence references.

Every field carries enough provenance to trace it to runtime state, a session entry, or a tool observation.

### Evidence-aware reduction

Large observations may be omitted or referenced, but compaction retains structured edit metadata, failed checks, concise errors, artifact references, current Git/workflow state, and other facts required to continue safely.

### Validated Luna output

The Luna response is parsed as a versioned structured compaction document. Validation rejects output that omits or contradicts required continuation state, claims unsupported completion, references invalid boundaries, or does not fit the resolved post-compaction budget.

### Strict manual failure

If the configured summarizer cannot be resolved or authenticated, the request fails, times out, returns invalid output, contradicts authoritative state, or exceeds budget, manual compaction fails. No compaction entry is appended and active context remains unchanged.

### Strict automatic failure

If automatic compaction fails, imp preserves the original active context and surfaces the failure.

- If the original provider request still fits the effective input budget, imp continues that request once without compaction.
- If it does not fit, imp terminates with the explicit context-full/compaction-failed diagnostic.
- It does not silently activate a deterministic or alternate-model summary.

### Canonical manual and automatic semantics

Manual and automatic triggers use the same plan, validation, persistence, and active-history projection. They may differ only in trigger policy and user presentation.

### Durable provenance

A new compaction record stores at least:

- schema version;
- trigger and reason;
- compacted source entry IDs;
- preserved entry IDs;
- typed continuation state or a durable embedded representation;
- rendered summary;
- summarizer model and provider route;
- source-state fingerprint;
- validation result;
- request-aware token counts before and after.

### Honest observability

The runtime and UI expose compaction start, success, and failure. They distinguish provider usage, local estimates, and unknown post-compaction usage. They never report success or saved-token counts when validation or persistence failed.

## Behavior scenarios

### Manual success

Given a long active branch and available Luna credentials, when the user invokes `/compact`, imp plans the source boundary using stable entry IDs, captures continuation state, obtains and validates Luna output, persists one versioned compaction record, and reloads active history from the durable session projection.

### Manual Luna failure

Given unchanged active history, when Luna resolution, authentication, transport, timeout, parsing, or validation fails, imp reports the reason and leaves the session branch and active context unchanged.

### Automatic failure while request fits

Given threshold-triggered automatic compaction and an original request below the effective hard input limit, when Luna compaction fails, imp emits a visible failure and sends the original sanitized request once.

### Automatic failure while request does not fit

Given a request at or above the effective hard input limit, when Luna compaction fails, imp preserves durable history and terminates with an explicit failure containing both context and compaction diagnostics.

### Repeated compaction after restart

Given a session compacted more than once and reloaded from disk, active history resolves from stable persisted entry IDs and matches the history calculated before restart.

### Contradictory summary

Given authoritative verification failure or pending obligations, when Luna claims work is complete or omits the obligation, validation rejects the output and no compaction transition occurs.

## Assumptions

- GPT-5.6 Luna remains registered as `gpt-5.6-luna` with OpenAI metadata.
- Luna availability can use any existing route that explicitly supports the model, but must not be fabricated by forcing an unsupported provider hint.
- Initial implementation may use deterministic fixtures or a protocol-faithful fake for generated-output validation; live-model evaluation is separate evidence.
