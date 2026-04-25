use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::Args;

use crate::verdict::build_verdict;

#[derive(Args)]
pub struct PrCommentArgs {
    /// Wasmer repository under test, for example `wasmerio/wasmer`.
    #[arg(long)]
    pub target_repo: String,
    /// Exact Wasmer commit SHA this compat run is expected to cover.
    #[arg(long)]
    pub target_sha: String,
    /// URL of the compat-tests workflow run to link from the PR comment.
    #[arg(long)]
    pub run_url: String,
    /// Repository where the PR comment should be posted.
    #[arg(long)]
    pub comment_repo: String,
    /// Pull request number to comment on.
    #[arg(long)]
    pub comment_pr_number: String,
    /// GitHub token used to post the PR comment.
    #[arg(long)]
    pub github_token: String,
    /// Optional compat-tests branch that stores the published PR snapshot.
    #[arg(long, default_value = "")]
    pub results_branch: String,
    /// Optional compat-tests commit that stores the published PR snapshot.
    #[arg(long, default_value = "")]
    pub results_commit: String,
}

pub fn pr_comment(args: PrCommentArgs) -> Result<()> {
    tracing::info!(repo = %args.comment_repo, pr = %args.comment_pr_number, "pr-comment");
    let verdict = build_verdict(
        Path::new("."),
        &args.target_sha,
        &args.run_url,
        &args.results_branch,
        &args.results_commit,
    )?;
    let body_path = write_body(&verdict.body)?;
    post_comment(
        &args.comment_repo,
        &args.comment_pr_number,
        &args.github_token,
        &body_path,
    )?;
    print!("{}", verdict.body);
    Ok(())
}
fn write_body(body: &str) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("shield-pr-comment-{}.md", std::process::id()));
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn post_comment(repo: &str, pr_number: &str, github_token: &str, body_path: &Path) -> Result<()> {
    let status = Command::new("gh")
        .args([
            "pr",
            "comment",
            pr_number,
            "--repo",
            repo,
            "--body-file",
            &body_path.display().to_string(),
        ])
        .env("GH_TOKEN", github_token)
        .status()
        .context("spawn gh pr comment")?;
    if status.success() {
        Ok(())
    } else {
        bail!("gh pr comment exited with {status}")
    }
}
