use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, ValueEnum};
use rayon::prelude::*;

use crate::git::ensure_checkout;
use crate::langs::python::PythonRunner;
use crate::langs::{LangRunner, Mode, Status, TestResult, Workspace};
use crate::wasmer::WasmerRuntime;

#[derive(Debug, Clone, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Lang {
    Python,
    Node,
    Php,
    Rust,
}

#[derive(Args)]
pub struct RunArgs {
    #[arg(long)]
    pub lang: Lang,

    pub filter: Option<String>,

    #[arg(long, conflicts_with = "wasmer_ref")]
    pub wasmer: Option<PathBuf>,

    #[arg(long)]
    pub wasmer_ref: Option<String>,

    #[arg(long, value_parser = humantime::parse_duration, default_value = "10m")]
    pub timeout: Duration,

    #[arg(long, default_value = "origin/main")]
    pub compare_ref: String,
}

#[derive(Debug, PartialEq)]
pub struct ExecutionReport {
    pub results: Vec<TestResult>,
    pub counts: StatusCounts,
    pub errors: Vec<ItemError>,
}

#[derive(Debug, PartialEq)]
pub struct StatusCounts(pub HashMap<Status, usize>);

#[derive(Debug, PartialEq)]
pub struct ItemError {
    pub id: String,
    pub message: String,
}

impl StatusCounts {
    pub fn increment(&mut self, status: Status) {
        *self.0.entry(status).or_insert(0) += 1;
    }
    pub fn total(&self) -> usize {
        self.0.values().sum()
    }
}

pub fn run(args: RunArgs) -> Result<()> {
    let wasmer_path = args.wasmer.clone().ok_or_else(|| {
        anyhow!("only --wasmer <PATH> is wired up so far; --wasmer-ref / default `main` not yet implemented")
    })?;
    if !wasmer_path.is_file() {
        bail!("--wasmer {} is not a file", wasmer_path.display());
    }
    if !matches!(args.lang, Lang::Python) {
        bail!(
            "runner for {:?} not yet implemented — only python works today",
            args.lang
        );
    }
    let runner = PythonRunner::new();
    let opts = PythonRunner::OPTS;
    let work_dir = PathBuf::from("runs").join(opts.name);
    let checkout = ensure_checkout(&work_dir, opts.git_repo, opts.git_ref)?;
    let workspace = Workspace { checkout, work_dir };
    let wasmer = WasmerRuntime::new(wasmer_path, args.timeout);
    tracing::info!("Running tests");
    let mode = if args.filter.is_some() {
        Mode::Debug
    } else {
        Mode::Capture
    };
    let report = execute_tests(&runner, &workspace, &wasmer, args.filter.as_deref(), mode)?;
    tracing::info!(counts = ?report.counts.0, errors = report.errors.len(), "done");
    Ok(())
}

pub fn execute_tests<R: LangRunner>(
    runner: &R,
    workspace: &Workspace,
    wasmer: &WasmerRuntime,
    filter: Option<&str>,
    mode: Mode,
) -> Result<ExecutionReport> {
    runner.prepare(workspace, wasmer)?;
    let ids = runner.discover(workspace, filter)?;
    let run_one = |id: &String| -> Result<Vec<TestResult>, ItemError> {
        runner
            .run_test(workspace, wasmer, id, mode)
            .map_err(|e| ItemError {
                id: id.clone(),
                message: format!("{e:#}"),
            })
    };
    let outcomes: Vec<Result<Vec<TestResult>, ItemError>> = match mode {
        Mode::Capture => ids.par_iter().map(run_one).collect(),
        Mode::Debug => ids.iter().map(run_one).collect(),
    };
    let mut results = Vec::new();
    let mut errors = Vec::new();
    let mut counts = StatusCounts(HashMap::new());
    for outcome in outcomes {
        match outcome {
            Ok(tests) => {
                for r in tests {
                    counts.increment(r.status);
                    results.push(r);
                }
            }
            Err(e) => {
                errors.push(e);
            }
        }
    }
    Ok(ExecutionReport {
        results,
        counts,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::langs::{Status, tests::MockRunner};

    #[test]
    fn mock_runner_reports_mixed_statuses() {
        let workspace = Workspace {
            checkout: PathBuf::new(),
            work_dir: PathBuf::new(),
        };
        let wasmer = WasmerRuntime::new(PathBuf::new(), Duration::ZERO);

        let report = execute_tests(&MockRunner, &workspace, &wasmer, None, Mode::Capture)
            .expect("execute_tests should succeed");

        assert_eq!(
            report,
            ExecutionReport {
                results: vec![
                    TestResult {
                        id: "pass_a".into(),
                        status: Status::Pass
                    },
                    TestResult {
                        id: "pass_b".into(),
                        status: Status::Pass
                    },
                    TestResult {
                        id: "fail_c".into(),
                        status: Status::Fail
                    },
                    TestResult {
                        id: "skip_d".into(),
                        status: Status::Skip
                    },
                    TestResult {
                        id: "timeout_e".into(),
                        status: Status::Timeout
                    },
                    TestResult {
                        id: "flaky_f".into(),
                        status: Status::Flaky
                    },
                ],
                counts: StatusCounts(HashMap::from([
                    (Status::Pass, 2),
                    (Status::Fail, 1),
                    (Status::Skip, 1),
                    (Status::Timeout, 1),
                    (Status::Flaky, 1),
                ])),
                errors: vec![],
            }
        );
    }
}
