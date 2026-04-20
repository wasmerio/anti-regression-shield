use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Error, Result, bail};
use clap::{Args, ValueEnum};
use rayon::prelude::*;

use crate::langs::{LangRunner, Status, TestResult, Workspace};
use crate::wasmer::WasmerRunner;

#[derive(Debug, Clone, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Lang {
    Python,
    Node,
    Php,
    Rust,
}

/// Resolved `--wasmer` source.
#[derive(Debug, Clone)]
pub enum WasmerSource {
    /// Prebuilt wasmer binary at the given path.
    Binary(PathBuf),
    /// Git ref to fetch + build (or fetch a prebuilt artifact for `main`).
    GitRef(String),
}

#[derive(Args)]
pub struct RunArgs {
    /// Language to run.
    #[arg(long)]
    pub lang: Lang,

    /// Test filer - when set, runs tests matching passed substring and uses debug mode: raw stdout/stderr,
    /// no status.json / metadata.json written.
    pub filter: Option<String>,

    /// Wasmer to test against - either path to the local wasmer binary or git ref otherwise
    #[arg(long)]
    pub wasmer: Option<WasmerSource>,

    /// Per-test timeout (e.g. `30s`, `10m`, `1h`).
    #[arg(long, value_parser = humantime::parse_duration, default_value = "10m")]
    pub timeout: Duration,

    /// Git ref inside the shield repo to compare against
    /// (drives flaky detection and comparison.json).
    #[arg(long, default_value = "origin/main")]
    pub compare_ref: String,
}

/// Aggregate outcome of one `execute_tests` call. `run()` turns this into
/// `status.json` / `metadata.json`; unit tests assert on it whole.
#[derive(Debug, PartialEq)]
pub struct ExecutionReport {
    /// Every completed test, in completion order.
    pub results: Vec<TestResult>,
    /// Pre-aggregated summary for logging + metadata.
    pub counts: StatusCounts,
    /// Per-item failures captured *without* bailing — wasmer subprocess
    /// crashes, Rust-side panics inside `run_test`, parse errors.
    pub errors: Vec<ItemError>,
}

#[derive(Debug, PartialEq)]
pub struct StatusCounts(pub HashMap<Status, usize>);

/// One item's `run_test` failed or panicked. The item's tests are not in
/// `results` (we never got a parseable outcome); `message` is the full
/// stringified error (anyhow's chain via `{:#}` formatting) for logs.
#[derive(Debug, PartialEq)]
pub struct ItemError {
    pub id: String,
    pub message: String,
}

impl FromStr for WasmerSource {
    type Err = Error;

    fn from_str(source: &str) -> Result<Self> {
        let path = PathBuf::from(source);
        if path.is_file() {
            Ok(Self::Binary(path))
        } else {
            Ok(Self::GitRef(source.to_string()))
        }
    }
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
    let wasmer = args
        .wasmer
        .unwrap_or_else(|| WasmerSource::GitRef("main".to_string()));
    tracing::info!(
        lang = ?args.lang,
        filter = args.filter.as_deref(),
        ?wasmer,
        timeout = %humantime::format_duration(args.timeout),
        "run",
    );
    // TODO: resolve wasmer -> WasmerRunner, clone upstream -> Workspace,
    // docker-compose up if needed, dispatch on args.lang to the matching
    // LangRunner, call execute_tests(&runner, ...), then write status.json
    // + metadata.json + docker-compose down.
    Ok(())
}

/// Run `runner` against `workspace` using `wasmer`, parallelized across
/// rayon's default pool (sized to `num_cpus`). Per-item `Err`s are
/// recorded in `report.errors`; `prepare` / `discover` failures and an
/// empty discovery bail via `anyhow::Error`. Rust-side panics are not
/// caught — they indicate a bug in our parser, not test data.
pub fn execute_tests<R: LangRunner>(
    runner: &R,
    workspace: &Workspace,
    wasmer: &WasmerRunner,
    filter: Option<&str>,
) -> Result<ExecutionReport> {
    runner.prepare(workspace, wasmer).context("runner.prepare() failed")?;
    let ids = runner.discover(workspace, filter).context("runner.discover() failed")?;
    if ids.is_empty() {
        match filter {
            Some(f) => bail!("no tests matched filter {f:?}"),
            None => bail!("runner discovered 0 tests"),
        }
    }
    let outcomes: Vec<Result<Vec<TestResult>, ItemError>> = ids
        .par_iter()
        .map(|id| {
            runner.run_test(workspace, wasmer, id).map_err(|e| ItemError {
                id: id.clone(),
                message: format!("{e:#}"),
            })
        })
        .collect();
    let mut results = Vec::new();
    let mut errors = Vec::new();
    let mut counts = StatusCounts(HashMap::new());
    for outcome in outcomes {
        match outcome {
            Ok(tests) => {
                for r in tests {
                    tracing::info!(id = %r.id, status = ?r.status, "test");
                    counts.increment(r.status);
                    results.push(r);
                }
            }
            Err(e) => {
                tracing::warn!(id = %e.id, "{}", e.message);
                errors.push(e);
            }
        }
    }
    Ok(ExecutionReport { results, counts, errors })
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
        let wasmer = WasmerRunner::new(PathBuf::new(), Duration::ZERO);

        let report = execute_tests(&MockRunner, &workspace, &wasmer, None)
            .expect("execute_tests should succeed");

        assert_eq!(
            report,
            ExecutionReport {
                results: vec![
                    TestResult { id: "pass_a".into(), status: Status::Pass },
                    TestResult { id: "pass_b".into(), status: Status::Pass },
                    TestResult { id: "fail_c".into(), status: Status::Fail },
                    TestResult { id: "skip_d".into(), status: Status::Skip },
                    TestResult { id: "timeout_e".into(), status: Status::Timeout },
                    TestResult { id: "flaky_f".into(), status: Status::Flaky },
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
