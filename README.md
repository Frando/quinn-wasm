# quinn-wasm-web

*Status: Experimental*

This demo runs the [quinn](https://github.com/quinn-rs/quinn) QUIC
implementation compiled to WASM in the browser. It forwards UDP packets
over a WebSocket connection to a relay server, which then sends them out
to the actual destination. All packets are end-to-end encrpyted between
browser and the destination QUIC server; the relay cannot read anything.

*Caveats:*

-   The demo uses self-signed certificates without any checks, so it
    could be MITM\'ed. This can be mitigated though by either shipping
    regular WebPKI certificates in the WASM bundle, or using P2P
    certificates, e.g. like [Iroh Net](https://iroh.computer)
-   This currently needs a few patches to `quinn` and `rustls`. See the
    [GitHub repo](https://github.com/Frando/quinn-wasm) for details.
-   You likely do not want to run this as-is in production, because it
    will allow anyone to use your relay server to send any kind of UDP
    packet to any destination.

## Demotime!

A public demo runs [here](https://quinn-wasm.dev.arso.xyz/) currently.

## Run the demo

Build the WASM library:
```
cd examples/web
wasm-pack build --target web --debug
```

Run the relay server:
```
cd examples/relay
cargo run
```

Run a QUIC server to connect to:
```
cd examples/quic-echo-server
cargo run
```

Then open http://localhost:3000 in a web browser.
Clicking the **START** button will open a websocket connection to the relay server, create an in-browser QUIC endpoint, and let the relay server forward the QUIC UDP packets to the destinaation server.

## Patches needed

* The `wasm32-unknown-unknown` target [does not implement `std::time::Instant` and `std::time::SystemTime`](https://github.com/rust-lang/rust/issues/48564). Therefore, any code that uses these will panic in the browser. There are implementations for these primitives that use the web platform, however there is no way currently to swap the panicking std impls for another impl. The only way is to use feature flags in all dependencies.
* The `quinn` crate always wants to compile `socket2`, even if the native UDP backend is not used.

These are the branches in use at the demo. If this endavour is deemed worthwhile, I will start to create PRs out of these branches.

* **quinn**: [Frando/quinn#feat-wasm-web](https://github.com/quinn-rs/quinn/compare/main...Frando:quinn:feat-wasm-web)
  * Put the native UDP implementation in `quinn-udp` behind a on-by-default feature flag
  * Swap `std::time` to [`web_time`](https://docs.rs/web-time/latest/web_time/) if the `wasm-web` feature is enabled
* **rustls-pki-types**: [Frando/rustls-pki-types#wasm-web](https://github.com/rustls/pki-types/compare/main...Frando:rustls-pki-types:wasm-web)
  * Use `web_time` in place of `std::time` when compiling to WASM for the browser (through an optional feature flag `wasm-web`).
* **rustls**: [Frando/rustls#0.21-wasm](https://github.com/rustls/rustls/compare/v/0.21.10...Frando:rustls:0.21-wasm)
  * swaps `std::time` to `web_time`. Will not be needed on `0.22` because there all usages are changed to `rustls_pki_types::UnixTime` (which will need a feature flag, see below).

