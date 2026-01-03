# MoQ Relay Safari Bridge Architecture

This document describes the architecture and implementation of WebSocket-based WebTransport support for the MoQ (Media over QUIC) relay, enabling Safari browser compatibility alongside Chrome/Firefox QUIC support.

## Overview

The MoQ relay now supports dual-stack transport:
- **QUIC/WebTransport** (UDP port 443) - Chrome, Firefox, and other browsers with native WebTransport
- **WebSocket/WebTransport** (TCP port 443) - Safari and browsers without native WebTransport support

Both transports share the same TLS certificates and port number (443), with the OS routing UDP traffic to QUIC and TCP traffic to WebSocket.

## Architecture Diagram

```
                                    Port 443
                                       │
                    ┌──────────────────┴──────────────────┐
                    │                                     │
                UDP (QUIC)                           TCP (WebSocket)
                    │                                     │
                    ▼                                     ▼
            quinn::Endpoint                        TcpListener
                    │                                     │
                    ▼                                     ▼
         web_transport_quinn                       tokio_rustls
                    │                                     │
                    ▼                                     ▼
         web_transport::Session              web_transport_ws::Session
                    │                                     │
                    ▼                                     ▼
      moq_transport::session::Session              WsSession (adapter)
                    │                                     │
                    └──────────────┬──────────────────────┘
                                   │
                                   ▼
                        moq_transport::transport
                           (abstraction layer)
                                   │
                    ┌──────────────┴──────────────┐
                    │                             │
                    ▼                             ▼
               Publisher                      Subscriber
                    │                             │
                    └──────────────┬──────────────┘
                                   │
                                   ▼
                              Relay Logic
                         (Producer/Consumer)
```

## Repository Structure

```
linode-moq-07/
├── Cargo.toml                          # Workspace root (updated)
├── arch.md                             # This document
├── internal_crates/                    # Vendored dependencies
│   ├── web-transport-proto/            # Protocol encoding/decoding
│   ├── web-transport-trait/            # Generic transport traits
│   └── web-transport-ws/               # WebSocket WebTransport impl
├── moq-transport/
│   └── src/
│       ├── transport.rs                # NEW: Transport abstraction
│       └── ...
├── moq-relay-ietf/
│   └── src/
│       ├── main.rs                     # Updated: async Relay::new()
│       ├── relay.rs                    # Updated: dual-stack accept
│       ├── ws.rs                       # NEW: WebSocket server
│       ├── ws_adapter.rs               # NEW: Transport adapters
│       └── ...
└── ...
```

## Implementation Steps

### Step 1: Vendoring Luke's WebSocket Polyfill

The `web-transport-ws` crate from [moq-dev/web-transport](https://github.com/moq-dev/web-transport) provides WebSocket-based WebTransport emulation. We vendored this along with its dependencies:

```bash
# Clone the upstream repo
git clone --depth 1 https://github.com/moq-dev/web-transport.git /tmp/web-transport-clone

# Copy required crates
mkdir -p internal_crates
cp -r /tmp/web-transport-clone/web-transport-proto internal_crates/
cp -r /tmp/web-transport-clone/web-transport-trait internal_crates/
cp -r /tmp/web-transport-clone/web-transport-ws internal_crates/
```

**Key modifications to vendored crates:**

1. **`web-transport-ws/Cargo.toml`**: Changed workspace dependencies to local paths:
   ```toml
   web-transport-proto = { path = "../web-transport-proto" }
   web-transport-trait = { path = "../web-transport-trait" }
   ```

2. **`web-transport-ws/src/lib.rs`**: Made `Error` type public:
   ```rust
   pub use error::*;  // Changed from pub(crate)
   ```

3. **`web-transport-ws/src/session.rs`**: Fixed tokio compatibility issue:
   ```rust
   // Original (doesn't compile with tokio 1.37):
   outbound.send(e.into_inner()).await.ok();

   // Fixed:
   let frame = match e {
       tokio::sync::mpsc::error::TrySendError::Full(f) => f,
       tokio::sync::mpsc::error::TrySendError::Closed(f) => f,
   };
   outbound.send(frame).await.ok();
   ```

### Step 2: Workspace Configuration

Updated root `Cargo.toml` to include vendored crates:

```toml
[workspace]
members = [
    # ... existing members ...
    "internal_crates/web-transport-proto",
    "internal_crates/web-transport-trait",
    "internal_crates/web-transport-ws",
]
```

### Step 3: Transport Abstraction Layer

Created `moq-transport/src/transport.rs` to define generic transport traits:

```rust
pub trait Session: Clone + Send + Sync + 'static {
    type SendStream: SendStream;
    type RecvStream: RecvStream;
    type Error: std::error::Error + Send + Sync + 'static;

    fn accept_bi(&mut self) -> impl Future<Output = Result<...>>;
    fn accept_uni(&mut self) -> impl Future<Output = Result<...>>;
    fn open_bi(&mut self) -> impl Future<Output = Result<...>>;
    fn open_uni(&mut self) -> impl Future<Output = Result<...>>;
    fn recv_datagram(&mut self) -> impl Future<Output = Result<Bytes, ...>>;
    fn send_datagram(&mut self, payload: Bytes) -> Result<(), ...>;
    fn close(self, code: u32, reason: &str);
    fn closed(&self) -> impl Future<Output = Self::Error>;
}

pub trait SendStream: Send { ... }
pub trait RecvStream: Send { ... }
```

Implementations provided for:
- `web_transport::Session` (QUIC)
- `web_transport::SendStream` / `RecvStream`

### Step 4: WebSocket Adapter (Newtype Pattern)

Created wrapper types in `moq-relay-ietf/src/ws_adapter.rs` to satisfy Rust's orphan rules:

```rust
#[derive(Clone)]
pub struct WsSession(pub web_transport_ws::Session);

pub struct WsSendStream(pub web_transport_ws::SendStream);
pub struct WsRecvStream(pub web_transport_ws::RecvStream);

impl transport::Session for WsSession {
    type SendStream = WsSendStream;
    type RecvStream = WsRecvStream;
    type Error = WsError;

    // Datagram handling per Luke's pattern:
    async fn recv_datagram(&mut self) -> Result<Bytes, Self::Error> {
        // Block forever - datagrams not supported over WebSocket
        std::future::pending().await
    }

    fn send_datagram(&mut self, _payload: Bytes) -> Result<(), Self::Error> {
        Err(WsError::DatagramsNotSupported)
    }
    // ... other methods delegate to inner session
}
```

### Step 5: WebSocket Server

Created `moq-relay-ietf/src/ws.rs`:

```rust
pub struct WsServer {
    listener: TcpListener,
    tls_acceptor: TlsAcceptor,
}

impl WsServer {
    pub async fn new(config: WsServerConfig) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(config.bind).await?;
        let tls_acceptor = TlsAcceptor::from(Arc::new(config.tls));
        Ok(Self { listener, tls_acceptor })
    }

    pub async fn accept(&self) -> Option<web_transport_ws::Session> {
        // 1. Accept TCP connection
        // 2. TLS handshake (tokio-rustls)
        // 3. WebSocket upgrade with "webtransport" subprotocol
        // 4. Return web_transport_ws::Session
    }
}
```

### Step 6: Dual-Stack Relay

Modified `moq-relay-ietf/src/relay.rs`:

```rust
pub struct Relay {
    quic: quic::Endpoint,
    ws: Option<WsServer>,  // NEW
    // ...
}

impl Relay {
    pub async fn new(config: RelayConfig) -> anyhow::Result<Self> {
        // Clone TLS config before QUIC consumes it
        let ws_tls_config = config.tls.server.clone();

        let quic = quic::Endpoint::new(...)?;

        // Create WebSocket server on same port (TCP vs UDP)
        let ws = if let Some(tls) = ws_tls_config {
            Some(WsServer::new(WsServerConfig { bind: config.bind, tls }).await?)
        } else {
            None
        };

        Ok(Self { quic, ws, ... })
    }

    pub async fn run(self) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                // QUIC connections (Chrome, Firefox)
                res = server.accept() => {
                    let conn = res?;
                    let (session, publisher, subscriber) =
                        moq_transport::session::Session::accept(conn).await?;
                    // ... handle session
                },

                // WebSocket connections (Safari)
                Some(ws_session) = async { ws_server.accept().await } => {
                    let _ws_session = WsSession::new(ws_session);
                    // TODO: Full MoQ integration pending
                },

                res = tasks.next() => res?,
            }
        }
    }
}
```

## Protocol Compatibility

### MoQ Version
- **Draft-07** - The relay maintains strict Draft-07 compatibility
- Version negotiation happens during the MoQ SETUP handshake
- SubgroupHeader uses `subscribe_id` field as required by Cloudflare's network

### Datagram Handling
Per Luke's WebSocket polyfill pattern:
- `max_datagram_size()` returns `None` (or 0)
- `send_datagram()` returns `Err(NotSupported)`
- `recv_datagram()` blocks forever (pending)
- MoQ automatically falls back to Subgroups/Streams for media delivery

## TLS Configuration

Both QUIC and WebSocket share the same certificates:

```rust
// QUIC uses quinn with rustls
let quic_config = quinn::ServerConfig::with_crypto(Arc::new(tls_config.clone()));

// WebSocket uses tokio-rustls
let tls_acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
```

CLI flags (unchanged):
- `--tls-cert <PATH>` - PEM-encoded certificate chain
- `--tls-key <PATH>` - PEM-encoded private key

## Current Status

### Completed
- [x] Vendored web-transport-ws and dependencies
- [x] Transport abstraction layer in moq-transport
- [x] WebSocket server with TLS support
- [x] Newtype adapters implementing transport traits
- [x] Dual-stack accept loop in relay
- [x] Successful build and compilation

### Pending (Future Work)
- [ ] Make `moq_transport::session::Session` generic over `transport::Session`
- [ ] Update `Reader`/`Writer` to use generic stream types
- [ ] Full MoQ protocol handling for WebSocket sessions
- [ ] End-to-end testing with Safari

## Testing

### Manual Testing
```bash
# Start the relay
cargo run -p moq-relay-ietf -- \
    --tls-cert /path/to/cert.pem \
    --tls-key /path/to/key.pem \
    --bind [::]:443

# Expected log output:
# INFO  WebSocket server listening on [::]:443 (for Safari)
# INFO  QUIC server listening on [::]:443
```

### Verification Checklist
1. Chrome connects via QUIC/WebTransport ✓ (existing functionality)
2. Safari connects via WebSocket/WebTransport (connection accepted, MoQ pending)
3. Both protocols share TLS certificates ✓
4. Both protocols use port 443 ✓

## Dependencies Added

```toml
# moq-relay-ietf/Cargo.toml
[dependencies]
web-transport-ws = { path = "../internal_crates/web-transport-ws" }
web-transport-trait = { path = "../internal_crates/web-transport-trait" }
tokio-rustls = "0.26"
bytes = "1"
thiserror = "1"
rustls = { version = "0.23", default-features = false, features = ["std", "tls12"] }
```

## References

- [MoQ Transport Draft-07](https://datatracker.ietf.org/doc/draft-ietf-moq-transport/07/)
- [web-transport-ws](https://github.com/moq-dev/web-transport/tree/main/web-transport-ws)
- [WebTransport over HTTP/3](https://www.w3.org/TR/webtransport/)
- [Cloudflare MoQ Demo](https://moq.cloudflare.tv/)
