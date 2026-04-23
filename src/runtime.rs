use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

pub use crate::process::Stream;
use crate::process::{ProcessError, ProcessSpec, run_process};
use crate::run_log::RunLog;

pub struct WasmerRuntime {
    binary: PathBuf,
    default_timeout: Duration,
    process_log: Arc<RunLog>,
}

pub struct RunSpec {
    pub package: String,
    pub flags: Vec<String>,
    pub args: Vec<String>,
    pub timeout: Option<Duration>,
}

impl WasmerRuntime {
    pub fn new(binary: PathBuf, default_timeout: Duration, process_log: Arc<RunLog>) -> Self {
        Self {
            binary,
            default_timeout,
            process_log,
        }
    }

    pub fn run<F>(&self, spec: RunSpec, on_line: F) -> std::result::Result<(), ProcessError>
    where
        F: FnMut(Stream, &str) -> Result<()>,
    {
        let timeout = spec.timeout.unwrap_or(self.default_timeout);
        let mut args: Vec<OsString> = vec!["run".into(), "--net".into()];
        args.extend(spec.flags.iter().map(OsString::from));
        args.push((&spec.package).into());
        if !spec.args.is_empty() {
            args.push("--".into());
            args.extend(spec.args.iter().map(OsString::from));
        }
        run_process(
            ProcessSpec {
                program: self.binary.clone(),
                args,
                cwd: std::env::current_dir()
                    .map_err(|e| ProcessError::Spawn(format!("resolve cwd: {e}")))?,
                timeout,
                log_output: self.process_log.clone(),
            },
            on_line,
        )
    }

    pub fn compile(&self, _wasm: &Path) -> Result<PathBuf> {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempdir::TempDir;

    #[test]
    fn runs_wasmer_version() {
        let dir = TempDir::new("shield-runtime").expect("tempdir");
        let mut version = String::new();
        run_process(
            ProcessSpec {
                program: "wasmer".into(),
                args: vec!["--version".into()],
                cwd: std::env::current_dir().expect("cwd"),
                timeout: Duration::from_secs(10),
                log_output: Arc::new(RunLog::new(dir.path().join("process.log"))),
            },
            |stream, line| {
                if matches!(stream, Stream::Stdout) {
                    version.push_str(line);
                    version.push('\n');
                }
                Ok(())
            },
        )
        .expect("version");
        assert!(version.to_lowercase().contains("wasmer"));
    }
}
