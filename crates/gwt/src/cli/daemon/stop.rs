//! Exact-instance, JSON-only cooperative stop. Never bootstrap or retry.
use std::{path::Path, time::Duration};

use gwt_core::daemon::{
    is_valid_daemon_stop_identity, load_endpoint, ClientFrame, DaemonEndpoint, DaemonFrame,
    RuntimeScope, RuntimeTarget, DAEMON_PROTOCOL_VERSION,
};
use gwt_github::SpecOpsError;

use super::{client::DaemonClient, config_error};

pub(super) const STOP_CONTACT_TIMEOUT: Duration = Duration::from_secs(5);

fn exact_endpoint(
    scope: &RuntimeScope,
    home: &Path,
    pid: u32,
    instance_id: &str,
) -> Result<DaemonEndpoint, SpecOpsError> {
    // Do not use bootstrap resolution: a refusal must not delete a descriptor.
    let endpoint = load_endpoint(&scope.endpoint_path(home))
        .map_err(|_| config_error("daemon.stop exact endpoint is missing or unreadable"))?;
    if endpoint.scope != *scope
        || endpoint.protocol_version != DAEMON_PROTOCOL_VERSION
        || endpoint.pid != pid
        || endpoint.auth_token.trim().is_empty()
        || endpoint.bind.trim().is_empty()
        || endpoint.instance_id() != instance_id
    {
        return Err(config_error("daemon.stop exact endpoint identity mismatch"));
    }
    Ok(endpoint)
}

pub(super) async fn request_stop(
    endpoint: &DaemonEndpoint,
    request_id: &str,
) -> Result<(), String> {
    tokio::time::timeout(STOP_CONTACT_TIMEOUT, async {
        let mut client = DaemonClient::connect(endpoint).await?;
        client
            .send_frame(&ClientFrame::Stop {
                expected_scope: endpoint.scope.clone(),
                expected_pid: endpoint.pid,
                expected_instance_id: endpoint.instance_id(),
                request_id: request_id.to_owned(),
            })
            .await?;
        match client.read_frame().await? {
            DaemonFrame::StopAccepted {
                pid,
                instance_id,
                request_id: received_id,
            } if pid == endpoint.pid
                && instance_id == endpoint.instance_id()
                && received_id == request_id =>
            {
                Ok(())
            }
            _ => Err("daemon.stop exact acceptance receipt unavailable; outcome unknown".into()),
        }
    })
    .await
    .map_err(|_| "daemon.stop contact timed out; outcome unknown".to_owned())?
}

pub(super) fn run(
    project_root: &Path,
    pid: u32,
    instance_id: &str,
    request_id: &str,
    out: &mut String,
) -> Result<i32, SpecOpsError> {
    if !project_root.is_absolute() || !is_valid_daemon_stop_identity(pid, instance_id, request_id) {
        return Err(config_error(
            "daemon.stop requires an explicit absolute project and exact identity",
        ));
    }
    let root = dunce::canonicalize(project_root)
        .map_err(|_| config_error("daemon.stop project_root is unavailable"))?;
    if !root.is_dir() {
        return Err(config_error("daemon.stop project_root is not a directory"));
    }
    let root = gwt_core::paths::resolve_current_worktree_root(&root);
    let scope = RuntimeScope::from_project_root(&root, RuntimeTarget::Host)
        .map_err(|_| config_error("daemon.stop project scope unavailable"))?;
    let endpoint = exact_endpoint(&scope, &gwt_core::paths::gwt_home(), pid, instance_id)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| config_error(format!("daemon.stop runtime unavailable: {error}")))?;
    runtime
        .block_on(request_stop(&endpoint, request_id))
        .map_err(|_| {
            config_error("daemon.stop acceptance not confirmed; outcome unknown, do not resend")
        })?;
    *out = serde_json::json!({"status":"stop_accepted", "pid":pid, "instance_id":instance_id,
        "request_id":request_id, "process_exit_verified":false})
    .to_string();
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    fn fixture() -> (TempDir, RuntimeScope, DaemonEndpoint) {
        let temp = TempDir::new().unwrap();
        let scope = RuntimeScope::new(
            "abcdef0123456789",
            "feedfacecafebeef",
            temp.path().to_owned(),
            RuntimeTarget::Host,
        )
        .unwrap();
        let endpoint = DaemonEndpoint::new(
            scope.clone(),
            77,
            "test-bind".into(),
            "temporary-test-token".into(),
            "test".into(),
        );
        gwt_core::daemon::persist_endpoint(&scope.endpoint_path(temp.path()), &endpoint).unwrap();
        (temp, scope, endpoint)
    }
    #[test]
    fn stale_pid_and_instance_cannot_adopt_or_remove_another_endpoint() {
        let (temp, scope, endpoint) = fixture();
        let path = scope.endpoint_path(temp.path());
        let before = std::fs::read(&path).unwrap();
        assert!(exact_endpoint(&scope, temp.path(), 78, &endpoint.instance_id()).is_err());
        assert!(exact_endpoint(&scope, temp.path(), 77, &"0".repeat(64)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            exact_endpoint(&scope, temp.path(), 77, &endpoint.instance_id()).unwrap(),
            endpoint
        );
    }
    #[test]
    fn missing_corrupt_and_foreign_scope_are_read_only_refusals() {
        let (temp, scope, endpoint) = fixture();
        let path = scope.endpoint_path(temp.path());
        let mut foreign = endpoint.clone();
        foreign.scope = RuntimeScope::new(
            "different",
            "feedfacecafebeef",
            temp.path().to_owned(),
            RuntimeTarget::Host,
        )
        .unwrap();
        gwt_core::daemon::persist_endpoint(&path, &foreign).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(exact_endpoint(&scope, temp.path(), 77, &endpoint.instance_id()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::write(&path, b"not-json").unwrap();
        assert!(exact_endpoint(&scope, temp.path(), 77, &endpoint.instance_id()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"not-json");
        std::fs::remove_file(&path).unwrap();
        assert!(exact_endpoint(&scope, temp.path(), 77, &endpoint.instance_id()).is_err());
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn missing_or_mismatched_receipt_is_unknown_without_resending() {
        use crate::cli::daemon::server::build_handshake_response;
        use crate::cli::daemon::transport::IpcListener;
        use gwt_core::daemon::IpcHandshakeRequest;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        for case in 0..4 {
            let temp = TempDir::new().unwrap();
            let scope = RuntimeScope::new(
                "abcdef0123456789",
                "feedfacecafebeef",
                temp.path().to_owned(),
                RuntimeTarget::Host,
            )
            .unwrap();
            let endpoint = DaemonEndpoint::new(
                scope,
                std::process::id(),
                temp.path()
                    .join("receipt.sock")
                    .to_string_lossy()
                    .into_owned(),
                "temporary-test-token".into(),
                "test".into(),
            );
            let mut listener = IpcListener::bind(Path::new(&endpoint.bind)).unwrap();
            let server_endpoint = endpoint.clone();
            let server = tokio::spawn(async move {
                tokio::time::timeout(Duration::from_secs(10), async {
                    let stream = listener.accept().await.unwrap();
                    let (read, mut write) = stream.into_split();
                    let mut reader = BufReader::new(read);
                    let mut line = String::new();
                    reader.read_line(&mut line).await.unwrap();
                    let request: IpcHandshakeRequest = serde_json::from_str(&line).unwrap();
                    let reply = build_handshake_response(&server_endpoint, &request);
                    let bytes = format!("{}\n", serde_json::to_string(&reply).unwrap());
                    write.write_all(bytes.as_bytes()).await.unwrap();
                    line.clear();
                    reader.read_line(&mut line).await.unwrap();
                    assert!(matches!(
                        serde_json::from_str::<ClientFrame>(&line).unwrap(),
                        ClientFrame::Stop { .. }
                    ));
                    if case != 0 {
                        let receipt = DaemonFrame::StopAccepted {
                            pid: if case == 1 {
                                server_endpoint.pid + 1
                            } else {
                                server_endpoint.pid
                            },
                            instance_id: if case == 2 {
                                "0".repeat(64)
                            } else {
                                server_endpoint.instance_id()
                            },
                            request_id: if case == 3 {
                                "another-request"
                            } else {
                                "stop-1"
                            }
                            .into(),
                        };
                        write
                            .write_all(
                                format!("{}\n", serde_json::to_string(&receipt).unwrap())
                                    .as_bytes(),
                            )
                            .await
                            .unwrap();
                    }
                    // A resend would require another connection; this single
                    // connection listener is dropped without accepting one.
                })
                .await
                .unwrap();
            });
            assert!(request_stop(&endpoint, "stop-1").await.is_err());
            server.await.unwrap();
        }
    }
}
