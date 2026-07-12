use crate::agent::{Agent, AgentEvent};
use crate::workflow::{
    capture_worktree_diff_artifacts, write_worktree_metadata, VerificationGateRunner,
    WorktreeRunMetadata,
};
use crate::{storage, trace::TraceWriter};

impl Agent {
    pub(super) async fn run_verification_gates(&mut self, artifacts: &storage::RunArtifacts) {
        let runner = VerificationGateRunner::new(&self.cwd, artifacts.root().join("verification"));
        let mut completed = Vec::new();
        for index in 0..self.verification_gates.len() {
            if matches!(
                self.verification_gates[index].status,
                crate::workflow::VerificationGateStatus::Passed
                    | crate::workflow::VerificationGateStatus::Failed
                    | crate::workflow::VerificationGateStatus::Blocked
                    | crate::workflow::VerificationGateStatus::Skipped
            ) {
                continue;
            }
            self.emit(AgentEvent::VerificationStarted {
                gate: self.verification_gates[index].clone(),
            })
            .await;
            let _ = runner.run(&mut self.verification_gates[index]).await;
            completed.push(self.verification_gates[index].clone());
        }
        for gate in completed {
            self.emit(AgentEvent::VerificationCompleted {
                closeout_effect: gate.closeout_effect(),
                gate,
            })
            .await;
        }
    }

    pub(super) async fn capture_worktree_run_artifacts(
        &self,
        artifacts: &storage::RunArtifacts,
    ) -> Option<WorktreeRunMetadata> {
        let mut metadata = self.worktree_run_metadata.clone()?;
        self.write_trace_event(&AgentEvent::WorktreeCreated {
            metadata: metadata.clone(),
        });
        let worktree_artifact_dir = artifacts.root().join("worktree");
        match capture_worktree_diff_artifacts(&mut metadata, &worktree_artifact_dir).await {
            Ok(_) => {
                let _ = write_worktree_metadata(
                    &worktree_artifact_dir.join("worktree-metadata.json"),
                    &metadata,
                )
                .await;
                self.write_trace_event(&AgentEvent::WorktreeDiffCaptured {
                    metadata: metadata.clone(),
                });
                Some(metadata)
            }
            Err(err) => {
                self.write_trace_event(&AgentEvent::Warning {
                    message: format!("failed to capture worktree diff artifacts: {err}"),
                });
                None
            }
        }
    }

    pub(super) fn start_trace_writer(&self, artifacts: &storage::RunArtifacts) {
        if let Ok(writer) = TraceWriter::create(artifacts.trace_path()) {
            if let Ok(mut active_trace_writer) = self.trace_writer.lock() {
                *active_trace_writer = Some(writer);
            }
        }
    }
}
