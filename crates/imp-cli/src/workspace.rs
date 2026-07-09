use std::path::Path;

use imp_core::managed_workspace::{
    CreateManagedWorkspace, ManagedWorkspaceRecord, ManagedWorkspaceService,
};

use crate::WorkspaceCommand;

type WorkspaceCliResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(super) async fn run(command: &WorkspaceCommand) -> WorkspaceCliResult<()> {
    let cwd = std::env::current_dir()?;
    let service = ManagedWorkspaceService::global();
    match command {
        WorkspaceCommand::Create {
            id,
            run_id,
            task,
            base,
            json,
        } => {
            let generated_id = uuid::Uuid::new_v4().simple().to_string();
            let id = id.clone().unwrap_or(generated_id);
            let run_id = run_id.clone().unwrap_or_else(|| id.clone());
            let record = service
                .create(
                    &cwd,
                    CreateManagedWorkspace {
                        id: Some(id),
                        run_id,
                        task: task.clone(),
                        base_ref: base.clone(),
                    },
                )
                .await?;
            print_record(&record, *json)?;
        }
        WorkspaceCommand::List { json } => {
            let records = service.list(&cwd).await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else if records.is_empty() {
                println!("No managed workspaces.");
            } else {
                for record in records {
                    println!(
                        "{}\t{:?}\t{}\t{} changed",
                        record.id,
                        record.state,
                        display_path(&cwd, &record.worktree_path),
                        record.changed_paths.len()
                    );
                }
            }
        }
        WorkspaceCommand::Inspect { id, json } => {
            let record = service.inspect(&cwd, id).await?;
            print_record(&record, *json)?;
        }
        WorkspaceCommand::Ready { id, json } => {
            let record = service.mark_ready(&cwd, id).await?;
            print_record(&record, *json)?;
        }
        WorkspaceCommand::Integrate { id, json } => {
            let record = service.integrate(&cwd, id).await?;
            print_record(&record, *json)?;
        }
        WorkspaceCommand::Doctor { json } => {
            let report = service.doctor(&cwd).await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Registry: {}", report.registry_path.display());
                println!("Managed workspaces: {}", report.records.len());
                if report.is_healthy() {
                    println!("No workspace findings.");
                } else {
                    for finding in report.findings {
                        println!("{}: {}", finding.code, finding.message);
                    }
                }
            }
        }
        WorkspaceCommand::Discard { id, yes, json } => {
            let record = service.discard(&cwd, id, *yes).await?;
            print_record(&record, *json)?;
        }
    }
    Ok(())
}

fn print_record(record: &ManagedWorkspaceRecord, json: bool) -> WorkspaceCliResult<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(record)?);
    } else {
        println!("Workspace: {}", record.id);
        println!("State: {:?}", record.state);
        println!("Path: {}", record.worktree_path.display());
        println!("Branch: {}", record.branch);
        println!("Base: {} ({})", record.base_ref, record.base_commit);
        if let Some(task) = &record.task {
            println!("Task: {task}");
        }
        if !record.changed_paths.is_empty() {
            println!("Changed paths:");
            for path in &record.changed_paths {
                println!("  {}", path.display());
            }
        }
        if let Some(diagnostic) = &record.diagnostic {
            println!("Diagnostic: {diagnostic}");
        }
    }
    Ok(())
}

fn display_path(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd).unwrap_or(path).display().to_string()
}
