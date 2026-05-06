use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use snomed_finding_extractor::{ExtractRequest, ExtractResponse, Extractor, TerminologyArtefact};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[arg(long)]
    artefact: PathBuf,
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 8060)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    let artefact = TerminologyArtefact::from_path(&cli.artefact)
        .with_context(|| format!("failed to load artefact {}", cli.artefact.display()))?;
    let extractor = Arc::new(Extractor::new(artefact)?);
    let address: SocketAddr = format!("{}:{}", cli.host, cli.port).parse()?;

    let app = Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/styles.css", get(styles_css))
        .route("/healthz", get(healthz))
        .route("/v1/extract", post(extract))
        .layer(TraceLayer::new_for_http())
        .with_state(extractor);

    let listener = TcpListener::bind(address).await?;
    tracing::info!(%address, "SNOMED extractor listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn healthz() -> &'static str {
    "ok"
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../../web/index.html"))
}

async fn app_js() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_str!("../../web/app.js"),
    )
}

async fn styles_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../../web/styles.css"),
    )
}

async fn extract(
    State(extractor): State<Arc<Extractor>>,
    Json(request): Json<ExtractRequest>,
) -> std::result::Result<Json<ExtractResponse>, (StatusCode, String)> {
    extractor
        .extract(request)
        .map(Json)
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))
}
