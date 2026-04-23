use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Result, anyhow};

pub struct RunLog {
    path: PathBuf,
    file: Mutex<File>,
}

impl RunLog {
    pub fn new(path: PathBuf) -> Self {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open run log");
        Self {
            path,
            file: Mutex::new(file),
        }
    }

    pub fn clear(&self) -> Result<()> {
        let mut f = self.file.lock().map_err(|_| anyhow!("log lock poisoned"))?;
        *f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        Ok(())
    }

    pub fn write_line(&self, stream: &str, line: &str) -> Result<()> {
        let mut f = self.file.lock().map_err(|_| anyhow!("log lock poisoned"))?;
        writeln!(f, "[{stream}] {line}")?;
        Ok(())
    }
}
