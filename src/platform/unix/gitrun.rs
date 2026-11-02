//! No git runner on Unix yet: a run reports that it could not start, with the reason.

use std::path::Path;
use std::time::Duration;

use crate::git_state::{GitRun, RunResult};
use crate::platform::Unsupported;

pub struct SystemGitRun;

impl GitRun for SystemGitRun {
    fn run(&self, _git: &Path, _dir: &Path, _args: &[String], _timeout: Duration, _cap: Option<usize>) -> RunResult {
        RunResult::Failed(Unsupported { feature: "Running git" }.to_string())
    }
}
