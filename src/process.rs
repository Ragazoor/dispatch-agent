use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::process::Output;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

pub trait ProcessRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output>;
}

// ---------------------------------------------------------------------------
// Real implementation — wraps std::process::Command
// ---------------------------------------------------------------------------

pub struct RealProcessRunner;

impl ProcessRunner for RealProcessRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output> {
        std::process::Command::new(program)
            .args(args)
            .output()
            .with_context(|| format!("failed to run {program}"))
    }
}

// ---------------------------------------------------------------------------
// Mock implementation — for tests only
// ---------------------------------------------------------------------------

pub struct MockProcessRunner {
    pub calls: Mutex<Vec<(String, Vec<String>)>>,
    responses: Mutex<VecDeque<Result<Output>>>,
}

impl MockProcessRunner {
    pub fn new(responses: Vec<Result<Output>>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(VecDeque::from(responses)),
        }
    }

    pub fn recorded_calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().unwrap().clone()
    }

    /// Successful Output with empty stdout/stderr.
    pub fn ok() -> Result<Output> {
        Ok(Output {
            status: exit_ok(),
            stdout: vec![],
            stderr: vec![],
        })
    }

    /// Successful Output with specific stdout bytes.
    pub fn ok_with_stdout(stdout: &[u8]) -> Result<Output> {
        Ok(Output {
            status: exit_ok(),
            stdout: stdout.to_vec(),
            stderr: vec![],
        })
    }

    /// Failed Output (non-zero exit) with specific stderr.
    pub fn fail(stderr: &str) -> Result<Output> {
        Ok(Output {
            status: exit_fail(),
            stdout: vec![],
            stderr: stderr.as_bytes().to_vec(),
        })
    }
}

impl ProcessRunner for MockProcessRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output> {
        self.calls
            .lock()
            .unwrap()
            .push((program.to_string(), args.iter().map(|s| s.to_string()).collect()));
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| panic!("MockProcessRunner: no response queued for {program} {args:?}"))
    }
}

// ---------------------------------------------------------------------------
// Helpers for constructing ExitStatus in tests (Unix only)
// ---------------------------------------------------------------------------

#[cfg(unix)]
pub fn exit_ok() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(0)
}

#[cfg(unix)]
pub fn exit_fail() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    // Raw status word: exit code 1 = 1 << 8 = 256
    std::process::ExitStatus::from_raw(1 << 8)
}
