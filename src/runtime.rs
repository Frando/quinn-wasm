use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

#[cfg(all(not(feature = "wasm-web"), feature = "native"))]
use tokio::time::{sleep_until, Sleep};
#[cfg(feature = "wasm-web")]
use wasmtimer::tokio::{sleep_until, Sleep};

use quinn::{time::Instant, AsyncTimer, AsyncUdpSocket, Runtime};

#[cfg(feature = "wasm-web")]
fn instant_to_wasmtimer_instant(input: Instant) -> wasmtimer::std::Instant {
    let now = Instant::now();
    let remaining = input.checked_duration_since(now).unwrap_or_default();
    let target = wasmtimer::std::Instant::now() + remaining;
    target
}

#[cfg(not(feature = "wasm-web"))]
fn instant_to_wasmtimer_instant(input: std::time::Instant) -> std::time::Instant {
    input
}

/// A Quinn runtime for Tokio
#[derive(Debug)]
pub struct JsRuntime;

impl Runtime for JsRuntime {
    fn new_timer(&self, t: Instant) -> Pin<Box<dyn AsyncTimer>> {
        let t = instant_to_wasmtimer_instant(t);
        Box::pin(MySleep(Box::pin(sleep_until(t.into()))))
    }

    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>) {
        #[cfg(all(not(feature = "wasm-web"), feature = "native"))]
        tokio::spawn(future);
        #[cfg(feature = "wasm-web")]
        wasm_bindgen_futures::spawn_local(future);
    }

    fn wrap_udp_socket(&self, _sock: std::net::UdpSocket) -> io::Result<Arc<dyn AsyncUdpSocket>> {
        unimplemented!("this runtime does not support UDP sockets")
    }
}

#[derive(Debug)]
struct MySleep(Pin<Box<Sleep>>);

impl AsyncTimer for MySleep {
    fn reset(mut self: Pin<&mut Self>, t: Instant) {
        let t = instant_to_wasmtimer_instant(t);
        Sleep::reset(self.0.as_mut(), t.into())
    }
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<()> {
        Future::poll(self.0.as_mut(), cx)
    }
}
