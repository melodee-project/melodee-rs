//! Optional pre-accept connection admission for the HTTP listener.
//!
//! HTTP/1 handles requests serially on a connection. Retaining one permit
//! until the socket is dropped bounds accepted HTTP/1 connections, including
//! handlers waiting for a contended scrobble. Waiting happens before `accept`,
//! so the operating system applies its TCP backlog instead of retaining an
//! unbounded application-side semaphore queue. This is not an HTTP/2 stream
//! limit; it must be revisited if the server enables HTTP/2.

use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use axum::serve::Listener;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore},
};

pub(crate) const ENV_MAX_CONNECTIONS: &str = "MELODEE__SERVER__MAX_CONNECTIONS";
const MAX_CONNECTIONS: i64 = 65_536;

/// Zero leaves admission disabled; no measured general-purpose default exists.
pub(crate) fn parse_limit(value: &str) -> Result<Option<usize>, &'static str> {
    let count = value
        .parse::<i64>()
        .map_err(|_| "MELODEE__SERVER__MAX_CONNECTIONS must be an integer from 0 to 65536")?;
    if !(0..=MAX_CONNECTIONS).contains(&count) {
        return Err("MELODEE__SERVER__MAX_CONNECTIONS must be between 0 and 65536");
    }
    let count = usize::try_from(count)
        .map_err(|_| "MELODEE__SERVER__MAX_CONNECTIONS must be between 0 and 65536")?;
    Ok((count != 0).then_some(count))
}

pub(crate) struct ConnectionListener {
    listener: TcpListener,
    permits: Option<Arc<Semaphore>>,
}

impl ConnectionListener {
    pub(crate) fn new(listener: TcpListener, limit: Option<usize>) -> Self {
        Self {
            listener,
            permits: limit.map(|limit| Arc::new(Semaphore::new(limit))),
        }
    }
}

/// A permit belongs to the actual connection, not an individual request or
/// response body, including while streaming or waiting for a slow client.
pub(crate) struct ConnectionIo {
    stream: TcpStream,
    _permit: Option<OwnedSemaphorePermit>,
}

impl Listener for ConnectionListener {
    type Io = ConnectionIo;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let permit = match &self.permits {
            // No code closes this semaphore; if that invariant changes, never
            // silently accept an unadmitted connection.
            Some(permits) => Some(match permits.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(error) => {
                    tracing::error!(%error, "HTTP connection admission closed");
                    std::future::pending::<OwnedSemaphorePermit>().await
                }
            }),
            None => None,
        };
        let (stream, address) = Listener::accept(&mut self.listener).await;
        (
            ConnectionIo {
                stream,
                _permit: permit,
            },
            address,
        )
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

impl AsyncRead for ConnectionIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for ConnectionIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write_vectored(cx, bufs)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use std::{net::SocketAddr, sync::Arc, time::Duration};

    use axum::{
        Router,
        extract::{ConnectInfo, State},
        http::StatusCode,
        routing::{get, post},
        serve::{Listener, ListenerExt as _},
    };
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream},
        sync::{Notify, mpsc, oneshot},
        time::timeout,
    };

    use super::{ConnectionListener, parse_limit};

    #[test]
    fn limit_validation_is_strict() {
        assert_eq!(parse_limit("0"), Ok(None));
        assert_eq!(parse_limit("-0"), Ok(None));
        assert_eq!(parse_limit("1"), Ok(Some(1)));
        assert_eq!(parse_limit("65536"), Ok(Some(65_536)));
        for invalid in ["", " 1", "-1", "65537", "unlimited"] {
            assert!(parse_limit(invalid).is_err());
        }
    }

    #[test]
    fn stored_startup_value_reaches_listener_configuration() {
        let values = [(super::ENV_MAX_CONNECTIONS.to_owned(), "19".to_owned())]
            .into_iter()
            .collect();
        melodee_persistence::runtime_settings::preview(values, || {
            let configured =
                melodee_persistence::runtime_settings::var(super::ENV_MAX_CONNECTIONS).unwrap();
            assert_eq!(parse_limit(&configured), Ok(Some(19)));
        });
    }

    #[tokio::test]
    async fn capacity_is_held_until_connection_drop_and_accept_is_cancellable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut listener = ConnectionListener::new(listener, Some(1));
        let mut client = TcpStream::connect(address).await.unwrap();
        let (mut first, _) = listener.accept().await;
        let _second_client = TcpStream::connect(address).await.unwrap();
        assert!(
            timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );

        // The active stream remains usable while further accepts are held.
        client.write_all(b"x").await.unwrap();
        let mut received = [0];
        first.read_exact(&mut received).await.unwrap();
        assert_eq!(received, *b"x");
        drop(first);
        let (_second, _) = timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("permit was not returned on disconnect");
    }

    #[derive(Clone)]
    struct HeldRequest {
        entered: mpsc::UnboundedSender<SocketAddr>,
        release: Arc<Notify>,
    }

    async fn delayed_play(
        State(state): State<HeldRequest>,
        ConnectInfo(peer): ConnectInfo<SocketAddr>,
    ) -> StatusCode {
        state.entered.send(peer).unwrap();
        state.release.notified().await;
        StatusCode::NO_CONTENT
    }

    #[tokio::test]
    async fn http1_backpressure_waits_for_success_and_keeps_real_peer() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let listener = ConnectionListener::new(listener, Some(1)).tap_io(|_| {});
        let (entered_tx, mut entered_rx) = mpsc::unbounded_channel();
        let release = Arc::new(Notify::new());
        let app = Router::new()
            .route("/Sessions/Playing/Stopped", post(delayed_play))
            .with_state(HeldRequest {
                entered: entered_tx,
                release: release.clone(),
            });
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
        });

        let request = b"POST /Sessions/Playing/Stopped HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let mut first = TcpStream::connect(address).await.unwrap();
        let first_peer = first.local_addr().unwrap();
        first.write_all(request).await.unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), entered_rx.recv())
                .await
                .unwrap(),
            Some(first_peer)
        );

        let mut second = TcpStream::connect(address).await.unwrap();
        let second_peer = second.local_addr().unwrap();
        second.write_all(request).await.unwrap();
        assert!(
            timeout(Duration::from_millis(100), entered_rx.recv())
                .await
                .is_err()
        );
        let mut byte = [0];
        assert!(
            timeout(Duration::from_millis(100), second.read(&mut byte))
                .await
                .is_err()
        );

        // No 204 is sent until the handler has completed its work.
        assert!(
            timeout(Duration::from_millis(100), first.read(&mut byte))
                .await
                .is_err()
        );
        release.notify_one();
        let mut response = Vec::new();
        timeout(Duration::from_secs(2), first.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 204"));
        assert_eq!(
            timeout(Duration::from_secs(2), entered_rx.recv())
                .await
                .unwrap(),
            Some(second_peer)
        );
        release.notify_one();
        response.clear();
        timeout(Duration::from_secs(2), second.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 204"));
        shutdown_tx.send(()).unwrap();
        timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn http2_prior_knowledge_is_not_accepted_by_the_current_server() {
        // A per-connection limit does not bound multiplexed HTTP/2 streams.
        // Keep this guard until explicit per-stream admission is implemented.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let listener = ConnectionListener::new(listener, Some(1)).tap_io(|_| {});
        let app = Router::new().route("/", get(|| async { "ok" }));
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
        });
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n\x00\x00\x00\x04\x00\x00\x00\x00\x00")
            .await
            .unwrap();
        let mut response = [0_u8; 64];
        let count = timeout(Duration::from_secs(2), client.read(&mut response))
            .await
            .unwrap()
            .unwrap_or(0);
        assert!(
            count == 0 || response[..count].starts_with(b"HTTP/1."),
            "HTTP/2 is enabled; per-stream admission is required"
        );
        drop(client);
        shutdown_tx.send(()).unwrap();
        timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
