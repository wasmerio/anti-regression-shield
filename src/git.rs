use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

pub fn ensure_checkout(work_dir: &Path, repo: &str, git_ref: &str) -> Result<PathBuf> {
    let checkout = work_dir.join("checkout");
    std::fs::create_dir_all(work_dir)?;
    if !checkout.join(".git").exists() {
        git(&["clone", "--depth", "1", repo, &checkout.display().to_string()], None)?;
    }
    let target = if has_local_commit(&checkout, git_ref) {
        rev_parse(&checkout, git_ref)?
    } else {
        git(&["fetch", "--depth", "1", "origin", git_ref], Some(&checkout))?;
        rev_parse(&checkout, "FETCH_HEAD")?
    };
    if head_commit(&checkout).as_deref() != Some(target.as_str()) {
        git(&["checkout", "-B", "shield-checkout", &target], Some(&checkout))?;
    }
    Ok(checkout)
}

fn has_local_commit(checkout: &Path, sha: &str) -> bool {
    Command::new("git")
        .args(["rev-parse", "-q", "--verify", &format!("{sha}^{{commit}}")])
        .current_dir(checkout)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn head_commit(checkout: &Path) -> Option<String> {
    rev_parse(checkout, "HEAD").ok()
}

fn rev_parse(checkout: &Path, rev: &str) -> Result<String> {
    let out = Command::new("git")
        .args(["rev-parse", rev])
        .current_dir(checkout)
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("spawn git rev-parse {rev}"))?;
    if !out.status.success() {
        bail!("git rev-parse {rev:?} exited with {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git(args: &[&str], cwd: Option<&Path>) -> Result<()> {
    tracing::info!(?args, ?cwd, "git");
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    let status = cmd
        .status()
        .with_context(|| format!("spawn git {args:?}"))?;
    if !status.success() {
        bail!("git {args:?} exited with {status}");
    }
    Ok(())
}
