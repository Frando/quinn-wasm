use std::{net::SocketAddr, sync::Arc};

use quinn_wasm::quinn::ClientConfig;
use tracing::{info};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    tracing_wasm::set_as_global_default();
    Ok(())
}

#[wasm_bindgen]
pub async fn connect_to_quic_demo(
    ws_relay_url: &str,
    quic_host: &str,
    message: &str,
) -> Result<String, AppError> {
    info!(?ws_relay_url, ?quic_host, ?message, "connect to quic!");
    let remote_addr: SocketAddr = quic_host.parse()?;
    let local_addr: SocketAddr = "127.0.0.1:9999".parse()?;
    let client_config = configure_client();
    let ep =
        quinn_wasm::create_endpoint(ws_relay_url, local_addr, None, Some(client_config)).await?;
    info!("ws connected!");
    let connecting = ep.connect(remote_addr, "localhost")?;
    info!("quinn connecting!");
    let conn = connecting.await?;
    info!("quinn connected!");
    let (mut send, mut recv) = conn.open_bi().await?;
    info!("bistream open!");
    send.write_all(message.as_bytes()).await?;
    send.finish().await?;
    info!("sent hello!");
    let reply = recv.read_to_end(1024).await?;
    let reply = String::from_utf8(reply)?;
    info!("received: {reply}");
    Ok(reply)
}

fn configure_client() -> ClientConfig {
    let crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(SkipServerVerification::new())
        .with_no_client_auth();

    ClientConfig::new(Arc::new(crypto))
}

// Dummy certificate verifier that treats any certificate as valid.
/// NOTE, such verification is vulnerable to MITM attacks, but convenient for testing.
struct SkipServerVerification;

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: web_time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}

pub struct AppError(anyhow::Error);
impl<T: Into<anyhow::Error>> From<T> for AppError {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

impl From<AppError> for JsError {
    fn from(value: AppError) -> Self {
        let err = format!("{:?}", value.0);
        JsError::new(&err)
    }
}
impl From<AppError> for JsValue {
    fn from(value: AppError) -> Self {
        let err = format!("{:?}", value.0);
        JsValue::from(err)
    }
}
