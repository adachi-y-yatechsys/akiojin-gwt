//! Temporary transport tests only: no daemon worker, configuration, GUI or AI.
use super::{BroadcastHub, ConnectionGuard, DaemonShutdown};
use crate::cli::daemon::{client::DaemonClient, transport::IpcListener};
use gwt_core::daemon::{ClientFrame, DaemonEndpoint, DaemonFrame, RuntimeScope, RuntimeTarget};
use std::{
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tokio::{io::AsyncWrite, task::JoinHandle};

async fn fixture() -> (TempDir, DaemonEndpoint, Arc<DaemonShutdown>, JoinHandle<()>) {
    fixture_with_mode(false).await
}

async fn fixture_with_mode(
    diagnostic_only: bool,
) -> (TempDir, DaemonEndpoint, Arc<DaemonShutdown>, JoinHandle<()>) {
    let temp = TempDir::new().unwrap();
    let scope = RuntimeScope::new(
        "abcdef0123456789",
        "feedfacecafebeef",
        temp.path().to_owned(),
        RuntimeTarget::Host,
    )
    .unwrap();
    let mut endpoint = DaemonEndpoint::new(
        scope,
        std::process::id(),
        temp.path().join("stop.sock").to_string_lossy().into_owned(),
        "temporary-test-token".into(),
        "test".into(),
    );
    endpoint.diagnostic_only = diagnostic_only;
    endpoint.background_cleanup_allowed = !diagnostic_only;
    let mut listener = IpcListener::bind(std::path::Path::new(&endpoint.bind)).unwrap();
    let shutdown = Arc::new(DaemonShutdown::new());
    let server_shutdown = shutdown.clone();
    let server_endpoint = Arc::new(endpoint.clone());
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async {
            let stream = listener.accept().await.unwrap();
            let guard = ConnectionGuard::new(Arc::new(AtomicUsize::new(0)));
            let hub = BroadcastHub::new();
            if diagnostic_only {
                hub.close_issue_monitor_controls();
            }
            super::handle_connection(
                stream,
                server_endpoint,
                hub,
                Instant::now(),
                &guard,
                server_shutdown,
            )
            .await
            .unwrap();
        })
        .await
        .expect("temporary connection finishes");
    });
    (temp, endpoint, shutdown, server)
}

#[tokio::test]
async fn diagnostic_transport_refuses_every_effectful_frame_before_dispatch() {
    use gwt_core::daemon::{HookEnvelope, VerificationSpawnRequest, DAEMON_PROTOCOL_VERSION};
    for case in 0..5 {
        let (_temp, endpoint, shutdown, server) = fixture_with_mode(true).await;
        let frame = match case {
            0 => ClientFrame::Subscribe {
                channels: vec!["board".into()],
            },
            1 => ClientFrame::SubscribeMaterializer {
                channels: vec!["issue-monitor".into()],
            },
            2 => ClientFrame::Publish {
                channel: "issue-monitor-control".into(),
                payload: serde_json::json!({"operation":"launch"}),
            },
            3 => ClientFrame::Hook(HookEnvelope {
                protocol_version: DAEMON_PROTOCOL_VERSION,
                scope: endpoint.scope.clone(),
                hook_name: "SessionStart".into(),
                session_id: None,
                cwd: endpoint.scope.project_root.clone(),
                payload: serde_json::json!({}),
            }),
            _ => ClientFrame::SpawnVerification(VerificationSpawnRequest {
                program: "must-not-run".into(),
                args: vec![],
                cwd: endpoint.scope.project_root.clone(),
                env: vec![],
                stdout_path: endpoint.scope.project_root.join("must-not-create.out"),
                stderr_path: endpoint.scope.project_root.join("must-not-create.err"),
            }),
        };
        let mut client = DaemonClient::connect(&endpoint).await.unwrap();
        client.send_frame(&frame).await.unwrap();
        assert!(matches!(client.read_frame::<DaemonFrame>().await.unwrap(),
            DaemonFrame::Error{message} if message=="diagnostic-only daemon admits Status and exact Stop only"));
        server.await.unwrap();
        assert!(!shutdown.requested.load(Ordering::Acquire));
        assert!(!endpoint
            .scope
            .project_root
            .join("must-not-create.out")
            .exists());
        assert!(!endpoint
            .scope
            .project_root
            .join("must-not-create.err")
            .exists());
    }
}

#[tokio::test]
async fn diagnostic_server_status_and_stop_never_load_monitor_or_create_its_fence() {
    use gwt_core::test_support::ScopedGwtHome;
    let temp = TempDir::new().unwrap();
    let _home = ScopedGwtHome::set(temp.path());
    let scope = RuntimeScope::new(
        "probe-repo",
        "probe-worktree",
        temp.path().to_owned(),
        RuntimeTarget::Host,
    )
    .unwrap();
    let prefs = crate::issue_monitor_prefs_path_for_repo_path(&scope.project_root);
    std::fs::create_dir_all(prefs.parent().unwrap()).unwrap();
    std::fs::write(&prefs, b"unreadable monitor bytes: must remain untouched").unwrap();
    let lease = crate::issue_monitor::acquire_issue_monitor_daemon_lease(&prefs).unwrap();
    let path = scope.endpoint_path(temp.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let socket = path.with_extension("sock");
    let mut endpoint = DaemonEndpoint::new(
        scope,
        std::process::id(),
        socket.to_string_lossy().into_owned(),
        "probe-test-token".into(),
        "test".into(),
    );
    endpoint.diagnostic_only = true;
    endpoint.background_cleanup_allowed = false;
    let bound = super::bind_daemon(&endpoint, &socket, &path, lease).unwrap();
    let shutdown = Arc::new(DaemonShutdown::new());
    let server = tokio::spawn(super::run_bound_server(
        endpoint.clone(),
        path,
        BroadcastHub::new(),
        shutdown,
        crate::IssueMonitorConfig::default(),
        Duration::from_secs(1),
        bound,
    ));
    let mut client = DaemonClient::connect(&endpoint).await.unwrap();
    client.send_frame(&ClientFrame::Status).await.unwrap();
    assert!(matches!(client.read_frame::<DaemonFrame>().await.unwrap(),
        DaemonFrame::Status(s) if s.diagnostic_only && !s.background_cleanup_allowed && s.issue_monitor.is_none()));
    drop(client);
    crate::cli::daemon::stop::request_stop(&endpoint, "probe-stop-test")
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        0
    );
    assert_eq!(
        std::fs::read(&prefs).unwrap(),
        b"unreadable monitor bytes: must remain untouched"
    );
    assert!(!crate::issue_monitor::issue_monitor_authority_fence_path(&prefs).exists());
    // A new lock holder proves the original project lifetime lease was released.
    drop(crate::issue_monitor::acquire_issue_monitor_daemon_lease(&prefs).unwrap());
}

#[test]
fn diagnostic_mode_survives_descriptor_repair_and_corrects_a_lost_mode_flag() {
    let temp = TempDir::new().unwrap();
    let scope = RuntimeScope::new(
        "probe-repo",
        "probe-worktree",
        temp.path().to_owned(),
        RuntimeTarget::Host,
    )
    .unwrap();
    let mut endpoint = DaemonEndpoint::new(
        scope,
        std::process::id(),
        "test-bind".into(),
        "probe-test-token".into(),
        "test".into(),
    );
    endpoint.diagnostic_only = true;
    endpoint.background_cleanup_allowed = false;
    let path = temp.path().join("endpoint.json");
    assert_eq!(
        super::heal_endpoint_descriptor(&endpoint, &path, &|_| false),
        super::EndpointDescriptorHeal::Rewritten
    );
    assert!(
        gwt_core::daemon::load_endpoint(&path)
            .unwrap()
            .diagnostic_only
    );
    let mut lost = endpoint.clone();
    assert!(
        !gwt_core::daemon::load_endpoint(&path)
            .unwrap()
            .background_cleanup_allowed
    );
    lost.diagnostic_only = false;
    lost.background_cleanup_allowed = true;
    gwt_core::daemon::persist_endpoint(&path, &lost).unwrap();
    assert_eq!(
        super::heal_endpoint_descriptor(&endpoint, &path, &|_| true),
        super::EndpointDescriptorHeal::Rewritten
    );
    assert!(
        gwt_core::daemon::load_endpoint(&path)
            .unwrap()
            .diagnostic_only
    );
    assert!(
        !gwt_core::daemon::load_endpoint(&path)
            .unwrap()
            .background_cleanup_allowed
    );
}

fn frame(endpoint: &DaemonEndpoint) -> ClientFrame {
    ClientFrame::Stop {
        expected_scope: endpoint.scope.clone(),
        expected_pid: endpoint.pid,
        expected_instance_id: endpoint.instance_id(),
        request_id: "stop-trial-1".into(),
    }
}

#[tokio::test]
async fn exact_authenticated_stop_receives_receipt_and_requests_shutdown() {
    let (_temp, endpoint, shutdown, server) = fixture().await;
    let mut client = DaemonClient::connect(&endpoint).await.unwrap();
    client.send_frame(&frame(&endpoint)).await.unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(10), client.read_frame::<DaemonFrame>())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt,
        DaemonFrame::StopAccepted {
            pid: endpoint.pid,
            instance_id: endpoint.instance_id(),
            request_id: "stop-trial-1".into()
        }
    );
    server.await.unwrap();
    assert!(shutdown.requested.load(Ordering::Acquire));
}

#[tokio::test]
async fn wrong_pid_instance_scope_or_request_refuses_without_shutdown() {
    for case in 0..5 {
        let (_temp, endpoint, shutdown, server) = fixture().await;
        let mut request = frame(&endpoint);
        if let ClientFrame::Stop {
            expected_pid,
            expected_instance_id,
            expected_scope,
            request_id,
        } = &mut request
        {
            match case {
                0 => *expected_pid += 1,
                1 => *expected_instance_id = "0".repeat(64),
                2 => {
                    *expected_scope = RuntimeScope::new(
                        "other-project",
                        "other-worktree",
                        endpoint.scope.project_root.clone(),
                        RuntimeTarget::Host,
                    )
                    .unwrap()
                }
                3 => *request_id = String::new(),
                _ => *request_id = "x".repeat(129),
            }
        }
        let mut client = DaemonClient::connect(&endpoint).await.unwrap();
        client.send_frame(&request).await.unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(10), client.read_frame::<DaemonFrame>())
                .await
                .unwrap()
                .unwrap(),
            DaemonFrame::Error { .. }
        ));
        server.await.unwrap();
        assert!(!shutdown.requested.load(Ordering::Acquire));
    }
}

#[tokio::test]
async fn a_subscription_connection_cannot_stop_the_daemon() {
    let (_temp, endpoint, shutdown, server) = fixture().await;
    let mut client = DaemonClient::connect(&endpoint).await.unwrap();
    client
        .send_frame(&ClientFrame::Subscribe {
            channels: vec!["unused-test-channel".into()],
        })
        .await
        .unwrap();
    assert_eq!(
        client.read_frame::<DaemonFrame>().await.unwrap(),
        DaemonFrame::Ack
    );
    client.send_frame(&frame(&endpoint)).await.unwrap();
    assert!(matches!(
        client.read_frame::<DaemonFrame>().await.unwrap(),
        DaemonFrame::Error { .. }
    ));
    server.await.unwrap();
    assert!(!shutdown.requested.load(Ordering::Acquire));
}

#[tokio::test]
async fn wrong_authentication_never_reaches_stop() {
    let (_temp, endpoint, shutdown, server) = fixture().await;
    let mut impostor = endpoint.clone();
    impostor.auth_token = "wrong-test-token".into();
    assert!(DaemonClient::connect(&impostor).await.is_err());
    server.await.unwrap();
    assert!(!shutdown.requested.load(Ordering::Acquire));
}

struct ReceiptWriter {
    shutdown: Arc<DaemonShutdown>,
    bytes: Vec<u8>,
    flushed: bool,
    fail_flush: bool,
}
impl AsyncWrite for ReceiptWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        assert!(
            !self.shutdown.requested.load(Ordering::Acquire),
            "shutdown preceded receipt write"
        );
        self.bytes.extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        assert!(
            !self.shutdown.requested.load(Ordering::Acquire),
            "shutdown preceded receipt flush"
        );
        self.flushed = true;
        if self.fail_flush {
            Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
        } else {
            Poll::Ready(Ok(()))
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn shutdown_follows_receipt_flush_and_survives_receipt_failure() {
    for fail_flush in [false, true] {
        let shutdown = Arc::new(DaemonShutdown::new());
        let mut writer = ReceiptWriter {
            shutdown: shutdown.clone(),
            bytes: vec![],
            flushed: false,
            fail_flush,
        };
        let receipt = DaemonFrame::StopAccepted {
            pid: 1,
            instance_id: "0".repeat(64),
            request_id: "one".into(),
        };
        super::write_stop_receipt(&mut writer, &receipt, &shutdown).await;
        assert!(writer.flushed);
        assert_eq!(
            serde_json::from_slice::<DaemonFrame>(&writer.bytes).unwrap(),
            receipt
        );
        assert!(shutdown.requested.load(Ordering::Acquire));
    }
}

#[tokio::test(start_paused = true)]
async fn blocked_receipt_writer_cannot_prevent_an_accepted_shutdown() {
    let shutdown = DaemonShutdown::new();
    let (mut writer, _unread_peer) = tokio::io::duplex(1);
    let receipt = DaemonFrame::StopAccepted {
        pid: 1,
        instance_id: "0".repeat(64),
        request_id: "one".into(),
    };
    let start = tokio::time::Instant::now();
    super::write_stop_receipt(&mut writer, &receipt, &shutdown).await;
    assert_eq!(
        start.elapsed(),
        crate::cli::daemon::stop::STOP_CONTACT_TIMEOUT
    );
    assert!(shutdown.requested.load(Ordering::Acquire));
}
