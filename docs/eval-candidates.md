# Eval candidates

Eval candidates capture useful failures and corrections from normal agent runs so they can later become regression tasks. They are records, not executable eval specifications.

For the executable harness, see [Coding eval runner](eval-runner.md).

## Current behavior

At closeout, the agent can write a redacted candidate sidecar under the project-local run artifacts:

```text
.imp/runs/<run-id>/eval-candidates/<run-id>-closeout/candidate.json
```

The automatic closeout path records candidates for selected failed, blocked, concern-bearing, or verification-related outcomes. Clean `DONE` runs do not produce a candidate by default.

Candidate records reference sibling run artifacts instead of copying large files:

```text
.imp/runs/<run-id>/trace.jsonl
.imp/runs/<run-id>/evidence.md
.imp/runs/<run-id>/workflow-contract.json
.imp/runs/<run-id>/verification/...
```

Core schema and redaction code lives in:

- `crates/imp-core/src/eval_candidate.rs`
- `crates/imp-core/src/eval_candidate_closeout.rs`
- `crates/imp-core/src/agent/workflow_integration/recipe_runtime.rs`

## Failure modes

The typed schema supports classifications such as:

- `blocked`;
- `done-with-concerns`;
- `verification-failed`;
- `verification-blocked`;
- `verification-skipped-required`;
- `policy-denied`;
- `tool-loop`;
- `tool-error`;
- `user-correction`;
- `negative-feedback`;
- `worktree-apply-conflict`;
- `manual`;
- `unknown`.

`failure_mode` identifies the primary reason. Labels can preserve additional facets.

## Schema

A candidate contains:

- schema version, id, and creation time;
- source run/workflow/session ids when available;
- trigger and primary failure mode;
- prompt or task summary;
- expected and actual behavior;
- verifier records;
- typed artifact references;
- policy references;
- privacy/redaction state;
- trust/provenance summary;
- optional human notes or correction metadata.

Example shape:

```json
{
  "schema_version": 1,
  "id": "run_abc-closeout",
  "source": { "run_id": "run_abc" },
  "trigger": "verification-failed",
  "failure_mode": "verification-failed",
  "expected_behavior": {
    "summary": "Required verification should pass"
  },
  "actual_behavior": {
    "summary": "Parser test failed"
  },
  "artifact_refs": [
    { "kind": "trace", "path": ".imp/runs/run_abc/trace.jsonl" },
    { "kind": "evidence", "path": ".imp/runs/run_abc/evidence.md" }
  ],
  "privacy": { "redaction_status": "redacted" }
}
```

## Verification and policy metadata

Verifier records can include the command, required flag, status, exit code, output path, and failure excerpt. Policy records can include tool name, decision, reason code, autonomy mode, resource scope, and trust labels.

Secret values must not be stored. Artifact references and short excerpts are preferred over raw output.

## Privacy

Before persistence, the closeout path applies candidate redaction. The schema tracks one of:

- `unreviewed`;
- `redacted`;
- `contains-sensitive-data`;
- `safe-to-export`.

Local storage is not an export guarantee. Review candidate JSON and every referenced artifact before sharing.

## Current limitations

- There is no shipped CLI/TUI browser for candidate sidecars.
- Manual “save correction as candidate” UX is not wired.
- Candidates are not automatically promoted into `evals/coding-agent/tasks`.
- The coding eval runner does not directly ingest candidate JSON.

Promotion should remain deliberate: choose a stable fixture or pinned repository, write a deterministic verifier, remove sensitive context, and create a normal eval task spec.
