use rmcp::transport::sse_server::{SseServer, SseServerConfig};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod common;
use common::movie_service::Movie;

const BIND_ADDRESS: &str = "127.0.0.1:9000";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = SseServerConfig {
        bind: BIND_ADDRESS.parse()?,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: tokio_util::sync::CancellationToken::new(),
        sse_keep_alive: Some(std::time::Duration::from_secs(15)),
    };

    let (sse_server, router) = SseServer::new(config);

    let listener = tokio::net::TcpListener::bind(sse_server.config.bind).await?;
    let ct = sse_server.config.ct.child_token();

    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        ct.cancelled().await;
        tracing::info!("movie sse server cancelled");
    });

    tokio::spawn(async move {
        if let Err(e) = server.await {
            tracing::error!(error = %e, "movie sse server shutdown with error");
        }
    });

    let ct = sse_server.with_service(Movie::new);

    tracing::info!(
        "movie server ready over SSE; endpoints: http://{}/sse",
        BIND_ADDRESS
    );
    tracing::info!("press Ctrl+C to stop");

    tokio::signal::ctrl_c().await?;
    ct.cancel();
    Ok(())
}
