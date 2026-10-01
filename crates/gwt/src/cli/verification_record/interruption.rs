//! The runner cannot persist its own SIGKILL. A small companion waits for its
//! private pipe to close and settles only that run's unfinished record. It owns
//! no verification lease and can never produce passing evidence.

use std::{
    io::{self, BufRead, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Stdio},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{integrity_ok, load, save, VerificationRunRecord};

pub const WATCHDOG_ARG: &str = "--verification-watchdog";
const READY: &str = "verification-watchdog-ready\n";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Interrupted,
}

/// Absent on completed (including legacy) records. An unfinished record is
/// never evidence of test success or test failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLifecycle {
    pub status: RunStatus,
    pub runner_pid: u32,
    /// The private pipe carries the token; only its digest reaches a record.
    /// A record ID from a diagnostic mirror does not authorize interruption.
    pub watchdog_token_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl RunLifecycle {
    pub(super) fn running(token: &str) -> Self {
        Self {
            status: RunStatus::Running,
            runner_pid: std::process::id(),
            watchdog_token_hash: token_hash(token),
            current_command: None,
            reason: None,
        }
    }
}

pub(super) struct Watchdog {
    child: Child,
    pipe: Option<ChildStdin>,
}

impl Watchdog {
    /// Used by the real gwtd entrypoint; unit-level in-process runs have no
    /// gwtd executable and exercise the record transitions directly instead.
    pub(super) fn start(worktree: &Path, record_id: &str, token: &str) -> io::Result<Self> {
        let mut child = gwt_core::process::hidden_command(std::env::current_exe()?)
            .arg(WATCHDOG_ARG)
            .arg(worktree)
            .arg(record_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let pipe = child.stdin.take();
        let mut watchdog = Self { child, pipe };
        let pipe = watchdog.pipe.as_mut().expect("piped stdin");
        pipe.write_all(token.as_bytes())?;
        pipe.write_all(b"\n")?;
        let mut ready = String::new();
        io::BufReader::new(watchdog.child.stdout.take().expect("piped stdout"))
            .read_line(&mut ready)?;
        if ready != READY {
            return Err(io::Error::other(
                "verification watchdog did not become ready",
            ));
        }
        Ok(watchdog)
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        if let Some(mut pipe) = self.pipe.take() {
            // A regular return (including an error) is distinguishable from
            // process death. A killed runner cannot send this byte.
            let _ = pipe.write_all(b"R");
        }
        let _ = self.child.wait();
    }
}

/// Private gwtd companion entrypoint, before the public operation dispatcher.
/// The pipe is inherited only by the runner that spawned this process.
pub fn run_watchdog(worktree: &Path, record_id: &str) -> io::Result<()> {
    let mut input = io::BufReader::new(io::stdin());
    let mut token = String::new();
    // Bound an invalid private invocation before accepting any work.
    input.by_ref().take(128).read_line(&mut token)?;
    let token = token.trim_end();
    if token.len() != 32 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid watchdog token",
        ));
    }
    io::stdout().write_all(READY.as_bytes())?;
    io::stdout().flush()?;
    let mut returned = [0];
    let normal_return = input.read(&mut returned)? != 0;
    settle_interrupted(worktree, record_id, token, normal_return)
}

fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn settle_interrupted(
    worktree: &Path,
    record_id: &str,
    token: &str,
    normal_return: bool,
) -> io::Result<()> {
    crate::cli::trusted_store::with_write_lease(worktree, || {
        let Some(mut record) = load(worktree)? else {
            return Ok(());
        };
        if record.record_id != record_id || !integrity_ok(&record) {
            return Ok(());
        }
        let Some(lifecycle) = record.lifecycle.as_mut() else {
            return Ok(());
        };
        if lifecycle.status != RunStatus::Running
            || lifecycle.watchdog_token_hash != token_hash(token)
        {
            return Ok(());
        }
        lifecycle.status = RunStatus::Interrupted;
        lifecycle.reason = Some(if normal_return {
            "verification runner returned without a terminal result; rerun required"
        } else {
            "external termination: runner pipe closed before a terminal result; signal unknown; rerun required"
        }.to_string());
        record.all_passed = false;
        record.plan_covered = false;
        record.created_at = chrono::Utc::now();
        save(worktree, &record)
    })
}

/// Call under the trusted write lease. Light runs can overlap, so an earlier
/// runner must not overwrite a newer run, even when both belong to one session.
pub(super) fn ensure_current(worktree: &Path, record_id: &str) -> io::Result<()> {
    match load(worktree)? {
        Some(record)
            if record.record_id == record_id
                && integrity_ok(&record)
                && record.lifecycle.as_ref().is_some_and(|lifecycle| {
                    lifecycle.status == RunStatus::Running
                }) => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "verification record was replaced or interrupted while this run was active; rerun verify.run",
        )),
    }
}

pub(super) fn checkpoint(worktree: &Path, record: &VerificationRunRecord) -> io::Result<()> {
    crate::cli::trusted_store::with_write_lease(worktree, || {
        ensure_current(worktree, &record.record_id)?;
        save(worktree, record)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_only_settles_its_authenticated_unfinished_record() {
        let dir = tempfile::tempdir().unwrap();
        let token = "0123456789abcdef0123456789abcdef";
        let mut record = super::super::tests::passing_record("sess-1", "no-git");
        record.lifecycle = Some(RunLifecycle::running(token));
        record.all_passed = false;
        record.plan_covered = false;
        save(dir.path(), &record).unwrap();
        let initial = load(dir.path()).unwrap().unwrap();
        settle_interrupted(dir.path(), &record.record_id, "wrong token", false).unwrap();
        settle_interrupted(dir.path(), "older-run", token, false).unwrap();
        assert_eq!(load(dir.path()).unwrap().unwrap(), initial);

        settle_interrupted(dir.path(), &record.record_id, token, false).unwrap();
        let interrupted = load(dir.path()).unwrap().unwrap();
        assert_eq!(
            interrupted.lifecycle.as_ref().unwrap().status,
            RunStatus::Interrupted
        );
        assert!(!interrupted.all_passed && !interrupted.plan_covered);
        assert!(integrity_ok(&interrupted));

        record.lifecycle = None;
        record.all_passed = true;
        save(dir.path(), &record).unwrap();
        let completed = load(dir.path()).unwrap().unwrap();
        settle_interrupted(dir.path(), &record.record_id, token, false).unwrap();
        assert_eq!(load(dir.path()).unwrap().unwrap(), completed);
    }
}
