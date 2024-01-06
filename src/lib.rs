use std::io::ErrorKind;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::ready;
use std::{
    fmt,
    io::{self, IoSliceMut},
    net::SocketAddr,
    sync::Arc,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures_util::{Sink, Stream};
use quinn::{
    udp::{RecvMeta, Transmit},
    AsyncUdpSocket, ClientConfig, Endpoint, ServerConfig,
};
use serde::{Deserialize, Serialize};
use tokio_tungstenite_wasm::{Message, WebSocketStream};
use tracing::debug;

pub use quinn;

mod runtime;

pub async fn create_endpoint<S: AsRef<str>>(
    url: S,
    local_addr: SocketAddr,
    server_config: Option<ServerConfig>,
    client_config: Option<ClientConfig>,
) -> anyhow::Result<Endpoint> {
    let ws = tokio_tungstenite_wasm::connect(url).await?;
    let ep = create_endpoint_for_websocket(ws, local_addr, server_config, client_config)?;
    Ok(ep)
}

pub fn create_endpoint_for_websocket(
    ws: WebSocketStream,
    local_addr: SocketAddr,
    server_config: Option<ServerConfig>,
    client_config: Option<ClientConfig>,
) -> anyhow::Result<Endpoint> {
    let socket = Arc::new(WsUdpSocket::new(ws, local_addr));
    let runtime = Arc::new(runtime::JsRuntime);
    let mut endpoint =
        Endpoint::new_with_abstract_socket(Default::default(), server_config, socket, runtime)?;
    if let Some(client_config) = client_config {
        endpoint.set_default_client_config(client_config);
    }
    Ok(endpoint)
}

pub struct WsUdpSocket {
    ws: Mutex<Pin<Box<WebSocketStream>>>,
    local_addr: SocketAddr,
    send_pos: Mutex<usize>,
}

impl fmt::Debug for WsUdpSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsUdpSocket")
            .field("local_addr", &self.local_addr)
            .finish()
    }
}

// this is only safe in js land..
unsafe impl Sync for WsUdpSocket {}
unsafe impl Send for WsUdpSocket {}

impl WsUdpSocket {
    pub fn new(ws: WebSocketStream, local_addr: SocketAddr) -> Self {
        Self {
            ws: Mutex::new(Box::pin(ws)),
            local_addr,
            send_pos: Mutex::new(0),
        }
    }
}

impl AsyncUdpSocket for WsUdpSocket {
    fn max_transmit_segments(&self) -> usize {
        1
    }

    /// Maximum number of datagrams that might be described by a single [`RecvMeta`]
    fn max_receive_segments(&self) -> usize {
        1
    }
    fn poll_send(
        &self,
        cx: &mut Context,
        transmits: &[Transmit],
    ) -> Poll<Result<usize, io::Error>> {
        debug!("start to SEND {} transmits", transmits.len());
        let mut ws = self.ws.lock().unwrap();
        let mut ws = ws.as_mut();
        ready!(Pin::new(&mut ws).poll_ready(cx))
            .map_err(|err| io::Error::new(ErrorKind::Other, err))?;
        let mut send_pos = self.send_pos.lock().unwrap();
        while *send_pos < transmits.len() {
            let transmit = &transmits[*send_pos];
            let message = transmit_to_message(&transmit).unwrap();
            Pin::new(&mut ws)
                .start_send(message)
                .map_err(|err| io::Error::new(ErrorKind::Other, err))?;
            ready!(Pin::new(&mut ws).poll_ready(cx))
                .map_err(|err| io::Error::new(ErrorKind::Other, err))?;
            debug!(
                "SENT to relay to {} len {}",
                transmit.destination,
                transmit.contents.len()
            );
            *send_pos += 1;
        }
        ready!(Pin::new(&mut ws).poll_flush(cx))
            .map_err(|err| io::Error::new(ErrorKind::Other, err))?;
        *send_pos = 0;
        Poll::Ready(Ok(transmits.len()))
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let mut ws = self.ws.lock().unwrap();
        let ws = ws.as_mut();
        let message = ready!(ws.poll_next(cx));
        let message =
            message.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "stream closed"))?;
        let message = message.map_err(|err| io::Error::new(ErrorKind::Other, err))?;
        let message = match message {
            Message::Binary(data) => Ok(data),
            _ => Err(io::Error::new(
                ErrorKind::InvalidData,
                "expected binary message",
            )),
        }?;
        let message: InboundMessage = postcard::from_bytes(&message)
            .map_err(|err| io::Error::new(ErrorKind::InvalidData, err))?;
        debug!(
            "RECV from relay from {} len {}",
            message.src,
            message.content.len()
        );
        bufs[0][..message.content.len()].copy_from_slice(&message.content);
        meta[0].addr = message.src;
        meta[0].dst_ip = None;
        meta[0].stride = message.content.len();
        meta[0].len = message.content.len();
        Poll::Ready(Ok(1))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }
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

impl InboundMessage {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, anyhow::Error> {
        let msg = postcard::from_bytes(bytes)?;
        Ok(msg)
    }
    pub fn to_vec(&self) -> Result<Vec<u8>, anyhow::Error> {
        let msg = postcard::to_stdvec(self)?;
        Ok(msg)
    }
}

impl OutboundMessage {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, anyhow::Error> {
        let msg = postcard::from_bytes(bytes)?;
        Ok(msg)
    }
    pub fn to_vec(&self) -> Result<Vec<u8>, anyhow::Error> {
        let msg = postcard::to_stdvec(self)?;
        Ok(msg)
    }
}

fn transmit_to_message(transmit: &Transmit) -> anyhow::Result<Message> {
    let message = OutboundMessage {
        dst: transmit.destination,
        content: transmit.contents.clone(),
    };
    let data = postcard::to_stdvec(&message)?;
    let message = Message::Binary(data);
    Ok(message)
}
