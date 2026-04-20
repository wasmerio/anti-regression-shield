use anyhow::Result;
use clap::Args;

#[derive(Args)]
pub struct IssueArgs {}

pub fn issue(args: IssueArgs) -> Result<()> {
    tracing::info!("issue");
    let _ = args;
    Ok(())
}
