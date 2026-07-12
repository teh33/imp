use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub(crate) enum EvidenceCommand {
    /// List recent run evidence records
    List,
    /// Print the latest evidence HTML path
    Latest,
}

pub(crate) fn run(command: Option<&EvidenceCommand>) -> imp_core::Result<()> {
    let records =
        imp_core::run_evidence::read_index_records(imp_core::storage::global_run_index_path())?;
    match command.unwrap_or(&EvidenceCommand::List) {
        EvidenceCommand::List => {
            for record in records.iter().rev().take(20) {
                let status = record.status.as_deref().unwrap_or("running");
                println!(
                    "{}\t{}\t{}\t{}",
                    record.run_id,
                    status,
                    record.cwd.display(),
                    record.evidence_html_path.display()
                );
            }
        }
        EvidenceCommand::Latest => {
            if let Some(record) = records.last() {
                println!("{}", record.evidence_html_path.display());
            }
        }
    }
    Ok(())
}
