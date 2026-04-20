use anyhow::Result;
use clap::Args;

#[derive(Args)]
pub struct PublishArgs {
    #[arg(long)]
    pub branch: String,
}

pub fn publish(args: PublishArgs) -> Result<()> {
    tracing::info!(branch = %args.branch, "publish");
    let _ = args;
    Ok(())
}
