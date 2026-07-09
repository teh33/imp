use std::path::Path;

use super::super::*;
use super::support::{git, repo, request};

#[tokio::test]
async fn doctor_reports_unmanaged_worktree_without_adopting_it() {
    let repo = repo();
    let state = tempfile::TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let unmanaged = repo
        .path()
        .join("..")
        .join(format!("unmanaged-tree-{}", uuid::Uuid::new_v4().simple()));
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-qb",
            "outside-agent",
            unmanaged.to_str().unwrap(),
        ],
    );

    let report = service.doctor(repo.path()).await.expect("doctor report");

    let unmanaged = unmanaged.canonicalize().expect("canonical unmanaged path");
    assert!(report.findings.iter().any(|finding| {
        finding.code == "unmanaged_worktree"
            && finding
                .path
                .as_ref()
                .and_then(|path| path.canonicalize().ok())
                .as_ref()
                == Some(&unmanaged)
    }));
    assert!(report.records.is_empty());
    git(
        repo.path(),
        &["worktree", "remove", "--force", unmanaged.to_str().unwrap()],
    );
}

#[tokio::test]
async fn doctor_reports_changed_path_overlap() {
    let repo = repo();
    let state = tempfile::TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let alpha = service
        .create(repo.path(), request("overlap-a"))
        .await
        .expect("alpha workspace");
    let beta = service
        .create(repo.path(), request("overlap-b"))
        .await
        .expect("beta workspace");
    std::fs::write(alpha.worktree_path.join("README.md"), "alpha\n").unwrap();
    std::fs::write(beta.worktree_path.join("README.md"), "beta\n").unwrap();

    let report = service.doctor(repo.path()).await.expect("doctor report");

    assert!(report.findings.iter().any(|finding| {
        finding.code == "changed_path_overlap"
            && finding.path.as_deref() == Some(Path::new("README.md"))
    }));
}
