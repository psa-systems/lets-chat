#![cfg(feature = "standalone")]
//! LC-1208: every WebSocket connection end logs one `ws connection closed`
//! line naming why it ended. Production "kicked out" drops were silent: the
//! read loop left without a log line on a client close frame, a dropped
//! stream or a socket error, so a log search found nothing. These tests drive
//! a real socket against the served router and read the captured log.
//!
//! Its own binary (not a module of `it`) because it installs a tracing
//! subscriber for the test thread.

use std::io::Write;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use futures::SinkExt;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

mod common;

/// A `MakeWriter` that appends every log line to a shared buffer.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
    fn closed_lines(&self) -> Vec<String> {
        self.text()
            .lines()
            .filter(|l| l.contains("ws connection closed"))
            .map(str::to_owned)
            .collect()
    }
}

/// Serve the real router on a loopback port and return its address plus a
/// session cookie for a fresh user.
async fn serve() -> (SocketAddr, String) {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let user_id = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: Arc::new(lets_chat::push::MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
        bunyip_sso: None,
        stt_client: None,
        llm_client: None,
        embedding_client: None,
    };
    let app = routes::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, session)
}

async fn open(
    addr: SocketAddr,
    session: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = format!("ws://{addr}/ws").into_client_request().unwrap();
    request
        .headers_mut()
        .insert("cookie", format!("session={session}").parse().unwrap());
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("the websocket upgrade succeeds for a signed-in user");
    socket
}

/// Wait until `n` close lines are in the log, or fail after a few seconds.
async fn wait_for_closes(log: &Captured, n: usize) -> Vec<String> {
    for _ in 0..100 {
        let lines = log.closed_lines();
        if lines.len() >= n {
            return lines;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!(
        "expected {n} `ws connection closed` lines, log was:\n{}",
        log.text()
    );
}

#[tokio::test]
async fn a_client_watchdog_close_and_a_dropped_stream_each_log_why_they_ended() {
    let log = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let (addr, session) = serve().await;

    // layout.html's 60s watchdog closes the socket with 4000 `lc-half-open`.
    let mut watchdog = open(addr, &session).await;
    watchdog
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::Library(4000),
            reason: "lc-half-open".into(),
        })))
        .await
        .unwrap();
    let lines = wait_for_closes(&log, 1).await;
    let line = &lines[0];
    assert!(line.contains("reason=client sent close"), "{line}");
    assert!(line.contains("close_code=Some(4000)"), "{line}");
    assert!(line.contains("close_reason=lc-half-open"), "{line}");
    assert!(line.contains("frames_in=1"), "{line}");
    // Nothing was sent to this brand-new connection before it closed.
    assert!(line.contains("last_out_s=-1"), "{line}");

    // A proxy or network drop ends the TCP stream with no close frame.
    let dropped = open(addr, &session).await;
    drop(dropped);
    let lines = wait_for_closes(&log, 2).await;
    let line = &lines[1];
    assert!(
        line.contains("reason=stream ended without a close frame")
            || line.contains("reason=socket error"),
        "{line}"
    );
    assert!(line.contains("close_code=None"), "{line}");
}
