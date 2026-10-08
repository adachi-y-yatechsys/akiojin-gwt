//! Explicit-start entry points. The diagnostic entry creates transport only:
//! no scans, GitHub calls, artifact GC, verification children or materializers.
//! The normal worker entry disables automatic branch/cache cleanup only.
use super::{client::DaemonClient, config_error};
use gwt_core::daemon::{ClientFrame, DaemonFrame, RuntimeScope, RuntimeTarget};
use gwt_github::SpecOpsError;
use std::{path::Path, time::Duration};

pub(super) fn scope(project_root: &Path) -> Result<RuntimeScope, SpecOpsError> {
    if !project_root.is_absolute() {
        return Err(config_error(
            "daemon probe requires an absolute project_root",
        ));
    }
    let root = dunce::canonicalize(project_root)
        .map_err(|_| config_error("daemon probe project_root unavailable"))?;
    if !root.is_dir() {
        return Err(config_error("daemon probe project_root is not a directory"));
    }
    let root = gwt_core::paths::resolve_current_worktree_root(&root);
    RuntimeScope::from_project_root(&root, RuntimeTarget::Host)
        .map_err(|_| config_error("daemon probe project scope unavailable"))
}

pub(super) fn require_empty_endpoint_directory(dir: &Path) -> Result<(), SpecOpsError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(config_error("daemon probe endpoint directory unreadable")),
    };
    for entry in entries {
        let entry = entry.map_err(|_| config_error("daemon probe endpoint entry unreadable"))?;
        // Supervisor stderr logs are history, not endpoint descriptors.
        // Accept only the existing exact regular-file naming convention;
        // preserve it without reading, following a link, or rewriting it.
        let name = entry.file_name();
        let text = name.to_string_lossy();
        let known_log = text.strip_suffix(".daemon-stderr.log").is_some_and(|stem| {
            stem.len() == 16
                && stem
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        });
        if known_log
            && entry
                .file_type()
                .map_err(|_| config_error("daemon probe entry type unreadable"))?
                .is_file()
        {
            continue;
        }
        // Descriptors, sockets and unknown objects belong to their owner.
        // Refuse before bind rather than cleaning or replacing them.
        return Err(config_error(format!(
            "daemon probe requires an empty endpoint directory; existing entry {} is preserved",
            entry.file_name().to_string_lossy()
        )));
    }
    Ok(())
}

pub(super) fn start<W: std::io::Write + ?Sized>(
    project_root: &Path,
    writer: &mut W,
) -> Result<i32, SpecOpsError> {
    let scope = scope(project_root)?;
    let home = gwt_core::paths::gwt_home();
    require_empty_endpoint_directory(&scope.daemon_dir(&home))?;
    let endpoint_path = scope.endpoint_path(&home);
    super::server::serve_blocking_with_mode(scope, endpoint_path, writer, true)
}

pub(super) fn start_without_cleanup<W: std::io::Write + ?Sized>(
    project_root: &Path,
    writer: &mut W,
) -> Result<i32, SpecOpsError> {
    let scope = scope(project_root)?;
    let home = gwt_core::paths::gwt_home();
    require_empty_endpoint_directory(&scope.daemon_dir(&home))?;
    let endpoint_path = scope.endpoint_path(&home);
    super::server::serve_blocking_with_options(scope, endpoint_path, writer, false, false)
}

pub(super) fn status(
    project_root: &Path,
    pid: u32,
    instance_id: &str,
    out: &mut String,
) -> Result<i32, SpecOpsError> {
    if !gwt_core::daemon::is_valid_daemon_stop_identity(pid, instance_id, "probe-status") {
        return Err(config_error(
            "daemon probe status requires an exact PID and instance identity",
        ));
    }
    let scope = scope(project_root)?;
    let endpoint =
        super::stop::exact_endpoint(&scope, &gwt_core::paths::gwt_home(), pid, instance_id)?;
    if !endpoint.diagnostic_only {
        return Err(config_error(
            "daemon probe status refuses a normal work daemon",
        ));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| config_error("daemon probe status runtime unavailable"))?;
    let snapshot = runtime
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let mut client = DaemonClient::connect(&endpoint).await?;
                client.send_frame(&ClientFrame::Status).await?;
                match client.read_frame().await? {
                    DaemonFrame::Status(s)
                        if s.diagnostic_only
                            && s.issue_monitor.is_none()
                            && s.protocol_version == endpoint.protocol_version
                            && s.daemon_version == endpoint.daemon_version =>
                    {
                        Ok(s)
                    }
                    _ => Err("diagnostic-only status not confirmed".to_owned()),
                }
            })
            .await
            .map_err(|_| "diagnostic-only status timed out".to_owned())?
        })
        .map_err(|_| config_error("daemon probe status not confirmed; do not retry"))?;
    *out = serde_json::json!({"status":"diagnostic_only", "pid":pid,
        "instance_id":instance_id, "scope":scope, "snapshot":snapshot,
        "normal_worker_verified":false})
    .to_string();
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_endpoint_and_unknown_entries_are_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        require_empty_endpoint_directory(tmp.path()).unwrap();
        let log = tmp.path().join("abcdef0123456789.daemon-stderr.log");
        std::fs::write(&log, b"preserved old stderr").unwrap();
        require_empty_endpoint_directory(tmp.path()).unwrap();
        for name in ["old.json", "unknown-entry"] {
            let path = tmp.path().join(name);
            std::fs::write(&path, b"unchanged").unwrap();
            assert!(require_empty_endpoint_directory(tmp.path()).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), b"unchanged");
            std::fs::remove_file(path).unwrap();
        }
        assert_eq!(std::fs::read(&log).unwrap(), b"preserved old stderr");
    }
}
