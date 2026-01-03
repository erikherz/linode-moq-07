use std::net;

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
