use db::FormaDBClient;
use forma::observability::{Telemeter, TelemetryMode};
use protocols::FormaDB;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    forma::install_crypto_provider();
    let tel = TelemetryMode::JustOtel;
    let telemeter = Telemeter::new("forma-manager", tel);

    // The manager deploys agents to multi-tenant servers, which read them
    // from MongoDB.
    let mut db = FormaDBClient::new_mongo()
        .await
        .expect("failed to connect to database");
    db.init().expect("failed to initialise database");

    let app = forma::manager::router(Arc::new(db));

    let port = std::env::var("PORT").unwrap_or_else(|_| "3001".to_string());
    let addr = format!("0.0.0.0:{port}");
    tracing::info!("forma-manager listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");
    axum::serve(listener, app)
        .with_graceful_shutdown(forma::observability::shutdown_signal())
        .await
        .expect("server error");

    // Ensure all spans are flushed before exit
    telemeter.shutdown();
}
