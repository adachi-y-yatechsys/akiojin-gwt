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
        temp.path().join("stop.sock").to_string_lossy().into_owned(),
        "temporary-test-token".into(),
        "test".into(),
    );
    let mut listener = IpcListener::bind(std::path::Path::new(&endpoint.bind)).unwrap();
    let shutdown = Arc::new(DaemonShutdown::new());
    let server_shutdown = shutdown.clone();
    let server_endpoint = Arc::new(endpoint.clone());
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async {
            let stream = listener.accept().await.unwrap();
            let guard = ConnectionGuard::new(Arc::new(AtomicUsize::new(0)));
            super::handle_connection(
                stream,
                server_endpoint,
                BroadcastHub::new(),
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
