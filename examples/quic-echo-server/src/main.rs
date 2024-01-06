use std::{net::SocketAddr, sync::Arc};

use quinn::{Endpoint, EndpointConfig, ServerConfig};

use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let addr = "127.0.0.1:4000".parse().unwrap();
    if let Err(err) = run(addr).await {
        error!(?err, "QUIC server failed")
    }
}

/// Runs a QUIC server bound to given address.
async fn run(addr: SocketAddr) -> anyhow::Result<()> {
    let endpoint = create_endpoint(addr)?;
    info!("QUIC server listening on {addr}");
    while let Some(conn) = endpoint.accept().await {
        tokio::task::spawn(async move {
            if let Err(err) = handle_conn(conn).await {
                error!("Conn failed: {err:?}");
            }
        });
    }
    Ok(())
}

pub fn create_endpoint(addr: SocketAddr) -> anyhow::Result<Endpoint> {
    let socket = std::net::UdpSocket::bind(addr)?;
    let runtime = quinn::default_runtime().unwrap();
    let config = EndpointConfig::default();
    let (server_config, _certs) = configure_server()?;
    let endpoint = quinn::Endpoint::new(config, Some(server_config), socket, runtime)?;
    Ok(endpoint)
}

async fn handle_conn(conn: quinn::Connecting) -> anyhow::Result<()> {
    let addr = conn.remote_address();
    info!(?addr, "incoming conn");
    let conn = conn.await?;
    info!(?addr, "incoming conn established",);
    let (mut send, mut recv) = conn.accept_bi().await?;
    info!(?addr, "bi stream accepted",);
    let msg = recv.read_to_end(1024).await?;
    let s = String::from_utf8(msg)?;
    info!(?addr, "recv: {s}");
    let s = format!(
        "I'm a quic server and return what you said in uppercase: {}",
        s.to_uppercase()
    );
    info!(?addr, "send: {s}");
    send.write_all(s.as_bytes()).await?;
    send.finish().await?;
    info!(?addr, "close");
    Ok(())
}

/// Returns default server configuration along with its certificate.
fn configure_server() -> anyhow::Result<(ServerConfig, Vec<u8>)> {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_der = cert.serialize_der().unwrap();
    let priv_key = cert.serialize_private_key_der();
    let priv_key = rustls::PrivateKey(priv_key);
    let cert_chain = vec![rustls::Certificate(cert_der.clone())];

    let mut server_config = ServerConfig::with_single_cert(cert_chain, priv_key)?;
    let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
    transport_config.max_concurrent_uni_streams(0_u8.into());

    Ok((server_config, cert_der))
}
