# imp “sports car” product handoff

Status: product vision and excellence checklist
Audience: maintainers working on the agent loop, orchestration, tools, TUI, policy, sessions, providers, evaluation, and releases

## Why this document exists

imp is not trying to become the everyman’s open-source coding agent. OpenCode is built for breadth: many clients, integrations, providers, deployment models, and user types. imp should not copy that strategy.

imp is the sports car. It should be the fastest, sharpest, most controllable coding agent for serious developers doing substantial work. It can carry less if it is better at the work it chooses to own.

The product promise is:

> Give imp a difficult repository task with real acceptance criteria. It can understand the codebase, plan and orchestrate the work, execute it with minimal waste, survive interruption, verify the result, and show exactly why the work is complete.

This document lists the capabilities imp must excel in to make that promise true. It is not a parity roadmap. A feature that helps OpenCode serve more people is not automatically useful to imp.

## How to use this checklist

A checked item means the behavior has been demonstrated against a named test, benchmark, fixture, or release criterion. Existing code does not automatically earn a check. The standard is product-level excellence, not implementation presence.

For each major section:

1. identify the current baseline;
2. link the implementation and tests;
3. define a measurable target where one is missing;
4. fix the highest-impact failure;
5. record evidence;
6. rerun the same test after material changes.

Suggested labels:

- **Core:** directly determines whether imp is better at serious coding work.
- **Control:** makes powerful behavior understandable and steerable.
- **Proof:** demonstrates that claims are true.
- **Enabler:** supports the core experience without becoming the product itself.
- **Boundary:** protects imp from turning into a broad, incoherent platform.

## Integration audit rubric

Last audited: 2026-07-10 against `fd8601213` plus the current uncommitted readiness ledger/checklist work. Percentages estimate end-to-end product integration, not code volume:

- **0–19% · Not started:** no reproducible product behavior.
- **20–39% · Scaffold:** types, plans, or smoke-level pieces exist.
- **40–59% · Partial:** useful implementation exists, but the path is incomplete or not integrated.
- **60–79% · Integrated:** usable on normal paths, with material edge/proof gaps.
- **80–89% · Strong:** broadly integrated and tested; remaining work is hardening or measured proof.
- **90–100% · Evidenced:** behavior is demonstrated by focused tests or release evidence.

Checkboxes remain open unless a named acceptance artifact proves the full statement. A high percentage therefore does not automatically mean the original excellence criterion is complete. Scores are rounded judgment calls and should move only with linked code, tests, benchmarks, or release evidence.

### Section readiness

<!-- readiness-summary:start -->
- **Overall checklist: 61%** (unweighted mean across all 481 criteria)
- **Preamble: 53%** (partial; 11 criteria)
- **1. Agent execution quality: 70%** (integrated; 26 criteria)
- **2. Workflow orchestration: 55%** (partial; 37 criteria)
- **3. Context intelligence: 68%** (integrated; 25 criteria)
- **4. Native tool quality: 80%** (strong; 49 criteria)
- **5. Terminal experience: 70%** (integrated; 34 criteria)
- **6. Performance and resource discipline: 54%** (partial; 27 criteria)
- **7. Reliability, cancellation, and recovery: 58%** (partial; 30 criteria)
- **8. Policy and autonomy: 69%** (integrated; 25 criteria)
- **9. Verification and evidence: 64%** (integrated; 24 criteria)
- **10. Session control and experimentation: 67%** (integrated; 23 criteria)
- **11. Provider depth: 65%** (integrated; 23 criteria)
- **12. Configuration and diagnostics: 62%** (integrated; 16 criteria)
- **13. Headless and editor operation: 58%** (partial; 19 criteria)
- **14. Evaluation and competitive proof: 14%** (not started; 28 criteria)
- **15. Release quality and installation: 54%** (partial; 20 criteria)
- **16. Architecture and product boundaries: 69%** (integrated; 30 criteria)
- **17. Documentation for expert users: 64%** (integrated; 10 criteria)
- **18. Recommended execution order: 46%** (partial; 19 criteria)
- **19. Handoff record: 69%** (integrated; 5 criteria)
<!-- readiness-summary:end -->

## Product-level exit criteria

- [ ] Difficult repository tasks complete with fewer wasted turns, reads, and tokens than the comparison baseline. — **45% · Partial.** Partial support exists in cross-cutting implementation and existing tests; no full product-exit suite yet; important end-to-end behavior or proof is missing.
- [ ] Multi-step work runs through durable workflows with bounded parallel workers and clear closeout evidence. — **38% · Scaffold.** Contracts and read models exist, but real concurrent worker execution/integration is not production-complete.
- [ ] Interrupting provider, tool, mutation, worker, and verification phases produces deterministic recovery behavior. — **62% · Integrated.** Usable through cross-cutting implementation and existing tests; no full product-exit suite yet, but not yet demonstrated at the full checklist standard.
- [ ] The TUI shows objective, progress, blockers, workers, policy, verification, and changes without requiring internal-file inspection. — **42% · Partial.** Partial support exists in cross-cutting implementation and existing tests; no full product-exit suite yet; important end-to-end behavior or proof is missing.
- [ ] Users can steer, queue, cancel, branch, and resume work without corrupting durable state. — **45% · Partial.** Partial support exists in cross-cutting implementation and existing tests; no full product-exit suite yet; important end-to-end behavior or proof is missing.
- [ ] Supported provider paths are deep, current, and tested rather than merely numerous. — **62% · Integrated.** Usable through cross-cutting implementation and existing tests; no full product-exit suite yet, but not yet demonstrated at the full checklist standard.
- [ ] Native tools and context selection consistently save time and tokens compared with shell-only behavior. — **55% · Partial.** Partial support exists in cross-cutting implementation and existing tests; no full product-exit suite yet; important end-to-end behavior or proof is missing.
- [ ] Policy is predictable enough that users can explain what a run may do before starting it. — **50% · Partial.** Partial support exists in cross-cutting implementation and existing tests; no full product-exit suite yet; important end-to-end behavior or proof is missing.
- [ ] Every completion claim points to acceptance criteria, verification, and relevant evidence. — **72% · Integrated.** Usable through cross-cutting implementation and existing tests; no full product-exit suite yet, but not yet demonstrated at the full checklist standard.
- [ ] Startup, interaction, cancellation, session loading, and tool dispatch stay within explicit performance budgets. — **28% · Scaffold.** Current GitHub workflows package edge/releases; they do not enforce this regression gate.
- [ ] Features outside the core thesis remain extensions, separate tools, or deliberate exclusions. — **88% · Strong.** Implemented and exercised through cross-cutting implementation and existing tests; no full product-exit suite yet; remaining work is edge hardening or product proof.

---

# 1. Agent execution quality

**Class:** Core

The agent loop is the engine. It must complete work cleanly, not merely expose many capabilities.

## Objective understanding

- [ ] Preserve the requested outcome, constraints, non-goals, and acceptance criteria as explicit runtime state. — **72% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Distinguish explanation, research, planning, implementation, review, debugging, and orchestration without brittle keyword behavior. — **68% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Ask for clarification only when a missing decision materially affects correctness or risk. — **66% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Avoid questions repository inspection can answer. — **76% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Detect conflicting instructions before mutation. — **50% · Partial.** Partial support exists in `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests; important end-to-end behavior or proof is missing.
- [ ] Retain important constraints through long runs and compaction. — **72% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Update the objective after steering without silently dropping prior commitments. — **70% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Show the current objective concisely to the user. — **48% · Partial.** Partial support exists in `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests; important end-to-end behavior or proof is missing.

## Turn discipline

- [ ] Every turn advances the objective, resolves uncertainty, performs a necessary action, or verifies an outcome. — **76% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Prevent repeated reads, searches, commands, and failed edits when no new evidence justifies them. — **72% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Prefer the narrowest sufficient operation over broad repository scans. — **78% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Stop exploring once enough evidence exists to act safely. — **68% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Recover deliberately after command, tool, or provider failure instead of reflexively retrying. — **78% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Separate progress from narration; routine work should not produce excessive status text. — **76% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Prevent completion after edits without required verification. — **86% · Strong.** Implemented and exercised through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests; remaining work is edge hardening or product proof.
- [ ] Stop cleanly when remaining work is blocked or needs a decision. — **82% · Strong.** Implemented and exercised through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests; remaining work is edge hardening or product proof.

## Completion judgment

- [ ] Tie completion to original acceptance criteria, not the last successful tool call. — **76% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Reconcile edited files, tests, workflow state, policy obligations, and unresolved concerns. — **78% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Use `DONE`, `DONE_WITH_CONCERNS`, `BLOCKED`, and `NEEDS_CONTEXT` consistently. — **82% · Strong.** Implemented and exercised through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests; remaining work is edge hardening or product proof.
- [ ] State material limitations directly. — **86% · Strong.** Implemented and exercised through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests; remaining work is edge hardening or product proof.
- [ ] Never silently downgrade a required failed check. — **90% · Evidenced.** Demonstrated in `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests; preserve the contract and regression coverage.
- [ ] Separate optional improvements from required completion. — **78% · Integrated.** Usable through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests, but not yet demonstrated at the full checklist standard.
- [ ] Produce a concise final answer with result, verification, and material risks. — **84% · Strong.** Implemented and exercised through `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests; remaining work is edge hardening or product proof.

## Proof

- [ ] Maintain fixtures for false completion, repeated exploration, failed-command recovery, conflicting instructions, and ambiguity. — **32% · Scaffold.** Closeout safeguards exist, but the dedicated false-success corpus and tracked rate are incomplete.
- [ ] Track completion correctness, false-success rate, turns, tools, retries, and tokens per task. — **35% · Scaffold.** Only foundations/scaffolding are present in `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests; this is not an integrated product behavior.
- [ ] Add a regression case whenever a real run wastes substantial work or stops incorrectly. — **38% · Scaffold.** Only foundations/scaffolding are present in `agent/task_state`, `turn_assessment`, `loop_policy`, obligations, and agent tests; this is not an integrated product behavior.

---

# 2. Workflow orchestration

**Class:** Core

Workflows are imp’s durable execution contract, not project-management decoration.

## Planning and decomposition

- [ ] Convert broad goals into steps with explicit outputs and acceptance criteria. — **70% · Integrated.** Usable through `workflow/schema`, service/tool actions, child contracts, and workflow tests, but not yet demonstrated at the full checklist standard.
- [ ] Represent dependencies, blockers, required context, checks, and non-goals. — **82% · Strong.** Implemented and exercised through `workflow/schema`, service/tool actions, child contracts, and workflow tests; remaining work is edge hardening or product proof.
- [ ] Keep steps cohesive enough to execute and verify independently. — **65% · Integrated.** Usable through `workflow/schema`, service/tool actions, child contracts, and workflow tests, but not yet demonstrated at the full checklist standard.
- [ ] Avoid fake decomposition where every step needs the same global context. — **55% · Partial.** Partial support exists in `workflow/schema`, service/tool actions, child contracts, and workflow tests; important end-to-end behavior or proof is missing.
- [ ] Identify work that can run concurrently without overlapping ownership. — **68% · Integrated.** Usable through `workflow/schema`, service/tool actions, child contracts, and workflow tests, but not yet demonstrated at the full checklist standard.
- [ ] Identify decisions requiring prototypes or user input. — **58% · Partial.** Partial support exists in `workflow/schema`, service/tool actions, child contracts, and workflow tests; important end-to-end behavior or proof is missing.
- [ ] Keep simple tasks out of workflow machinery when direct execution is clearer. — **76% · Integrated.** Usable through `workflow/schema`, service/tool actions, child contracts, and workflow tests, but not yet demonstrated at the full checklist standard.

## Durable execution

- [ ] Select the next runnable step deterministically. — **92% · Evidenced.** Demonstrated in `workflow/schema`, service/tool actions, child contracts, and workflow tests; preserve the contract and regression coverage.
- [ ] Explain why no step is runnable. — **88% · Strong.** Implemented and exercised through `workflow/schema`, service/tool actions, child contracts, and workflow tests; remaining work is edge hardening or product proof.
- [ ] Persist status transitions and reasons. — **88% · Strong.** Implemented and exercised through `workflow/schema`, service/tool actions, child contracts, and workflow tests; remaining work is edge hardening or product proof.
- [ ] Keep event history append-only and reviewable. — **88% · Strong.** Implemented and exercised through `workflow/schema`, service/tool actions, child contracts, and workflow tests; remaining work is edge hardening or product proof.
- [ ] Reject invalid or premature closeout transitions. — **78% · Integrated.** Usable through `workflow/schema`, service/tool actions, child contracts, and workflow tests, but not yet demonstrated at the full checklist standard.
- [ ] Reconcile step, check, acceptance, and workflow status after completion. — **84% · Strong.** Implemented and exercised through `workflow/schema`, service/tool actions, child contracts, and workflow tests; remaining work is edge hardening or product proof.
- [ ] Resume from persisted workflow state after restart. — **82% · Strong.** Implemented and exercised through `workflow/schema`, service/tool actions, child contracts, and workflow tests; remaining work is edge hardening or product proof.
- [ ] Detect state/filesystem divergence before continuing. — **45% · Partial.** Partial support exists in `workflow/schema`, service/tool actions, child contracts, and workflow tests; important end-to-end behavior or proof is missing.
- [ ] Prevent workflow state and event history from silently disagreeing after interruption. — **62% · Integrated.** Usable through `workflow/schema`, service/tool actions, child contracts, and workflow tests, but not yet demonstrated at the full checklist standard.

## Bounded parallel workers

- [ ] Launch workers only from explicit bounded contracts. — **72% · Integrated.** Usable through `workflow/schema`, service/tool actions, child contracts, and workflow tests, but not yet demonstrated at the full checklist standard.
- [ ] Give each worker a narrow objective, context, tool surface, write scope, and completion condition. — **72% · Integrated.** Usable through `workflow/schema`, service/tool actions, child contracts, and workflow tests, but not yet demonstrated at the full checklist standard.
- [ ] Use isolated worktrees when concurrent writes could overlap. — **25% · Scaffold.** Core worktree helpers exist; normal CLI orchestration and closeout wiring remain incomplete.
- [ ] Detect overlapping file or resource ownership before dispatch. — **55% · Partial.** Partial support exists in `workflow/schema`, service/tool actions, child contracts, and workflow tests; important end-to-end behavior or proof is missing.
- [ ] Bound concurrency based on task shape and machine/provider limits. — **35% · Scaffold.** Only foundations/scaffolding are present in `workflow/schema`, service/tool actions, child contracts, and workflow tests; this is not an integrated product behavior.
- [ ] Persist worker identity, parent, status, scope, artifacts, and result. — **36% · Scaffold.** Only foundations/scaffolding are present in `workflow/schema`, service/tool actions, child contracts, and workflow tests; this is not an integrated product behavior.
- [ ] Allow independent workers to progress without serializing unrelated sessions. — **30% · Scaffold.** Only foundations/scaffolding are present in `workflow/schema`, service/tool actions, child contracts, and workflow tests; this is not an integrated product behavior.
- [ ] Cancel child work predictably when the parent is cancelled or invalidated. — **34% · Scaffold.** Only foundations/scaffolding are present in `workflow/schema`, service/tool actions, child contracts, and workflow tests; this is not an integrated product behavior.
- [ ] Preserve concerns and failed checks while collecting worker results. — **58% · Partial.** Partial support exists in `workflow/schema`, service/tool actions, child contracts, and workflow tests; important end-to-end behavior or proof is missing.
- [ ] Verify worker output before integration. — **42% · Partial.** Partial support exists in `workflow/schema`, service/tool actions, child contracts, and workflow tests; important end-to-end behavior or proof is missing.
- [ ] Detect and explain merge or semantic conflicts. — **28% · Scaffold.** Only foundations/scaffolding are present in `workflow/schema`, service/tool actions, child contracts, and workflow tests; this is not an integrated product behavior.
- [ ] Clean up only agent-owned inactive worktrees and branches. — **55% · Partial.** Core worktree helpers exist; normal CLI orchestration and closeout wiring remain incomplete.

## Orchestration UX

- [ ] Show current workflow, active step, queued steps, blocked steps, and progress in the TUI. — **42% · Partial.** Partial support exists in `workflow/schema`, service/tool actions, child contracts, and workflow tests; important end-to-end behavior or proof is missing.
- [ ] Show active workers and scopes without flooding chat. — **30% · Scaffold.** Contracts and read models exist, but real concurrent worker execution/integration is not production-complete.
- [ ] Make worker completion, failure, cancellation, and integration visible. — **34% · Scaffold.** Only foundations/scaffolding are present in `workflow/schema`, service/tool actions, child contracts, and workflow tests; this is not an integrated product behavior.
- [ ] Surface the next action and why it was selected. — **68% · Integrated.** Usable through `workflow/schema`, service/tool actions, child contracts, and workflow tests, but not yet demonstrated at the full checklist standard.
- [ ] Let users pause, resume, cancel, or redirect workflows. — **28% · Scaffold.** Only foundations/scaffolding are present in `workflow/schema`, service/tool actions, child contracts, and workflow tests; this is not an integrated product behavior.
- [ ] Make concerns and blockers obvious at closeout. — **58% · Partial.** Partial support exists in `workflow/schema`, service/tool actions, child contracts, and workflow tests; important end-to-end behavior or proof is missing.

## Proof

- [ ] Test independent parallel branches, overlapping writes, worker failure, parent cancellation, and restart during active work. — **28% · Scaffold.** Only foundations/scaffolding are present in `workflow/schema`, service/tool actions, child contracts, and workflow tests; this is not an integrated product behavior.
- [ ] Demonstrate a multi-hour workflow resuming after termination without duplicate side effects. — **10% · Not started.** No reproducible product-level evidence is recorded; treat this as not started.
- [ ] Compare sequential and parallel execution for wall time, cost, conflicts, and verification quality. — **18% · Not started.** No reproducible product-level evidence is recorded; treat this as not started.

---

# 3. Context intelligence

**Class:** Core

imp should win by giving the model less irrelevant context and better relevant context.

## Repository orientation

- [ ] Detect project structure, package boundaries, instructions, generated code, and tests quickly. — **86% · Strong.** Implemented and exercised through `scan`, context prefill/projection, compaction, and context tests; remaining work is edge hardening or product proof.
- [ ] Prefer structural search for symbols and relationships when text search would be noisy. — **90% · Evidenced.** Demonstrated in `scan`, context prefill/projection, compaction, and context tests; preserve the contract and regression coverage.
- [ ] Use raw text search for strings, configuration, errors, and unstructured content. — **86% · Strong.** Implemented and exercised through `scan`, context prefill/projection, compaction, and context tests; remaining work is edge hardening or product proof.
- [ ] Read the smallest useful ranges first. — **88% · Strong.** Implemented and exercised through `scan`, context prefill/projection, compaction, and context tests; remaining work is edge hardening or product proof.
- [ ] Expand context only when evidence requires it. — **78% · Integrated.** Usable through `scan`, context prefill/projection, compaction, and context tests, but not yet demonstrated at the full checklist standard.
- [ ] Track inspected material to reduce redundant reads. — **62% · Integrated.** Usable through `scan`, context prefill/projection, compaction, and context tests, but not yet demonstrated at the full checklist standard.
- [ ] Preserve package-specific instructions and dependency boundaries. — **84% · Strong.** Implemented and exercised through `scan`, context prefill/projection, compaction, and context tests; remaining work is edge hardening or product proof.

## Context assembly

- [ ] Include objective, relevant constraints, selected history, workflow state, and necessary tool observations. — **78% · Integrated.** Usable through `scan`, context prefill/projection, compaction, and context tests, but not yet demonstrated at the full checklist standard.
- [ ] Exclude stale chatter and superseded output. — **75% · Integrated.** Usable through `scan`, context prefill/projection, compaction, and context tests, but not yet demonstrated at the full checklist standard.
- [ ] Bound large command output before model context. — **90% · Evidenced.** Demonstrated in `scan`, context prefill/projection, compaction, and context tests; preserve the contract and regression coverage.
- [ ] Keep worker context narrower than parent context. — **62% · Integrated.** Usable through `scan`, context prefill/projection, compaction, and context tests, but not yet demonstrated at the full checklist standard.
- [ ] Avoid leaking unrelated session or project state. — **82% · Strong.** Implemented and exercised through `scan`, context prefill/projection, compaction, and context tests; remaining work is edge hardening or product proof.
- [ ] Explain omissions when they can affect correctness. — **42% · Partial.** Partial support exists in `scan`, context prefill/projection, compaction, and context tests; important end-to-end behavior or proof is missing.
- [ ] Preserve exact excerpts where summaries would lose required detail. — **76% · Integrated.** Usable through `scan`, context prefill/projection, compaction, and context tests, but not yet demonstrated at the full checklist standard.

## Compaction

- [ ] Compact before exhaustion without discarding useful detail too early. — **80% · Strong.** Implemented and exercised through `scan`, context prefill/projection, compaction, and context tests; remaining work is edge hardening or product proof.
- [ ] Preserve decisions, constraints, questions, edits, verification, and obligations. — **82% · Strong.** Implemented and exercised through `scan`, context prefill/projection, compaction, and context tests; remaining work is edge hardening or product proof.
- [ ] Record what was compacted and the summary used. — **90% · Evidenced.** Demonstrated in `scan`, context prefill/projection, compaction, and context tests; preserve the contract and regression coverage.
- [ ] Detect summaries conflicting with durable state. — **35% · Scaffold.** Only foundations/scaffolding are present in `scan`, context prefill/projection, compaction, and context tests; this is not an integrated product behavior.
- [ ] Reload authoritative workflow/session state after compaction. — **70% · Integrated.** Usable through `scan`, context prefill/projection, compaction, and context tests, but not yet demonstrated at the full checklist standard.
- [ ] Test long sessions with repeated compaction. — **66% · Integrated.** Usable through `scan`, context prefill/projection, compaction, and context tests, but not yet demonstrated at the full checklist standard.

## Measurement

- [ ] Track tokens, cache use, selected files, lines read, and repeated reads. — **58% · Partial.** Partial support exists in `scan`, context prefill/projection, compaction, and context tests; important end-to-end behavior or proof is missing.
- [ ] Measure context-assembly time. — **60% · Integrated.** Usable through `scan`, context prefill/projection, compaction, and context tests, but not yet demonstrated at the full checklist standard.
- [ ] Compare structural search with grep/read workflows on representative tasks. — **38% · Scaffold.** The Dirac A/B harness exists, but no maintained large-repository comparison establishes latency and token savings.
- [ ] Maintain token-efficiency targets per benchmark class. — **30% · Scaffold.** Benchmark scaffolding exists, but no maintained baseline/gate proves this item continuously.
- [ ] Flag context regressions that increase tokens without improving completion. — **20% · Scaffold.** Only foundations/scaffolding are present in `scan`, context prefill/projection, compaction, and context tests; this is not an integrated product behavior.

---

# 4. Native tool quality

**Class:** Core

Every native tool must be safer, clearer, or more efficient than shell improvisation.

## Common contract

- [ ] Use narrow typed schemas with explicit mutability. — **92% · Evidenced.** Demonstrated in native tool implementations and focused tool/TUI renderer tests; preserve the contract and regression coverage.
- [ ] Reject malformed or ambiguous arguments before side effects. — **86% · Strong.** Native tools generally validate typed parameters and focused tests cover malformed calls, but cross-tool conformance is not proven for every side-effecting tool.
- [ ] Return actionable recovery errors. — **78% · Integrated.** Usable through native tool implementations and focused tool/TUI renderer tests, but not yet demonstrated at the full checklist standard.
- [ ] Bound output and support focused continuation. — **84% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Support cancellation and timeouts where work can block. — **82% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Record policy decisions and side-effect boundaries. — **80% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Render concise TUI summaries with expandable detail. — **88% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Avoid overlap unless each tool has a clear selection rule. — **68% · Integrated.** Usable through native tool implementations and focused tool/TUI renderer tests, but not yet demonstrated at the full checklist standard.
- [ ] Remove tools that do not outperform simpler composition. — **72% · Integrated.** Usable through native tool implementations and focused tool/TUI renderer tests, but not yet demonstrated at the full checklist standard.

## Read, scan, and search

- [ ] Keep ranged reads reliable for text, large files, and supported images. — **92% · Evidenced.** Demonstrated in native tool implementations and focused tool/TUI renderer tests; preserve the contract and regression coverage.
- [ ] Preserve line numbers and stable anchors where possible. — **92% · Evidenced.** Demonstrated in native tool implementations and focused tool/TUI renderer tests; preserve the contract and regression coverage.
- [ ] Keep tree-sitter search/extraction fast across supported languages. — **86% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Degrade explicitly for unsupported parsers or languages. — **88% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Return enclosing symbols and bounded source without unrelated files. — **88% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Make results easy to feed into follow-up reads or edits. — **86% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Benchmark search latency and output-token cost on large repositories. — **35% · Scaffold.** Benchmark scaffolding exists, but no maintained baseline/gate proves this item continuously.

## Edit and write

- [ ] Require exact reviewable mutation intent. — **92% · Evidenced.** Demonstrated in native tool implementations and focused tool/TUI renderer tests; preserve the contract and regression coverage.
- [ ] Detect stale source and conflicts before writing. — **90% · Evidenced.** Demonstrated in native tool implementations and focused tool/TUI renderer tests; preserve the contract and regression coverage.
- [ ] Keep transactional multi-file edits atomic or fail without partial mutation. — **62% · Integrated.** All edits are validated before writes and checkpoints are created, but writes are sequential with no rollback if a later filesystem write fails.
- [ ] Preserve encoding, line endings, and byte-order marks. — **52% · Partial.** CRLF/LF is detected and tested, but reads are UTF-8-lossy and BOM/non-UTF-8 round trips are not preserved as a general contract.
- [ ] Validate syntax where supported without pretending syntax proves correctness. — **90% · Evidenced.** Demonstrated in native tool implementations and focused tool/TUI renderer tests; preserve the contract and regression coverage.
- [ ] Report files and line deltas accurately. — **82% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Integrate mutations with session diff and evidence. — **72% · Integrated.** Usable through native tool implementations and focused tool/TUI renderer tests, but not yet demonstrated at the full checklist standard.
- [ ] Keep write policy authoritative for every mutation route. — **88% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.

## Bash and processes

- [ ] Stream output without blocking the TUI. — **88% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Cancel the owned process tree promptly without killing unrelated processes. — **86% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Bound retained output while preserving useful diagnostics. — **88% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Record exit status, duration, cwd, timeout, and cancellation reason. — **86% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Classify dangerous commands before execution. — **78% · Integrated.** Usable through native tool implementations and focused tool/TUI renderer tests, but not yet demonstrated at the full checklist standard.
- [ ] Handle interactive or hanging commands predictably. — **76% · Integrated.** Usable through native tool implementations and focused tool/TUI renderer tests, but not yet demonstrated at the full checklist standard.

## Git and worktrees

- [ ] Provide typed status, diff, log, staging, commit, restore, and worktree operations. — **92% · Evidenced.** Demonstrated in native tool implementations and focused tool/TUI renderer tests; preserve the contract and regression coverage.
- [ ] Preserve unrelated dirty work and the existing index. — **88% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Prevent accidental secret, cache, and runtime-state staging. — **62% · Integrated.** Usable through native tool implementations and focused tool/TUI renderer tests, but not yet demonstrated at the full checklist standard.
- [ ] Make branch/worktree ownership explicit. — **65% · Integrated.** Usable through native tool implementations and focused tool/TUI renderer tests, but not yet demonstrated at the full checklist standard.
- [ ] Refuse destructive history changes unless directly authorized. — **86% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Show staged and unstaged changes before commit. — **72% · Integrated.** Usable through native tool implementations and focused tool/TUI renderer tests, but not yet demonstrated at the full checklist standard.
- [ ] Keep non-local remote writes disabled unless authorized. — **68% · Integrated.** Usable through native tool implementations and focused tool/TUI renderer tests, but not yet demonstrated at the full checklist standard.

## Web and browser

- [ ] Separate read-only retrieval from stateful browser input. — **90% · Evidenced.** Demonstrated in native tool implementations and focused tool/TUI renderer tests; preserve the contract and regression coverage.
- [ ] Block private-network targets unless explicitly allowed. — **82% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Apply time, response-size, and navigation bounds. — **88% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Keep browser input denied by default. — **92% · Evidenced.** Demonstrated in native tool implementations and focused tool/TUI renderer tests; preserve the contract and regression coverage.
- [ ] Prevent secret values from entering requests without mediated policy. — **78% · Integrated.** Usable through native tool implementations and focused tool/TUI renderer tests, but not yet demonstrated at the full checklist standard.
- [ ] Preserve source URLs for externally derived claims. — **82% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Clean up browser subprocesses and sessions reliably. — **88% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.

## User questions

- [ ] Use structured choices when the decision space is known. — **90% · Evidenced.** Demonstrated in native tool implementations and focused tool/TUI renderer tests; preserve the contract and regression coverage.
- [ ] Allow free-form answers when choices would constrain intent. — **88% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Explain why an answer is needed and what it changes. — **62% · Integrated.** Usable through native tool implementations and focused tool/TUI renderer tests, but not yet demonstrated at the full checklist standard.
- [ ] Avoid asking for approval after an action occurred. — **86% · Strong.** Implemented and exercised through native tool implementations and focused tool/TUI renderer tests; remaining work is edge hardening or product proof.
- [ ] Resume the exact blocked operation after an answer. — **48% · Partial.** Partial support exists in native tool implementations and focused tool/TUI renderer tests; important end-to-end behavior or proof is missing.

---

# 5. Terminal experience

**Class:** Core and Control

The TUI is the cockpit. It must be exceptional for the developer imp serves.

## Prompting and steering

- [ ] Support fast multiline editing, history, context attachment, commands, and shell shortcuts. — **82% · Strong.** Implemented and exercised through `imp-tui` app/views and session-lifecycle/rendering tests; remaining work is edge hardening or product proof.
- [ ] Keep typing responsive while the agent streams or tools run. — **78% · Integrated.** Usable through `imp-tui` app/views and session-lifecycle/rendering tests, but not yet demonstrated at the full checklist standard.
- [ ] Distinguish steer, queue, follow-up, and new-session intent. — **82% · Strong.** Implemented and exercised through `imp-tui` app/views and session-lifecycle/rendering tests; remaining work is edge hardening or product proof.
- [ ] Show queued input and allow safe removal or reordering. — **52% · Partial.** Partial support exists in `imp-tui` app/views and session-lifecycle/rendering tests; important end-to-end behavior or proof is missing.
- [ ] Make cancellation immediate and visible. — **78% · Integrated.** Usable through `imp-tui` app/views and session-lifecycle/rendering tests, but not yet demonstrated at the full checklist standard.
- [ ] Preserve draft input across navigation and transient errors. — **75% · Integrated.** Usable through `imp-tui` app/views and session-lifecycle/rendering tests, but not yet demonstrated at the full checklist standard.
- [ ] Show model, role, thinking, autonomy, cwd, session, and workflow scope without clutter. — **78% · Integrated.** Usable through `imp-tui` app/views and session-lifecycle/rendering tests, but not yet demonstrated at the full checklist standard.

## Execution legibility

- [ ] Show what imp is doing now at a glance. — **82% · Strong.** Implemented and exercised through `imp-tui` app/views and session-lifecycle/rendering tests; remaining work is edge hardening or product proof.
- [ ] Show objective and active workflow step. — **48% · Partial.** Partial support exists in `imp-tui` app/views and session-lifecycle/rendering tests; important end-to-end behavior or proof is missing.
- [ ] Distinguish thinking, waiting, tool execution, verification, blocked, and idle states. — **80% · Strong.** Implemented and exercised through `imp-tui` app/views and session-lifecycle/rendering tests; remaining work is edge hardening or product proof.
- [ ] Show active workers and aggregate progress. — **30% · Scaffold.** Contracts and read models exist, but real concurrent worker execution/integration is not production-complete.
- [ ] Show pending decisions and approvals prominently. — **80% · Strong.** Implemented and exercised through `imp-tui` app/views and session-lifecycle/rendering tests; remaining work is edge hardening or product proof.
- [ ] Show verification gates and failures. — **78% · Integrated.** Usable through `imp-tui` app/views and session-lifecycle/rendering tests, but not yet demonstrated at the full checklist standard.
- [ ] Show changed files and diff summary. — **58% · Partial.** Partial support exists in `imp-tui` app/views and session-lifecycle/rendering tests; important end-to-end behavior or proof is missing.
- [ ] Show context, elapsed time, tokens, and cost without overwhelming the main view. — **78% · Integrated.** Usable through `imp-tui` app/views and session-lifecycle/rendering tests, but not yet demonstrated at the full checklist standard.
- [ ] Explain stop reasons plainly. — **72% · Integrated.** Usable through `imp-tui` app/views and session-lifecycle/rendering tests, but not yet demonstrated at the full checklist standard.

## Inspection

- [ ] Keep routine tool calls compact. — **90% · Evidenced.** Demonstrated in `imp-tui` app/views and session-lifecycle/rendering tests; preserve the contract and regression coverage.
- [ ] Expand tool input, output, timing, and errors on demand. — **88% · Strong.** Implemented and exercised through `imp-tui` app/views and session-lifecycle/rendering tests; remaining work is edge hardening or product proof.
- [ ] Provide useful formatters rather than raw JSON for core tools. — **90% · Evidenced.** Demonstrated in `imp-tui` app/views and session-lifecycle/rendering tests; preserve the contract and regression coverage.
- [ ] Navigate among tools, workers, workflow, diffs, and evidence through the sidebar. — **58% · Partial.** Partial support exists in `imp-tui` app/views and session-lifecycle/rendering tests; important end-to-end behavior or proof is missing.
- [ ] Avoid repetitive evidence paths and status messages in chat. — **82% · Strong.** Implemented and exercised through `imp-tui` app/views and session-lifecycle/rendering tests; remaining work is edge hardening or product proof.
- [ ] Make final evidence and verification easy to open. — **58% · Partial.** Partial support exists in `imp-tui` app/views and session-lifecycle/rendering tests; important end-to-end behavior or proof is missing.

## Sessions

- [ ] Create, rename, search, continue, and close sessions quickly. — **72% · Integrated.** Usable through `imp-tui` app/views and session-lifecycle/rendering tests, but not yet demonstrated at the full checklist standard.
- [ ] Navigate parent, child, and branch sessions. — **62% · Integrated.** Usable through `imp-tui` app/views and session-lifecycle/rendering tests, but not yet demonstrated at the full checklist standard.
- [ ] Preserve scroll, selection, draft, and sidebar state while switching. — **52% · Partial.** Partial support exists in `imp-tui` app/views and session-lifecycle/rendering tests; important end-to-end behavior or proof is missing.
- [ ] Show active and background status across sessions. — **40% · Partial.** Partial support exists in `imp-tui` app/views and session-lifecycle/rendering tests; important end-to-end behavior or proof is missing.
- [ ] Make recovery-required sessions obvious. — **35% · Scaffold.** Only foundations/scaffolding are present in `imp-tui` app/views and session-lifecycle/rendering tests; this is not an integrated product behavior.

## Terminal quality

- [ ] Work in narrow and wide terminals. — **82% · Strong.** Implemented and exercised through `imp-tui` app/views and session-lifecycle/rendering tests; remaining work is edge hardening or product proof.
- [ ] Preserve contrast and visible focus. — **84% · Strong.** Implemented and exercised through `imp-tui` app/views and session-lifecycle/rendering tests; remaining work is edge hardening or product proof.
- [ ] Support keyboard-only operation. — **90% · Evidenced.** Demonstrated in `imp-tui` app/views and session-lifecycle/rendering tests; preserve the contract and regression coverage.
- [ ] Avoid essential meaning conveyed only by color. — **82% · Strong.** Implemented and exercised through `imp-tui` app/views and session-lifecycle/rendering tests; remaining work is edge hardening or product proof.
- [ ] Handle resize, suspend/resume, paste, Unicode, and slow rendering. — **76% · Integrated.** Usable through `imp-tui` app/views and session-lifecycle/rendering tests, but not yet demonstrated at the full checklist standard.
- [ ] Restore terminal state after crash or panic. — **78% · Integrated.** Usable through `imp-tui` app/views and session-lifecycle/rendering tests, but not yet demonstrated at the full checklist standard.
- [ ] Test input latency and frame time under heavy streaming. — **28% · Scaffold.** Only foundations/scaffolding are present in `imp-tui` app/views and session-lifecycle/rendering tests; this is not an integrated product behavior.

---

# 6. Performance and resource discipline

**Class:** Core and Proof

“Written in Rust” is not a result. imp needs budgets and regression tests.

## Startup

- [ ] Measure cold and warm startup separately. — **50% · Partial.** Partial support exists in timing events and core hot-path benchmarks; important end-to-end behavior or proof is missing.
- [ ] Budget process start, config load, session readiness, auth, models, tools, and first render. — **28% · Scaffold.** Only foundations/scaffolding are present in timing events and core hot-path benchmarks; this is not an integrated product behavior.
- [ ] Keep optional heavy modules out of the default startup path. — **66% · Integrated.** Usable through timing events and core hot-path benchmarks, but not yet demonstrated at the full checklist standard.
- [ ] Avoid pre-work network requests unless explicitly required. — **85% · Strong.** Implemented and exercised through timing events and core hot-path benchmarks; remaining work is edge hardening or product proof.
- [ ] Bound extension discovery. — **72% · Integrated.** Usable through timing events and core hot-path benchmarks, but not yet demonstrated at the full checklist standard.

## Run latency

- [ ] Measure prompt-to-provider-request time. — **64% · Integrated.** Usable through timing events and core hot-path benchmarks, but not yet demonstrated at the full checklist standard.
- [ ] Measure request-to-first-event and first-text latency separately. — **60% · Integrated.** Usable through timing events and core hot-path benchmarks, but not yet demonstrated at the full checklist standard.
- [ ] Measure tool dispatch separately from execution. — **62% · Integrated.** Usable through timing events and core hot-path benchmarks, but not yet demonstrated at the full checklist standard.
- [ ] Measure post-tool context assembly and continuation latency. — **62% · Integrated.** Usable through timing events and core hot-path benchmarks, but not yet demonstrated at the full checklist standard.
- [ ] Measure cancellation acknowledgement and process termination. — **45% · Partial.** Partial support exists in timing events and core hot-path benchmarks; important end-to-end behavior or proof is missing.
- [ ] Attribute latency by trace stage, not only wall time. — **72% · Integrated.** Usable through timing events and core hot-path benchmarks, but not yet demonstrated at the full checklist standard.

## Repository scale

- [ ] Maintain small, medium, large, monorepo, and large-session fixtures. — **35% · Scaffold.** Only foundations/scaffolding are present in timing events and core hot-path benchmarks; this is not an integrated product behavior.
- [ ] Keep session listing and loading responsive at scale. — **68% · Integrated.** Usable through timing events and core hot-path benchmarks, but not yet demonstrated at the full checklist standard.
- [ ] Bound memory during reads, searches, diffs, and output. — **62% · Integrated.** Usable through timing events and core hot-path benchmarks, but not yet demonstrated at the full checklist standard.
- [ ] Avoid repeated full-repository or full-session scans on hot paths. — **64% · Integrated.** Usable through timing events and core hot-path benchmarks, but not yet demonstrated at the full checklist standard.
- [ ] Cache only with explicit tested invalidation. — **55% · Partial.** Partial support exists in timing events and core hot-path benchmarks; important end-to-end behavior or proof is missing.

## TUI performance

- [ ] Preserve responsive input during provider and subprocess output. — **78% · Integrated.** Usable through timing events and core hot-path benchmarks, but not yet demonstrated at the full checklist standard.
- [ ] Avoid reprocessing full transcripts on every delta. — **80% · Strong.** Implemented and exercised through timing events and core hot-path benchmarks; remaining work is edge hardening or product proof.
- [ ] Page or virtualize large histories where needed. — **58% · Partial.** Partial support exists in timing events and core hot-path benchmarks; important end-to-end behavior or proof is missing.
- [ ] Bound highlighting and wrapping costs. — **72% · Integrated.** Usable through timing events and core hot-path benchmarks, but not yet demonstrated at the full checklist standard.
- [ ] Detect scroll and visual instability during streaming. — **45% · Partial.** Partial support exists in timing events and core hot-path benchmarks; important end-to-end behavior or proof is missing.

## Gates

- [ ] Store benchmark baselines as versioned artifacts. — **32% · Scaffold.** Measurement hooks exist, but versioned budgets and CI regression enforcement do not.
- [ ] Define allowed regression percentages for critical paths. — **12% · Not started.** Measurement hooks exist, but versioned budgets and CI regression enforcement do not.
- [ ] Fail CI or require explicit approval for material regressions. — **8% · Not started.** Measurement hooks exist, but versioned budgets and CI regression enforcement do not.
- [ ] Report median and tail latency. — **45% · Partial.** Measurement hooks exist, but versioned budgets and CI regression enforcement do not.
- [ ] Separate provider latency from imp overhead. — **72% · Integrated.** Measurement hooks exist, but versioned budgets and CI regression enforcement do not.
- [ ] Track binary size, idle memory, peak memory, and subprocess leakage. — **18% · Not started.** Measurement hooks exist, but versioned budgets and CI regression enforcement do not.

---

# 7. Reliability, cancellation, and recovery

**Class:** Core

## Durable writes

- [ ] Use atomic replacement for critical single-file state. — **46% · Partial.** Workflow YAML uses temporary-file rename, but sessions and several artifacts append/write directly without a uniform atomic replacement contract.
- [ ] Flush and validate records at defined durable boundaries. — **42% · Partial.** JSON serialization and reopen/recovery tests exist, but session append does not explicitly flush or fsync at declared durability boundaries.
- [ ] Detect truncated session, workflow, evidence, and index files. — **62% · Integrated.** Usable through recovery checkpoints, session recovery tests, retry logic, and process handling, but not yet demonstrated at the full checklist standard.
- [ ] Rebuild derived indexes from source-of-truth records. — **55% · Partial.** Partial support exists in recovery checkpoints, session recovery tests, retry logic, and process handling; important end-to-end behavior or proof is missing.
- [ ] Version durable schemas and test migrations. — **72% · Integrated.** Usable through recovery checkpoints, session recovery tests, retry logic, and process handling, but not yet demonstrated at the full checklist standard.
- [ ] Never silently discard unknown durable records. — **65% · Integrated.** Usable through recovery checkpoints, session recovery tests, retry logic, and process handling, but not yet demonstrated at the full checklist standard.

## Cancellation

- [ ] Distinguish requested, acknowledged, tool-stopped, provider-stopped, and run-ended states. — **42% · Partial.** Partial support exists in recovery checkpoints, session recovery tests, retry logic, and process handling; important end-to-end behavior or proof is missing.
- [ ] Cancel provider streams and owned process trees promptly. — **76% · Integrated.** Usable through recovery checkpoints, session recovery tests, retry logic, and process handling, but not yet demonstrated at the full checklist standard.
- [ ] Propagate cancellation to bounded workers. — **30% · Scaffold.** Only foundations/scaffolding are present in recovery checkpoints, session recovery tests, retry logic, and process handling; this is not an integrated product behavior.
- [ ] Preserve completed side effects and evidence. — **72% · Integrated.** Usable through recovery checkpoints, session recovery tests, retry logic, and process handling, but not yet demonstrated at the full checklist standard.
- [ ] Do not report cancellation ambiguously as failure or success. — **82% · Strong.** Implemented and exercised through recovery checkpoints, session recovery tests, retry logic, and process handling; remaining work is edge hardening or product proof.
- [ ] Allow safe follow-up after cancellation. — **76% · Integrated.** Usable through recovery checkpoints, session recovery tests, retry logic, and process handling, but not yet demonstrated at the full checklist standard.

## Recovery

- [ ] Record provider request start and completion boundaries. — **82% · Strong.** Implemented and exercised through recovery checkpoints, session recovery tests, retry logic, and process handling; remaining work is edge hardening or product proof.
- [ ] Record assistant tool-call observation before execution. — **90% · Evidenced.** Demonstrated in recovery checkpoints, session recovery tests, retry logic, and process handling; preserve the contract and regression coverage.
- [ ] Record tool plan, execution start, execution end, and result-context boundaries. — **90% · Evidenced.** Demonstrated in recovery checkpoints, session recovery tests, retry logic, and process handling; preserve the contract and regression coverage.
- [ ] Classify interruption as safe to resume, safe to retry, completed, or requiring review. — **82% · Strong.** Implemented and exercised through recovery checkpoints, session recovery tests, retry logic, and process handling; remaining work is edge hardening or product proof.
- [ ] Refuse ambiguous retries that may duplicate side effects. — **88% · Strong.** Implemented and exercised through recovery checkpoints, session recovery tests, retry logic, and process handling; remaining work is edge hardening or product proof.
- [ ] Reconcile transcript, filesystem, git, workflow, and evidence before resuming. — **52% · Partial.** Partial support exists in recovery checkpoints, session recovery tests, retry logic, and process handling; important end-to-end behavior or proof is missing.
- [ ] Present recovery state and choices clearly in TUI and machine output. — **38% · Scaffold.** Only foundations/scaffolding are present in recovery checkpoints, session recovery tests, retry logic, and process handling; this is not an integrated product behavior.

## Failure handling

- [ ] Normalize provider errors into retryable, auth, quota, invalid-request, and terminal classes. — **78% · Integrated.** Usable through recovery checkpoints, session recovery tests, retry logic, and process handling, but not yet demonstrated at the full checklist standard.
- [ ] Use bounded retries with visible backoff. — **86% · Strong.** Implemented and exercised through recovery checkpoints, session recovery tests, retry logic, and process handling; remaining work is edge hardening or product proof.
- [ ] Avoid retry storms across workers. — **30% · Scaffold.** Only foundations/scaffolding are present in recovery checkpoints, session recovery tests, retry logic, and process handling; this is not an integrated product behavior.
- [ ] Isolate extension, browser, MCP, and hook failure from the core runtime. — **55% · Partial.** Lua, hooks, and browser subprocesses have bounded error paths; MCP is a placeholder and experimental extension code still adds core coupling.
- [ ] Preserve diagnostics without leaking secrets. — **84% · Strong.** Implemented and exercised through recovery checkpoints, session recovery tests, retry logic, and process handling; remaining work is edge hardening or product proof.
- [ ] Clean up subprocesses, temporary files, locks, and worktrees. — **62% · Integrated.** Usable through recovery checkpoints, session recovery tests, retry logic, and process handling, but not yet demonstrated at the full checklist standard.

## Proof

- [ ] Inject termination at every durable checkpoint. — **20% · Scaffold.** Some fault tests exist, but this systematic fault-injection/soak matrix is not present.
- [ ] Test disk-full, permission-denied, malformed-state, provider-drop, process-hang, and terminal-close cases. — **28% · Scaffold.** Some fault tests exist, but this systematic fault-injection/soak matrix is not present.
- [ ] Run repeated crash/restart sequences against the same session and workflow. — **25% · Scaffold.** Only foundations/scaffolding are present in recovery checkpoints, session recovery tests, retry logic, and process handling; this is not an integrated product behavior.
- [ ] Soak concurrent sessions and workers. — **15% · Not started.** Some fault tests exist, but this systematic fault-injection/soak matrix is not present.
- [ ] Track successful and ambiguous recovery rates. — **15% · Not started.** No reproducible product-level evidence is recorded; treat this as not started.

---

# 8. Policy and autonomy

**Class:** Core and Control

Policy must provide precise control without turning local development into paperwork.

## Effective policy

- [ ] Define precedence among agent mode, role, run policy, autonomy, configs, hooks, and tool metadata. — **76% · Integrated.** Usable through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests, but not yet demonstrated at the full checklist standard.
- [ ] Make deny rules authoritative over broad allows. — **90% · Evidenced.** Demonstrated in run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; preserve the contract and regression coverage.
- [ ] Keep safe mode compatible and unsurprising. — **86% · Strong.** Implemented and exercised through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; remaining work is edge hardening or product proof.
- [ ] Keep allow-all modes auditable and subject to hard rails. — **82% · Strong.** Implemented and exercised through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; remaining work is edge hardening or product proof.
- [ ] Apply identical checks through TUI, CLI, RPC, ACP, workflows, workers, extensions, and tools. — **52% · Partial.** Partial support exists in run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; important end-to-end behavior or proof is missing.
- [ ] Prevent extensions and workers from self-authorizing. — **82% · Strong.** Implemented and exercised through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; remaining work is edge hardening or product proof.

## Resource scopes

- [ ] Classify workspace, worktree, outside-workspace, network-read, network-mutate, secret-use, secret-reveal, system, and production. — **86% · Strong.** Implemented and exercised through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; remaining work is edge hardening or product proof.
- [ ] Resolve paths canonically before decisions. — **82% · Strong.** Implemented and exercised through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; remaining work is edge hardening or product proof.
- [ ] Keep outside-workspace writes denied without specific grants. — **88% · Strong.** Implemented and exercised through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; remaining work is edge hardening or product proof.
- [ ] Keep credential, publish, deploy, billing, DNS, and production changes behind dedicated capabilities. — **78% · Integrated.** Usable through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests, but not yet demonstrated at the full checklist standard.
- [ ] Distinguish read-only network access from mutation. — **88% · Strong.** Implemented and exercised through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; remaining work is edge hardening or product proof.

## User understanding

- [ ] Explain effective policy before a run. — **25% · Scaffold.** Only foundations/scaffolding are present in run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; this is not an integrated product behavior.
- [ ] Show autonomy and write scope in the TUI. — **62% · Integrated.** Usable through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests, but not yet demonstrated at the full checklist standard.
- [ ] Explain allow, deny, ask, sandbox-only, dry-run-only, and verification-required decisions. — **58% · Partial.** Partial support exists in run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; important end-to-end behavior or proof is missing.
- [ ] Include tool, resource, matched rule, reason, and scope in prompts. — **60% · Integrated.** Usable through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests, but not yet demonstrated at the full checklist standard.
- [ ] Support scoped one-time and run-limited approvals. — **72% · Integrated.** Usable through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests, but not yet demonstrated at the full checklist standard.
- [ ] Make headless/CI behavior fail closed instead of hanging. — **84% · Strong.** Implemented and exercised through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; remaining work is edge hardening or product proof.

## Secrets

- [ ] Keep values in OS-backed credential storage and metadata separate. — **92% · Evidenced.** Demonstrated in run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; preserve the contract and regression coverage.
- [ ] Mediate secret use without model-context exposure. — **82% · Strong.** Implemented and exercised through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; remaining work is edge hardening or product proof.
- [ ] Redact secrets from logs, traces, evidence, summaries, extension payloads, and requests. — **82% · Strong.** Implemented and exercised through run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; remaining work is edge hardening or product proof.
- [ ] Test known, encoded, fragmented, and accidental disclosure paths. — **35% · Scaffold.** Only foundations/scaffolding are present in run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; this is not an integrated product behavior.

## Proof

- [ ] Maintain a decision matrix for every autonomy mode and resource scope. — **55% · Partial.** Partial support exists in run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; important end-to-end behavior or proof is missing.
- [ ] Test equivalent actions through every transport. — **30% · Scaffold.** Only foundations/scaffolding are present in run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; this is not an integrated product behavior.
- [ ] Test path traversal, symlinks, shell indirection, extension bypass, and worker inheritance. — **55% · Partial.** Partial support exists in run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; important end-to-end behavior or proof is missing.
- [ ] Treat policy bypass as a release blocker. — **45% · Partial.** Partial support exists in run policy, ReferenceMonitor, autonomy/hard-rail logic, and policy tests; important end-to-end behavior or proof is missing.

---

# 9. Verification and evidence

**Class:** Core and Proof

Evidence should increase trust without making normal conversation noisy.

## Verification

- [ ] Attach required verification before closeout. — **88% · Strong.** Implemented and exercised through verification gates/runner, trace events, evidence packets, and closeout tests; remaining work is edge hardening or product proof.
- [ ] Run the narrowest meaningful checks first. — **78% · Integrated.** Usable through verification gates/runner, trace events, evidence packets, and closeout tests, but not yet demonstrated at the full checklist standard.
- [ ] Expand when risk or repository instructions require it. — **72% · Integrated.** Usable through verification gates/runner, trace events, evidence packets, and closeout tests, but not yet demonstrated at the full checklist standard.
- [ ] Record command, cwd, exit status, duration, and bounded output. — **88% · Strong.** Implemented and exercised through verification gates/runner, trace events, evidence packets, and closeout tests; remaining work is edge hardening or product proof.
- [ ] Distinguish not run, skipped, failed, and unavailable. — **84% · Strong.** Implemented and exercised through verification gates/runner, trace events, evidence packets, and closeout tests; remaining work is edge hardening or product proof.
- [ ] Invalidate verification after relevant later edits. — **45% · Partial.** Partial support exists in verification gates/runner, trace events, evidence packets, and closeout tests; important end-to-end behavior or proof is missing.
- [ ] Require re-verification after invalidation. — **42% · Partial.** Partial support exists in verification gates/runner, trace events, evidence packets, and closeout tests; important end-to-end behavior or proof is missing.
- [ ] Support command, artifact, manual-review, and aggregate checks. — **78% · Integrated.** Usable through verification gates/runner, trace events, evidence packets, and closeout tests, but not yet demonstrated at the full checklist standard.

## Evidence model

- [ ] Keep versioned JSONL as source of truth. — **55% · Partial.** Partial support exists in verification gates/runner, trace events, evidence packets, and closeout tests; important end-to-end behavior or proof is missing.
- [ ] Record run, turn, message, tool, policy, workflow, worker, verification, usage, cost, diff, and recovery references. — **68% · Integrated.** Usable through verification gates/runner, trace events, evidence packets, and closeout tests, but not yet demonstrated at the full checklist standard.
- [ ] Keep evidence append-only where practical. — **72% · Integrated.** Usable through verification gates/runner, trace events, evidence packets, and closeout tests, but not yet demonstrated at the full checklist standard.
- [ ] Bound payloads and store large artifacts by reference. — **86% · Strong.** Implemented and exercised through verification gates/runner, trace events, evidence packets, and closeout tests; remaining work is edge hardening or product proof.
- [ ] Make indexes rebuildable. — **55% · Partial.** Partial support exists in verification gates/runner, trace events, evidence packets, and closeout tests; important end-to-end behavior or proof is missing.
- [ ] Preserve causal ordering across workers. — **28% · Scaffold.** Only foundations/scaffolding are present in verification gates/runner, trace events, evidence packets, and closeout tests; this is not an integrated product behavior.
- [ ] Include autonomy and effective scope. — **76% · Integrated.** Usable through verification gates/runner, trace events, evidence packets, and closeout tests, but not yet demonstrated at the full checklist standard.

## Human report

- [ ] Generate a concise report from structured evidence. — **78% · Integrated.** Usable through verification gates/runner, trace events, evidence packets, and closeout tests, but not yet demonstrated at the full checklist standard.
- [ ] Include outcome, scope, changed files, decisions, checks, concerns, policy exceptions, workers, recovery, usage, and cost. — **72% · Integrated.** Usable through verification gates/runner, trace events, evidence packets, and closeout tests, but not yet demonstrated at the full checklist standard.
- [ ] Link claims to events or artifacts. — **68% · Integrated.** Usable through verification gates/runner, trace events, evidence packets, and closeout tests, but not yet demonstrated at the full checklist standard.
- [ ] Never imply verification beyond what evidence proves. — **86% · Strong.** Implemented and exercised through verification gates/runner, trace events, evidence packets, and closeout tests; remaining work is edge hardening or product proof.
- [ ] Keep final chat concise while deeper evidence remains easy to inspect. — **82% · Strong.** Implemented and exercised through verification gates/runner, trace events, evidence packets, and closeout tests; remaining work is edge hardening or product proof.

## Proof

- [ ] Validate evidence schemas and references. — **42% · Partial.** Evidence artifacts exist, but schema/reference validation and replay-only proof are incomplete.
- [ ] Regenerate reports from JSONL alone. — **28% · Scaffold.** Evidence artifacts exist, but schema/reference validation and replay-only proof are incomplete.
- [ ] Test concurrent ordering and interrupted evidence writes. — **25% · Scaffold.** Evidence artifacts exist, but schema/reference validation and replay-only proof are incomplete.
- [ ] Compare report claims against git diff and check results. — **35% · Scaffold.** Only foundations/scaffolding are present in verification gates/runner, trace events, evidence packets, and closeout tests; this is not an integrated product behavior.

---

# 10. Session control and experimentation

**Class:** Core and Control

## Lifecycle

- [ ] Create, continue, list, search, inspect, rename, archive, and close sessions. — **72% · Integrated.** Create/list/search/continue/rename exist; archive and close lifecycle are not complete as one surface.
- [ ] Keep identity stable across restart. — **90% · Evidenced.** Demonstrated in session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; preserve the contract and regression coverage.
- [ ] Record project, cwd, model, role, branch, workflow, and usage. — **78% · Integrated.** Usable through session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests, but not yet demonstrated at the full checklist standard.
- [ ] Handle moved or unavailable paths explicitly. — **52% · Partial.** Partial support exists in session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; important end-to-end behavior or proof is missing.
- [ ] Keep session listing fast at realistic scale. — **58% · Partial.** Partial support exists in session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; important end-to-end behavior or proof is missing.

## Steering and delivery

- [ ] Admit input durably before execution. — **52% · Partial.** Partial support exists in session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; important end-to-end behavior or proof is missing.
- [ ] Distinguish steer from queued follow-up. — **82% · Strong.** Implemented and exercised through session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; remaining work is edge hardening or product proof.
- [ ] Promote steering at a safe provider-turn boundary. — **78% · Integrated.** Usable through session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests, but not yet demonstrated at the full checklist standard.
- [ ] Keep queued work pending until active work would become idle. — **80% · Strong.** Implemented and exercised through session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; remaining work is edge hardening or product proof.
- [ ] Reconcile exact retry without duplicate admission. — **55% · Partial.** Partial support exists in session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; important end-to-end behavior or proof is missing.
- [ ] Reject conflicting reuse of durable message identity. — **30% · Scaffold.** Only foundations/scaffolding are present in session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; this is not an integrated product behavior.
- [ ] Make pending inputs visible and controllable. — **60% · Integrated.** Usable through session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests, but not yet demonstrated at the full checklist standard.

## Branching and comparison

- [ ] Branch from a selected durable message boundary. — **82% · Strong.** Implemented and exercised through session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; remaining work is edge hardening or product proof.
- [ ] Preserve parent/child and branch metadata. — **86% · Strong.** Implemented and exercised through session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; remaining work is edge hardening or product proof.
- [ ] Allow branches to use different models, roles, or strategies. — **62% · Integrated.** Usable through session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests, but not yet demonstrated at the full checklist standard.
- [ ] Compare outcomes, diffs, checks, cost, and evidence. — **32% · Scaffold.** Only foundations/scaffolding are present in session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; this is not an integrated product behavior.
- [ ] Never imply transcript branching reverts filesystem effects. — **80% · Strong.** Implemented and exercised through session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; remaining work is edge hardening or product proof.
- [ ] Provide explicit verified filesystem or git rollback when requested. — **72% · Integrated.** Usable through session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests, but not yet demonstrated at the full checklist standard.

## Resume and replay

- [ ] Resume from authoritative durable state. — **82% · Strong.** Implemented and exercised through session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; remaining work is edge hardening or product proof.
- [ ] Reload projected history before continuation. — **90% · Evidenced.** Demonstrated in session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; preserve the contract and regression coverage.
- [ ] Make retry and continuation semantics explicit. — **72% · Integrated.** Usable through session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests, but not yet demonstrated at the full checklist standard.
- [ ] Prevent duplicate tool effects on replay. — **68% · Integrated.** Usable through session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests, but not yet demonstrated at the full checklist standard.
- [ ] Surface sessions requiring recovery review. — **35% · Scaffold.** Only foundations/scaffolding are present in session JSONL, branching/forking, ImpSession controls, RPC/TUI session tests; this is not an integrated product behavior.

---

# 11. Provider depth

**Class:** Core and Enabler

imp does not need every provider. Supported providers must work extremely well.

## Curated support

- [ ] Define a small primary tier with explicit compatibility guarantees. — **35% · Scaffold.** Only foundations/scaffolding are present in `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; this is not an integrated product behavior.
- [ ] Define a secondary tier for compatible and community-tested providers. — **30% · Scaffold.** Only foundations/scaffolding are present in `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; this is not an integrated product behavior.
- [ ] Document partial capabilities per provider and model. — **45% · Partial.** Partial support exists in `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; important end-to-end behavior or proof is missing.
- [ ] Update models, limits, pricing, reasoning controls, and deprecations promptly. — **76% · Integrated.** Usable through `imp-llm` adapters, auth/model registry, and provider unit/recorded tests, but not yet demonstrated at the full checklist standard.

## Streaming and tools

- [ ] Normalize text, thinking, tools, usage, finish reasons, and errors. — **90% · Evidenced.** Demonstrated in `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; preserve the contract and regression coverage.
- [ ] Preserve one intended stream call per provider turn. — **88% · Strong.** Implemented and exercised through `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; remaining work is edge hardening or product proof.
- [ ] Handle partial tool arguments and malformed events safely. — **82% · Strong.** Implemented and exercised through `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; remaining work is edge hardening or product proof.
- [ ] Test parallel and sequential tool behavior where supported. — **72% · Integrated.** Usable through `imp-llm` adapters, auth/model registry, and provider unit/recorded tests, but not yet demonstrated at the full checklist standard.
- [ ] Keep cancellation and timeouts consistent. — **72% · Integrated.** Provider and process paths have cancellation/timeout handling, but cross-provider behavior is not covered by one conformance suite.
- [ ] Verify prompt caching and accounting. — **72% · Integrated.** Usable through `imp-llm` adapters, auth/model registry, and provider unit/recorded tests, but not yet demonstrated at the full checklist standard.

## Authentication

- [ ] Make key and OAuth setup direct and diagnosable. — **88% · Strong.** Implemented and exercised through `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; remaining work is edge hardening or product proof.
- [ ] Refresh OAuth safely. — **86% · Strong.** Implemented and exercised through `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; remaining work is edge hardening or product proof.
- [ ] Import external credentials only with explicit documented behavior. — **72% · Integrated.** Usable through `imp-llm` adapters, auth/model registry, and provider unit/recorded tests, but not yet demonstrated at the full checklist standard.
- [ ] Provide login, list, show, and doctor without revealing values. — **88% · Strong.** Implemented and exercised through `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; remaining work is edge hardening or product proof.
- [ ] Distinguish missing, expired, rejected, and misconfigured credentials. — **78% · Integrated.** Usable through `imp-llm` adapters, auth/model registry, and provider unit/recorded tests, but not yet demonstrated at the full checklist standard.

## Model selection

- [ ] Make switching provider, model, reasoning, and role quick. — **82% · Strong.** Implemented and exercised through `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; remaining work is edge hardening or product proof.
- [ ] Preserve or explain compatibility changes during session switches. — **52% · Partial.** Partial support exists in `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; important end-to-end behavior or proof is missing.
- [ ] Warn when a model lacks required tools, context, image, or reasoning support. — **58% · Partial.** Partial support exists in `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; important end-to-end behavior or proof is missing.
- [ ] Use explicit defaults and show their source. — **52% · Partial.** Partial support exists in `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; important end-to-end behavior or proof is missing.

## Proof

- [ ] Maintain live or recorded contract tests for the primary tier. — **55% · Partial.** Partial support exists in `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; important end-to-end behavior or proof is missing.
- [ ] Track provider-specific failures. — **45% · Partial.** Partial support exists in `imp-llm` adapters, auth/model registry, and provider unit/recorded tests; important end-to-end behavior or proof is missing.
- [ ] Test long streams, cancellation, tools, reasoning, caching, and quota errors. — **62% · Integrated.** Usable through `imp-llm` adapters, auth/model registry, and provider unit/recorded tests, but not yet demonstrated at the full checklist standard.
- [ ] Publish tiers rather than implying equal adapter quality. — **20% · Scaffold.** Provider adapters exist, but support tiers and capability guarantees are not published.

---

# 12. Configuration and diagnostics

**Class:** Control

Power is only useful when users can predict it.

## Configuration

- [ ] Document precedence across defaults, user/project config, environment, CLI, roles, workflows, and session overrides. — **62% · Integrated.** Usable through typed config merge, settings/setup, secrets doctor, and browser doctor, but not yet demonstrated at the full checklist standard.
- [ ] Use typed values and reject invalid fields. — **82% · Strong.** Implemented and exercised through typed config merge, settings/setup, secrets doctor, and browser doctor; remaining work is edge hardening or product proof.
- [ ] Keep one canonical name per concept. — **62% · Integrated.** Usable through typed config merge, settings/setup, secrets doctor, and browser doctor, but not yet demonstrated at the full checklist standard.
- [ ] Avoid duplicate settings controlling the same behavior. — **55% · Partial.** Partial support exists in typed config merge, settings/setup, secrets doctor, and browser doctor; important end-to-end behavior or proof is missing.
- [ ] Preserve compatibility intentionally with visible deprecation. — **62% · Integrated.** Usable through typed config merge, settings/setup, secrets doctor, and browser doctor, but not yet demonstrated at the full checklist standard.
- [ ] Keep secrets out of normal config. — **90% · Evidenced.** Demonstrated in typed config merge, settings/setup, secrets doctor, and browser doctor; preserve the contract and regression coverage.

## Explain and doctor

- [ ] Show every loaded config source in precedence order. — **20% · Scaffold.** Config is typed and merged, but no complete effective-config provenance/explain command exists.
- [ ] Show effective provider, model, role, autonomy, tools, write/network scope, hooks, extensions, and gates. — **45% · Partial.** Partial support exists in typed config merge, settings/setup, secrets doctor, and browser doctor; important end-to-end behavior or proof is missing.
- [ ] Explain why a value won. — **20% · Scaffold.** Config is typed and merged, but no complete effective-config provenance/explain command exists.
- [ ] Diagnose credentials, browser, MCP, extensions, sessions, worktrees, and executables. — **48% · Partial.** Secrets and browser doctors are real; MCP is a placeholder and there is no unified doctor for extensions, sessions, worktrees, and executables.
- [ ] Offer actionable fixes rather than generic errors. — **72% · Integrated.** Usable through typed config merge, settings/setup, secrets doctor, and browser doctor, but not yet demonstrated at the full checklist standard.
- [ ] Support machine-readable diagnostics. — **58% · Partial.** Partial support exists in typed config merge, settings/setup, secrets doctor, and browser doctor; important end-to-end behavior or proof is missing.

## Defaults

- [ ] Keep the default fast and conservative without bureaucracy. — **82% · Strong.** Implemented and exercised through typed config merge, settings/setup, secrets doctor, and browser doctor; remaining work is edge hardening or product proof.
- [ ] Keep optional systems lazy and disabled until needed. — **74% · Integrated.** Usable through typed config merge, settings/setup, secrets doctor, and browser doctor, but not yet demonstrated at the full checklist standard.
- [ ] Let a first run complete useful work without architecture knowledge. — **82% · Strong.** Implemented and exercised through typed config merge, settings/setup, secrets doctor, and browser doctor; remaining work is edge hardening or product proof.
- [ ] Avoid silent behavior changes when optional dependencies appear. — **72% · Integrated.** Usable through typed config merge, settings/setup, secrets doctor, and browser doctor, but not yet demonstrated at the full checklist standard.

---

# 13. Headless and editor operation

**Class:** Enabler

imp does not need a general application platform. It needs clean access to its core engine.

## One-shot and machine output

- [ ] Keep one-shot behavior equivalent to TUI runtime where interaction is unnecessary. — **84% · Strong.** Implemented and exercised through `imp-cli` one-shot/RPC plus the ACP protocol scaffold and tests; remaining work is edge hardening or product proof.
- [ ] Provide stable text, clean-text, JSON, and JSONL contracts. — **88% · Strong.** Implemented and exercised through `imp-cli` one-shot/RPC plus the ACP protocol scaffold and tests; remaining work is edge hardening or product proof.
- [ ] Include final status, policy violations, tools, verification, metrics, usage, cost, and evidence references. — **78% · Integrated.** Usable through `imp-cli` one-shot/RPC plus the ACP protocol scaffold and tests, but not yet demonstrated at the full checklist standard.
- [ ] Send output and diagnostics to predictable streams. — **88% · Strong.** Implemented and exercised through `imp-cli` one-shot/RPC plus the ACP protocol scaffold and tests; remaining work is edge hardening or product proof.
- [ ] Return meaningful exit codes. — **72% · Integrated.** Usable through `imp-cli` one-shot/RPC plus the ACP protocol scaffold and tests, but not yet demonstrated at the full checklist standard.
- [ ] Fail closed when required input is unavailable. — **86% · Strong.** Headless UI requests and approval-dependent actions fail closed; remaining gaps are transport-specific conformance proof.

## JSONL RPC

- [ ] Version commands and events. — **45% · Partial.** Partial support exists in `imp-cli` one-shot/RPC plus the ACP protocol scaffold and tests; important end-to-end behavior or proof is missing.
- [ ] Support prompt, cancel, steer, follow-up, questions, permissions, status, and closeout. — **50% · Partial.** RPC implements prompt/cancel/steer/follow-up; questions, permission replies, status queries, and closeout commands are not complete protocol inputs.
- [ ] Preserve ordering and correlation IDs. — **58% · Partial.** Partial support exists in `imp-cli` one-shot/RPC plus the ACP protocol scaffold and tests; important end-to-end behavior or proof is missing.
- [ ] Apply backpressure and bound queues. — **72% · Integrated.** Usable through `imp-cli` one-shot/RPC plus the ACP protocol scaffold and tests, but not yet demonstrated at the full checklist standard.
- [ ] Prevent disconnect from corrupting active sessions. — **48% · Partial.** Partial support exists in `imp-cli` one-shot/RPC plus the ACP protocol scaffold and tests; important end-to-end behavior or proof is missing.
- [ ] Document compatibility guarantees. — **45% · Partial.** Partial support exists in `imp-cli` one-shot/RPC plus the ACP protocol scaffold and tests; important end-to-end behavior or proof is missing.

## ACP

- [ ] Support new, load, resume, prompt, cancel, and close reliably. — **55% · Partial.** Session protocol/lifecycle is implemented, but prompts are stubbed and do not run the shared live agent path.
- [ ] Translate messages, tools, updates, and stop reasons without semantic loss. — **40% · Partial.** Session protocol/lifecycle is implemented, but prompts are stubbed and do not run the shared live agent path.
- [ ] Advertise capabilities accurately. — **82% · Strong.** Session protocol/lifecycle is implemented, but prompts are stubbed and do not run the shared live agent path.
- [ ] Preserve instructions, policy, tools, MCP, and durable sessions. — **25% · Scaffold.** Session protocol/lifecycle is implemented, but prompts are stubbed and do not run the shared live agent path.
- [ ] Test against at least one real editor. — **10% · Not started.** Session protocol/lifecycle is implemented, but prompts are stubbed and do not run the shared live agent path.
- [ ] Keep editor and TUI behavior on the same runtime path. — **15% · Not started.** Session protocol/lifecycle is implemented, but prompts are stubbed and do not run the shared live agent path.

## Deliberate limit

- [ ] Add HTTP, SDKs, or clients only when a concrete high-value workflow cannot be served cleanly by TUI, one-shot, RPC, or ACP. — **70% · Integrated.** The runtime remains terminal/local-first, but the preview SDK and experimental GUI mean this boundary is policy rather than a proven gate.

---

# 14. Evaluation and competitive proof

**Class:** Proof

The sports-car claim must be measurable.

## Benchmark suite

- [ ] Cover fixes, cross-module changes, refactors, migrations, debugging, review, and research. — **40% · Partial.** Eight pinned Dirac-derived refactor tasks cover several repositories and change shapes, but debugging, review, research, and scored imp runs are not represented.
- [ ] Include long workflow and parallel-worker tasks. — **10% · Not started.** No committed evaluation task exercises a durable multi-step workflow or real parallel-worker integration.
- [ ] Include large repositories and long sessions. — **20% · Scaffold.** Dirac-derived tasks target large upstream repositories, but no committed imp result or long-session fixture demonstrates this criterion.
- [ ] Include interrupted provider, tool, mutation, worker, and verification phases. — **10% · Not started.** No committed evaluation fixture injects interruption across these runtime phases.
- [ ] Include ambiguity, conflicts, and policy denials. — **10% · Not started.** The current evaluation task catalog does not deliberately exercise ambiguity, merge conflicts, or policy denial behavior.
- [ ] Include tasks where no code change is correct. — **0% · Not started.** No committed evaluation task has a no-change acceptance outcome.
- [ ] Keep held-out tasks to detect overfitting. — **0% · Not started.** The committed task catalogs do not define a held-out split.

## Metrics

- [ ] Completion correctness and acceptance satisfaction. — **35% · Scaffold.** The Dirac harness records verifier pass/fail, but several verifiers remain unresolved and no completed imp baseline results are committed.
- [ ] Verification pass and false-completion rates. — **30% · Scaffold.** Per-run verifier state is modeled, but no committed result set calculates verification or false-completion rates.
- [ ] Wall time and imp-only overhead. — **10% · Not started.** The external harness does not record duration or isolate imp overhead in committed results.
- [ ] Time to first useful action. — **0% · Not started.** No current evaluation artifact records time to first useful action.
- [ ] Turns, tools, failures, and repeated calls. — **20% · Scaffold.** Transcripts can preserve raw behavior, but the harness does not emit structured turn, tool, failure, or repetition metrics.
- [ ] Input, output, cache, and effective tokens. — **10% · Not started.** The result schema contains nullable usage fields, but committed results do not contain token or cache measurements.
- [ ] Cost. — **10% · Not started.** The result schema contains nullable cost, but no committed scored result provides cost evidence.
- [ ] Files and lines read versus changed. — **20% · Scaffold.** Diffs and imported reference metadata capture changed paths and lines, but files read and imp run deltas are not measured end to end.
- [ ] User interventions and approvals. — **0% · Not started.** No current evaluation result records user interventions or approval decisions.
- [ ] Recovery success. — **0% · Not started.** No committed evaluation task or aggregate measures recovery success.
- [ ] Worker conflicts and integration success. — **0% · Not started.** No committed evaluation task executes real parallel workers or measures integration conflicts.

## Comparison discipline

- [ ] Compare with OpenCode and relevant agents using matched model, repository, prompt, permissions, and limits. — **20% · Scaffold.** Imported Dirac reference patches provide an external comparison input, but no matched multi-agent run matrix is committed.
- [ ] Separate model quality from runtime quality where possible. — **10% · Not started.** The current harness does not run controlled model/runtime ablations.
- [ ] Record exact versions and configuration. — **50% · Partial.** Task specs pin source commits and result records include provider/model fields, but no completed baseline records the full agent/runtime configuration.
- [ ] Publish failures and tradeoffs, not only wins. — **10% · Not started.** No committed real run corpus publishes failures and tradeoffs.
- [ ] Avoid aggregates that hide catastrophic failure classes. — **20% · Scaffold.** The harness preserves per-run artifacts, but no aggregate or failure-class reporting is implemented.
- [ ] Use repeated trials for nondeterministic tasks. — **0% · Not started.** The current harness runs one task at a time and has no repeated-trial orchestration.

## Regression process

- [ ] Run narrow suites on relevant changes and full suites before releases. — **10% · Not started.** The one-task Dirac runner supports manual narrow runs, but no CI or release suite policy is enforced.
- [ ] Store artifacts for failures and material changes. — **35% · Scaffold.** The harness defines prompt, transcript, diff, verifier, and result artifacts; only a dry-run placeholder is committed.
- [ ] Require explanation for correctness, performance, or token regressions. — **0% · Not started.** No current gate requires regression explanations.
- [ ] Turn important real failures into fixtures. — **20% · Scaffold.** Eval candidates and task fixtures exist separately, but there is no demonstrated promotion workflow from a real failure.

---

# 15. Release quality and installation

**Class:** Enabler

imp does not need every package manager. It needs a dependable path to a working agent.

## Installation

- [ ] Provide reliable binaries for chosen platforms. — **82% · Strong.** Implemented and exercised through release/edge workflows, Homebrew generation, install-local, and smoke coverage; remaining work is edge hardening or product proof.
- [ ] Publish checksums and signatures or provenance. — **58% · Partial.** Release artifacts feed SHA-256 values into the generated Homebrew formula, but standalone checksums, signatures, and build provenance are not published.
- [ ] Keep Homebrew and source installation tested. — **80% · Strong.** Implemented and exercised through release/edge workflows, Homebrew generation, install-local, and smoke coverage; remaining work is edge hardening or product proof.
- [ ] Provide direct upgrade and version reporting. — **72% · Integrated.** Usable through release/edge workflows, Homebrew generation, install-local, and smoke coverage, but not yet demonstrated at the full checklist standard.
- [ ] Avoid requiring Rust for ordinary binary users. — **90% · Evidenced.** Demonstrated in release/edge workflows, Homebrew generation, install-local, and smoke coverage; preserve the contract and regression coverage.
- [ ] Diagnose missing credentials and optional browser dependencies on first start. — **80% · Strong.** Implemented and exercised through release/edge workflows, Homebrew generation, install-local, and smoke coverage; remaining work is edge hardening or product proof.

## Compatibility

- [ ] Define supported OSes, architectures, terminals, shells, and toolchains. — **45% · Partial.** Partial support exists in release/edge workflows, Homebrew generation, install-local, and smoke coverage; important end-to-end behavior or proof is missing.
- [ ] Test paths, processes, credential stores, and terminals on each platform. — **38% · Scaffold.** Only foundations/scaffolding are present in release/edge workflows, Homebrew generation, install-local, and smoke coverage; this is not an integrated product behavior.
- [ ] Mark unsupported environments honestly. — **42% · Partial.** Partial support exists in release/edge workflows, Homebrew generation, install-local, and smoke coverage; important end-to-end behavior or proof is missing.
- [ ] Test session and workflow migrations across releases. — **30% · Scaffold.** Only foundations/scaffolding are present in release/edge workflows, Homebrew generation, install-local, and smoke coverage; this is not an integrated product behavior.

## Release gates

- [ ] TUI startup and prompt smoke test. — **40% · Partial.** Release packaging exists, but there is no comprehensive PR/release quality-gate workflow.
- [ ] One-shot text and JSON/JSONL smoke tests. — **45% · Partial.** Release packaging exists, but there is no comprehensive PR/release quality-gate workflow.
- [ ] RPC prompt/cancel/steer smoke test. — **40% · Partial.** Release packaging exists, but there is no comprehensive PR/release quality-gate workflow.
- [ ] ACP session smoke test. — **70% · Integrated.** Release packaging exists, but there is no comprehensive PR/release quality-gate workflow.
- [ ] Workflow and subagent smoke test. — **35% · Scaffold.** Release packaging exists, but there is no comprehensive PR/release quality-gate workflow.
- [ ] Policy hard-rail tests. — **72% · Integrated.** Release packaging exists, but there is no comprehensive PR/release quality-gate workflow.
- [ ] Recovery and migration tests. — **38% · Scaffold.** Release packaging exists, but there is no comprehensive PR/release quality-gate workflow.
- [ ] Primary provider contract tests. — **45% · Partial.** Release packaging exists, but there is no comprehensive PR/release quality-gate workflow.
- [ ] Performance budget check. — **15% · Not started.** Release packaging exists, but there is no comprehensive PR/release quality-gate workflow.
- [ ] Secret and dependency audits. — **55% · Partial.** Release packaging exists, but there is no comprehensive PR/release quality-gate workflow.

---

# 16. Architecture and product boundaries

**Class:** Boundary

imp stays fast and coherent by refusing features that do not strengthen its workflow.

## Core inclusion test

Before adding a core feature, answer yes to at least one:

- [ ] It materially improves difficult-task completion. — **54% · Partial.** Partial support exists in crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape; important end-to-end behavior or proof is missing.
- [ ] It reduces latency, tokens, repeated work, or intervention. — **54% · Partial.** Partial support exists in crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape; important end-to-end behavior or proof is missing.
- [ ] It is required for orchestration, recovery, policy, verification, or evidence correctness. — **54% · Partial.** Partial support exists in crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape; important end-to-end behavior or proof is missing.
- [ ] It makes the TUI cockpit clearer and more controllable. — **54% · Partial.** Partial support exists in crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape; important end-to-end behavior or proof is missing.
- [ ] It cannot be implemented safely as an extension or separate tool. — **54% · Partial.** Partial support exists in crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape; important end-to-end behavior or proof is missing.

Require all of the following:

- [ ] Named owner and maintenance plan. — **25% · Scaffold.** Only foundations/scaffolding are present in crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape; this is not an integrated product behavior.
- [ ] Measured startup and runtime cost. — **20% · Scaffold.** Only foundations/scaffolding are present in crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape; this is not an integrated product behavior.
- [ ] Defined policy and evidence behavior. — **72% · Integrated.** Usable through crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape, but not yet demonstrated at the full checklist standard.
- [ ] Isolated failure mode. — **58% · Partial.** Partial support exists in crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape; important end-to-end behavior or proof is missing.
- [ ] Removal or migration semantics if the experiment fails. — **45% · Partial.** Partial support exists in crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape; important end-to-end behavior or proof is missing.

## Prefer extension or separate tool when

- [ ] The capability serves a narrow language, provider, forge, or organization. — **78% · Integrated.** Usable through crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape, but not yet demonstrated at the full checklist standard.
- [ ] It changes frequently outside imp’s release cycle. — **72% · Integrated.** Usable through crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape, but not yet demonstrated at the full checklist standard.
- [ ] Stable tool, hook, RPC, or workflow contracts can support it. — **76% · Integrated.** Usable through crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape, but not yet demonstrated at the full checklist standard.
- [ ] It adds heavy dependencies or background processes. — **70% · Integrated.** Usable through crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape, but not yet demonstrated at the full checklist standard.
- [ ] It is useful but not required for the primary terminal workflow. — **76% · Integrated.** Usable through crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape, but not yet demonstrated at the full checklist standard.

## Deliberate exclusions unless evidence changes

- [ ] No generic consumer web chat. — **98% · Evidenced.** The current repository still respects this exclusion; keep it explicit during future scope reviews.
- [ ] No desktop shell solely for parity. — **78% · Integrated.** The GUI crate is experimental and excluded from default workspace membership, so the boundary is mostly preserved but not a complete absence.
- [ ] No public hosted session-sharing service. — **98% · Evidenced.** The current repository still respects this exclusion; keep it explicit during future scope reviews.
- [ ] No Slack or broad team-chat integration in core. — **98% · Evidenced.** The current repository still respects this exclusion; keep it explicit during future scope reviews.
- [ ] No provider-count race. — **58% · Partial.** Native and compatible adapters are numerous; no published tier discipline or measured value gate currently prevents breadth from outrunning depth.
- [ ] No unrestricted npm plugin runtime in core. — **98% · Evidenced.** The current repository still respects this exclusion; keep it explicit during future scope reviews.
- [ ] No default catalog of every language server and formatter. — **95% · Evidenced.** The current repository still respects this exclusion; keep it explicit during future scope reviews.
- [ ] No hosted billing or model marketplace in the runtime. — **98% · Evidenced.** The current repository still respects this exclusion; keep it explicit during future scope reviews.
- [ ] No broad enterprise control plane in the local agent. — **98% · Evidenced.** The current repository still respects this exclusion; keep it explicit during future scope reviews.
- [ ] No theme or customization work that outranks execution clarity. — **58% · Partial.** Theme selection and substantial settings UI exist while workflow/worker cockpit visibility remains incomplete, so this prioritization is not fully demonstrated.

## Subtraction discipline

- [ ] Review core tools and features periodically for overlap and low use. — **58% · Partial.** Partial support exists in crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape; important end-to-end behavior or proof is missing.
- [ ] Remove or consolidate features that do not justify conceptual cost. — **72% · Integrated.** Usable through crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape, but not yet demonstrated at the full checklist standard.
- [ ] Keep optional dependencies off startup and turn paths. — **68% · Integrated.** Usable through crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape, but not yet demonstrated at the full checklist standard.
- [ ] Track the user-facing concepts required for a normal run. — **50% · Partial.** Partial support exists in crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape; important end-to-end behavior or proof is missing.
- [ ] Treat reduced complexity as product improvement when capability remains. — **75% · Integrated.** Usable through crate boundaries, AGENTS policy, shipped Lua boundary, and current repository shape, but not yet demonstrated at the full checklist standard.

---

# 17. Documentation for expert users

**Class:** Control and Enabler

- [ ] Provide one concise mental model for sessions, runs, workflows, workers, evidence, policy, and autonomy. — **80% · Strong.** Implemented and exercised through the audited docs index and current technical reference pages; remaining work is edge hardening or product proof.
- [ ] Document shortest successful paths for TUI, one-shot, RPC, ACP, workflows, and worktree automation. — **68% · Integrated.** Usable through the audited docs index and current technical reference pages, but not yet demonstrated at the full checklist standard.
- [ ] Show exact interruption and recovery behavior. — **58% · Partial.** Partial support exists in the audited docs index and current technical reference pages; important end-to-end behavior or proof is missing.
- [ ] Show how to inspect effective config and policy. — **35% · Scaffold.** Config is typed and merged, but no complete effective-config provenance/explain command exists.
- [ ] Document provider tiers and capability differences. — **20% · Scaffold.** Provider adapters exist, but support tiers and capability guarantees are not published.
- [ ] Document durable storage and privacy boundaries. — **82% · Strong.** Implemented and exercised through the audited docs index and current technical reference pages; remaining work is edge hardening or product proof.
- [ ] Troubleshoot credentials, browser, MCP, extensions, sessions, worktrees, and verification. — **48% · Partial.** Current docs cover credentials, browser, sessions, worktrees, extensions, and verification unevenly; MCP remains explicitly unimplemented.
- [ ] Label planned and experimental capabilities. — **90% · Evidenced.** Demonstrated in the audited docs index and current technical reference pages; preserve the contract and regression coverage.
- [ ] Remove stale docs when behavior changes. — **90% · Evidenced.** Demonstrated in the audited docs index and current technical reference pages; preserve the contract and regression coverage.
- [ ] Prefer tested examples and real output over broad claims. — **68% · Integrated.** Usable through the audited docs index and current technical reference pages, but not yet demonstrated at the full checklist standard.

---

# 18. Recommended execution order

## Stage 1: establish proof

- [ ] Freeze representative coding, orchestration, interruption, and TUI benchmarks. — **40% · Partial.** Benchmark scaffolding exists, but no maintained baseline/gate proves this item continuously.
- [ ] Record correctness, latency, tokens, tools, and recovery baselines. — **25% · Scaffold.** Core hot-path benchmarks and external harness schemas exist, but no current committed baseline covers correctness, latency, tokens, tools, and recovery together.
- [ ] Identify the five largest sources of wasted time or failed work. — **20% · Scaffold.** The repository has instrumentation and candidate capture, but no current evidence-backed top-five waste analysis is committed.
- [ ] Set initial performance and reliability budgets. — **20% · Scaffold.** Only foundations/scaffolding are present in current implementation and remaining readiness gaps; this is not an integrated product behavior.

## Stage 2: harden the engine

- [ ] Fix false completion, repeated work, context waste, and weak failure recovery. — **68% · Integrated.** Closeout safeguards exist, but the dedicated false-success corpus and tracked rate are incomplete.
- [ ] Complete deterministic cancellation and recovery. — **58% · Partial.** Partial support exists in current implementation and remaining readiness gaps; important end-to-end behavior or proof is missing.
- [ ] Make workflow and worker state authoritative and resumable. — **48% · Partial.** Partial support exists in current implementation and remaining readiness gaps; important end-to-end behavior or proof is missing.
- [ ] Enforce verification invalidation after relevant edits. — **45% · Partial.** Partial support exists in current implementation and remaining readiness gaps; important end-to-end behavior or proof is missing.

## Stage 3: finish the cockpit

- [ ] Make objective, current action, workflow, workers, blockers, policy, diffs, and gates visible. — **52% · Partial.** Partial support exists in current implementation and remaining readiness gaps; important end-to-end behavior or proof is missing.
- [ ] Make steering, queueing, cancellation, and session navigation frictionless. — **72% · Integrated.** Usable through current implementation and remaining readiness gaps, but not yet demonstrated at the full checklist standard.
- [ ] Add effective config/policy explanation and diagnostics. — **30% · Scaffold.** Config is typed and merged, but no complete effective-config provenance/explain command exists.

## Stage 4: optimize

- [ ] Remove hot-path scans, unnecessary context, blocking rendering, and heavy startup work. — **50% · Partial.** Partial support exists in current implementation and remaining readiness gaps; important end-to-end behavior or proof is missing.
- [ ] Optimize tools using benchmark evidence. — **62% · Integrated.** Usable through current implementation and remaining readiness gaps, but not yet demonstrated at the full checklist standard.
- [ ] Deepen the primary provider tier. — **55% · Partial.** Provider adapters exist, but support tiers and capability guarantees are not published.
- [ ] Set CI gates for critical regressions. — **10% · Not started.** Current GitHub workflows package edge/releases; they do not enforce this regression gate.

## Stage 5: expose the engine narrowly

- [ ] Stabilize one-shot and JSONL RPC contracts. — **75% · Integrated.** Usable through current implementation and remaining readiness gaps, but not yet demonstrated at the full checklist standard.
- [ ] Make ACP excellent in one real editor. — **10% · Not started.** No real-editor ACP interoperability evidence is checked in.
- [ ] Add only integrations demonstrating a high-value expert workflow. — **50% · Partial.** Partial support exists in current implementation and remaining readiness gaps; important end-to-end behavior or proof is missing.
- [ ] Keep optional ecosystem work outside the core runtime. — **82% · Strong.** Implemented and exercised through current implementation and remaining readiness gaps; remaining work is edge hardening or product proof.

---

# 19. Handoff record

The next maintainer should fill this section before turning the vision into implementation work.

## Current baseline

- Benchmark revision: no current scored imp baseline is committed. `evals/dirac-comparison` contains eight pinned tasks, imported Dirac reference patches, and one dry-run artifact; `evals/terminal-bench-2` is adapter scaffolding.
- imp revision: reassessed on 2026-07-10 against `fd8601213` plus the uncommitted readiness ledger/checklist work.
- Comparison revisions: Dirac reference patches record upstream source metadata, but no matched imp/OpenCode/Pi run matrix is committed.
- Supported platforms: release workflows build macOS and Linux for x86_64 and aarch64; terminals/shells/credential stores do not yet have a published support matrix.
- Primary provider tier: not formally defined. Anthropic, OpenAI/Codex-compatible, and Google have the deepest adapters/tests, but guarantees are unpublished.
- Known critical failures: real parallel worker execution is incomplete; ACP prompts are stubbed; worktree-auto is not wired through normal CLI startup; evidence has overlapping legacy/current paths; no comprehensive PR quality or performance gate exists.
- Current performance budgets: no enforced budgets or current committed end-to-end latency baseline.

## Top five gaps

1. Finish real bounded-worker execution, cancellation, isolation, result verification, and integration.
2. Add verifier-backed regression breadth and CI gates for correctness, latency, tokens, and recovery.
3. Complete deterministic cancellation/recovery UX and systematic interruption testing.
4. Make objective, workflow/worker progress, diffs, policy, gates, and evidence first-class in the TUI.
5. Add effective-config/policy explanation and publish provider/platform support tiers.

## Active work

| Area | Owner | Workflow/issue | Target | Evidence | Status |
|---|---|---|---|---|---|
| Agent execution | Unassigned | task-state fast path | Preserve evidence/verification without unnecessary planning rounds | Agent/task-state tests | Partial; no current comparative latency baseline |
| Orchestration | Unassigned | bounded workers | Real concurrent execution with cancellation and verified integration | `crates/imp-core/src/agent/subagent.rs` | Scaffold/partial |
| Context | Unassigned | task-aware initial context | Prompt-relevant files/symbols/tests before first request | `context_prefill.rs`, `scan` | Partial |
| TUI | Unassigned | cockpit read model | Objective, workflow, workers, blockers, diff, policy, gates | `crates/imp-tui/src/app/` | Partial |
| Performance | Unassigned | performance budget | Define end-to-end latency and resource budgets | `crates/imp-core/benches/core_hot_paths.rs` | Core hot paths measured; end-to-end budget absent |
| Recovery | Unassigned | interruption matrix | Deterministic recovery across provider/tool/write/worker/check phases | recovery checkpoints/tests | Partial |
| Policy | Unassigned | effective policy explain | One pre-run explanation and cross-transport conformance matrix | ReferenceMonitor tests | Partial |
| Providers | Unassigned | support tiers | Published primary/secondary guarantees and contract tests | `crates/imp-llm` | Adapters strong; tiers missing |
| Evaluation | Unassigned | evaluation harnesses | Complete verifiers, commit scored baselines, add held-out/interruption/no-change tasks, and gate CI | `evals/dirac-comparison`, `evals/terminal-bench-2` | Eight pinned Dirac tasks; no committed scored imp baseline |

## Decision log

| Date | Decision | Why | Evidence | Revisit when |
|---|---|---|---|---|
| 2026-07-09 | Score every criterion by end-to-end integration; leave checkboxes open without named proof. | Code presence was overstating readiness and hiding product/verification gaps. | This audit rubric and 481 annotated criteria. | A criterion gains a stable test, benchmark, fixture, or release gate. |

## Final handoff questions

- [ ] What measurable behavior is better than the last baseline? — **20% · Scaffold.** No current committed comparable imp baseline supports a measured improvement claim; the previous paired-run artifact is absent.
- [ ] What failure remains most likely to waste a serious developer’s time? — **75% · Integrated.** Failed edit/command attempts and recovery rounds are the measured near-term waste; real worker orchestration and interruption recovery remain larger unmeasured risks on complex work.
- [ ] Which current feature adds the most complexity for the least core value? — **70% · Integrated.** Experimental GUI and non-Lua extension compatibility add maintenance surface without contributing to the shipped terminal/Lua product path; this is identified but not yet removed or fully isolated.
- [ ] Which claim lacks a reproducible test? — **90% · Evidenced.** “Bounded parallel workers” lacks a production end-to-end execution/integration test; current evidence covers contracts and read models rather than real concurrent completion.
- [ ] What should imp explicitly refuse to build next? — **92% · Evidenced.** Refuse hosted chat/session sharing, billing/marketplaces, broad enterprise control planes, and unrestricted plugin runtimes until execution correctness, recovery, worker orchestration, and measurable performance gates are complete.

The product stays a sports car by answering those questions honestly.
