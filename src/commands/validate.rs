use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::Args;

use crate::reports::load_metadata_at_ref;

#[derive(Args)]
pub struct ValidateArgs {
    /// Runner name used to locate the baseline summary file.
    #[arg(long)]
    pub runner: String,

    /// Wasmer git repository being tested.
    #[arg(long)]
    pub wasmer_repo: String,

    /// Wasmer git ref or SHA being tested.
    #[arg(long)]
    pub wasmer_ref: String,

    /// Git ref used to load baseline metadata.
    #[arg(long, default_value = "origin/main")]
    pub compare_ref: String,
}

pub fn validate(args: ValidateArgs) -> Result<()> {
    let output_dir = std::env::current_dir()?;
    let baseline = load_metadata_at_ref(&output_dir, &args.compare_ref, &args.runner)?;
    if baseline.wasmer.commit.is_empty() {
        return Ok(());
    }

    let checkout = output_dir.join(".work").join("validate-wasmer-ancestry");
    ensure_git_dir(&checkout, &args.wasmer_repo)?;
    fetch_commitish(&checkout, &args.wasmer_ref)?;
    let target = rev_parse(&checkout, "FETCH_HEAD")?;
    fetch_commitish(&checkout, &baseline.wasmer.commit)?;
    let baseline_commit = rev_parse(&checkout, "FETCH_HEAD")?;

    if !is_ancestor(&checkout, &baseline_commit, &target)? {
        bail!(
            "baseline Wasmer commit {baseline_commit} is newer that target Wasmer commit {target}. Consider updating your branch"
        );
    }

    Ok(())
}

fn ensure_git_dir(path: &Path, repo: &str) -> Result<()> {
    std::fs::create_dir_all(path)?;
    if !path.join(".git").exists() {
        git(&["init"], path)?;
    }
    if remote_url(path).ok().as_deref() != Some(repo) {
        let _ = git(&["remote", "remove", "origin"], path);
        git(&["remote", "add", "origin", repo], path)?;
    }
    Ok(())
}

fn remote_url(path: &Path) -> Result<String> {
    output(
        Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(path),
    )
}

fn fetch_commitish(path: &Path, commitish: &str) -> Result<()> {
    git(
        &[
            "fetch",
            "--filter=blob:none",
            "--no-tags",
            "origin",
            commitish,
        ],
        path,
    )
}

fn rev_parse(path: &Path, rev: &str) -> Result<String> {
    output(
        Command::new("git")
            .args(["rev-parse", rev])
            .current_dir(path),
    )
}

fn is_ancestor(path: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let status = Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("spawn git merge-base in {}", path.display()))?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("git merge-base --is-ancestor exited with {status}"),
    }
}

fn git(args: &[&str], path: &Path) -> Result<()> {
    tracing::info!(?args, cwd = %path.display(), "git");
    let status = Command::new("git")
        .args(args)
        .current_dir(path)
        .status()
        .with_context(|| format!("spawn git {args:?}"))?;
    if !status.success() {
        bail!("git {args:?} exited with {status}");
    }
    Ok(())
}

fn output(cmd: &mut Command) -> Result<String> {
    let out = cmd
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("spawn {cmd:?}"))?;
    if !out.status.success() {
        bail!("{cmd:?} exited with {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
