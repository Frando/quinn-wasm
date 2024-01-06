use std::io::IoSliceMut;
use std::sync::Arc;

use axum::extract::connect_info::ConnectInfo;
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Extension, Router,
};
use axum_extra::TypedHeader;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use tower_http::trace::DefaultMakeSpan;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const DEFAULT_PORT: u16 = 3000;

type AllowedHosts = Option<Vec<SocketAddr>>;

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "info,example_static_file_server=debug,tower_http=debug".into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let port: u16 = match std::env::var("PORT") {
        Ok(port) => port.parse().expect("PORT must be a number"),
        Err(_) => DEFAULT_PORT,
    };
    let allowed_hosts: Option<Vec<SocketAddr>> = match std::env::var("ALLOWED_HOSTS") {
        Ok(hosts) => {
            let hosts = hosts.split(" ");
            let mut res = vec![];
            for host in hosts {
                res.push(host.parse().expect("ALLOWED_HOSTS must be a list of socket addresses (host:port) seperated by spaces"))
            }
            Some(res)
        }
        Err(_) => None,
    };
    if let Err(err) = serve(router(), port, allowed_hosts).await {
        error!("App crashed: {err:?}");
    }
}

fn router() -> Router {
    let dir = "../web";
    // `ServeDir` allows setting a fallback if an asset is not found
    // so with this `GET /assets/doesnt-exist.jpg` will return `index.html`
    // rather than a 404
    let serve_dir =
        ServeDir::new(dir).not_found_service(ServeFile::new(&format!("{dir}/index.html")));

    Router::new()
        .route("/quic", get(ws_handler))
        .fallback_service(serve_dir)
}

async fn serve(app: Router, port: u16, allowed_hosts: AllowedHosts) -> anyhow::Result<()> {
    let app = app.layer(
        TraceLayer::new_for_http().make_span_with(DefaultMakeSpan::default().include_headers(true)),
    );

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let quinn_rt = quinn::default_runtime().unwrap();
    let app = app
        .layer(Extension(quinn_rt))
        .layer(Extension(Arc::new(allowed_hosts)));
    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// The handler for the HTTP request (this gets called when the HTTP GET lands at the start
/// of websocket negotiation). After this completes, the actual switching from HTTP to
/// websocket protocol will occur.
/// This is the last point where we can extract TCP/IP metadata such as IP address of the client
/// as well as things from HTTP headers such as user-agent of the browser etc.
async fn ws_handler(
    ws: WebSocketUpgrade,
    user_agent: Option<TypedHeader<headers::UserAgent>>,
    Extension(quinn_rt): Extension<Arc<dyn quinn::Runtime>>,
    Extension(allowed_hosts): Extension<Arc<AllowedHosts>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let user_agent = if let Some(TypedHeader(user_agent)) = user_agent {
        user_agent.to_string()
    } else {
        String::from("Unknown browser")
    };
    println!("`{user_agent}` at {addr} connected.");
    // finalize the upgrade process by returning upgrade callback.
    // we can customize the callback by sending additional info such as address.
    ws.on_upgrade(move |socket| async move {
        if let Err(err) = handle_socket(quinn_rt, socket, addr, allowed_hosts).await {
            error!("websocket connection from {addr} failed: {err:?}");
        }
    })
}

/// Actual websocket statemachine (one will be spawned per connection)
async fn handle_socket(
    quinn_rt: Arc<dyn quinn::Runtime>,
    ws: WebSocket,
    who: SocketAddr,
    allowed_hosts: Arc<AllowedHosts>,
) -> anyhow::Result<()> {
    info!("new websocket connection from {who}");
    let udp_socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    let udp_socket = quinn_rt.wrap_udp_socket(udp_socket)?;
    const BATCH_SIZE: usize = 1;
    let (mut ws_send, mut ws_recv) = ws.split();

    let recv_task: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::task::spawn({
        let udp_socket = udp_socket.clone();

        let mut metas = [quinn_udp::RecvMeta::default(); BATCH_SIZE];
        async move {
            let mut recv_buf = vec![0u8; 1600];
            loop {
                let recv_slice = IoSliceMut::new(&mut recv_buf);
                let mut iovs = [recv_slice];
                let res =
                    std::future::poll_fn(|cx| udp_socket.poll_recv(cx, &mut iovs, &mut metas))
                        .await;
                let _count = res?;
                let meta = &metas[0];
                let buf = &recv_buf[..meta.len];
                let message = InboundMessage {
                    content: buf.to_vec().into(),
                    src: meta.addr,
                };
                info!(
                    "fwd to web: from {} len {}",
                    message.src,
                    message.content.len()
                );
                let msg = Message::Binary(postcard::to_stdvec(&message)?);
                ws_send.send(msg).await?;
            }
        }
    });
    // let mut state = quinn_udp::UdpState::default();
    while let Some(message) = ws_recv.next().await {
        let message = message?;
        let Message::Binary(message) = message else {
            anyhow::bail!("Expected binary messages only");
        };

        let message: OutboundMessage = postcard::from_bytes(&message)?;
        if let Some(hosts) = allowed_hosts.as_ref() {
            if !hosts.contains(&message.dst) {
                warn!("skip fwd to native: {} not allowed", message.dst,);
                continue;
            }
        }
        info!(
            "fwd to native: to {} len {}",
            message.dst,
            message.content.len()
        );
        let transmit = quinn_udp::Transmit {
            destination: message.dst,
            ecn: None,
            contents: message.content,
            segment_size: None,
            src_ip: None,
        };
        let transmits = Arc::new([transmit]);
        let udp_socket = udp_socket.clone();
        std::future::poll_fn(move |cx| udp_socket.poll_send(cx, transmits.as_ref())).await?;
    }
    recv_task.await??;
    Ok(())
}

#[derive(Serialize, Deserialize, Debug)]
pub struct OutboundMessage {
    pub dst: SocketAddr,
    pub content: Bytes,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct InboundMessage {
    pub src: SocketAddr,
    pub content: Bytes,
}
