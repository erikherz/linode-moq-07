use std::{net, sync::Arc};

use axum::{extract::State, http::Method, response::IntoResponse, routing::get, Router};
use tower_http::cors::{Any, CorsLayer};

pub struct WebConfig {
    pub bind: net::SocketAddr,
    pub fingerprints: Vec<String>,
}

// Run a plain HTTP server using Axum for development purposes.
// Serves the certificate fingerprint for WebTransport debugging.
pub struct Web {
    bind: net::SocketAddr,
    app: Router,
}

impl Web {
    pub fn new(config: WebConfig) -> Self {
        // Get the first certificate's fingerprint.
        // TODO serve all of them so we can support multiple signature algorithms.
        let fingerprint = config
            .fingerprints
            .first()
            .expect("missing certificate")
            .clone();

        let app = Router::new()
            .route("/fingerprint", get(serve_fingerprint))
            .layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods([Method::GET]),
            )
            .with_state(fingerprint);

        Self {
            bind: config.bind,
            app,
        }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        log::info!("dev HTTP server listening on {}", self.bind);
        let listener = tokio::net::TcpListener::bind(self.bind).await?;
        axum::serve(listener, self.app.into_make_service()).await?;
        Ok(())
    }
}

async fn serve_fingerprint(State(fingerprint): State<String>) -> impl IntoResponse {
    fingerprint
}

// ============================================================================
// HTTPS (Secure) Development Server
// ============================================================================

pub struct WebSecureConfig {
    pub bind: net::SocketAddr,
    pub fingerprints: Vec<String>,
    pub tls: rustls::ServerConfig,
}

/// HTTPS web server for development purposes.
/// Serves the certificate fingerprint over TLS.
pub struct WebSecure {
    bind: net::SocketAddr,
    app: Router,
    tls: rustls::ServerConfig,
}

impl WebSecure {
    pub fn new(config: WebSecureConfig) -> Self {
        let fingerprint = config
            .fingerprints
            .first()
            .expect("missing certificate")
            .clone();

        let app = Router::new()
            .route("/fingerprint", get(serve_fingerprint))
            .layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods([Method::GET]),
            )
            .with_state(fingerprint);

        let mut tls = config.tls;
        tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        Self {
            bind: config.bind,
            app,
            tls,
        }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        log::info!("dev HTTPS server listening on {}", self.bind);
        let tls_config = hyper_serve::tls_rustls::RustlsConfig::from_config(Arc::new(self.tls));
        let server = hyper_serve::bind_rustls(self.bind, tls_config);
        server.serve(self.app.into_make_service()).await?;
        Ok(())
    }
}
