use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use snomed_finding_extractor::{
    DiagnosisExtractRequest, ExaminationFindingsExtractRequest, ExtractRequest, ExtractResponse,
    Extractor, ObservableExtractRequest, TerminologyArtefact,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[arg(long)]
    artefact: Option<PathBuf>,
    #[arg(long)]
    observables_artefact: Option<PathBuf>,
    #[arg(long)]
    examination_findings_artefact: Option<PathBuf>,
    #[arg(long, alias = "disorders-artefact")]
    diagnoses_artefact: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 8060)]
    port: u16,
}

#[derive(Debug, Clone)]
struct AppState {
    findings: Option<Arc<Extractor>>,
    observables: Option<Arc<Extractor>>,
    examination_findings: Option<Arc<Extractor>>,
    diagnoses: Option<Arc<Extractor>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    if cli.artefact.is_none()
        && cli.observables_artefact.is_none()
        && cli.examination_findings_artefact.is_none()
        && cli.diagnoses_artefact.is_none()
    {
        anyhow::bail!(
            "configure at least one artefact with --artefact, --observables-artefact, --examination-findings-artefact, or --diagnoses-artefact"
        );
    }

    let findings = load_optional_extractor(cli.artefact.as_ref(), "finding")?;
    let observables = load_optional_extractor(cli.observables_artefact.as_ref(), "observable")?;
    let examination_findings = load_optional_extractor(
        cli.examination_findings_artefact.as_ref(),
        "examination findings",
    )?;
    let diagnoses = load_optional_extractor(cli.diagnoses_artefact.as_ref(), "diagnoses")?;
    let state = AppState {
        findings,
        observables,
        examination_findings,
        diagnoses,
    };
    let address: SocketAddr = format!("{}:{}", cli.host, cli.port).parse()?;

    let app = Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/styles.css", get(styles_css))
        .route("/healthz", get(healthz))
        .route("/v1/extract", post(extract))
        .route("/v1/extract-observables", post(extract_observables))
        .route(
            "/v1/extract-examination-findings",
            post(extract_examination_findings),
        )
        .route("/v1/extract-diagnoses", post(extract_diagnoses))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = TcpListener::bind(address).await?;
    tracing::info!(%address, "SNOMED extractor listening");
    axum::serve(listener, app).await?;
    Ok(())
}

fn load_optional_extractor(path: Option<&PathBuf>, label: &str) -> Result<Option<Arc<Extractor>>> {
    let Some(path) = path else {
        return Ok(None);
    };

    let artefact = TerminologyArtefact::from_path(path)
        .with_context(|| format!("failed to load {label} artefact {}", path.display()))?;
    Ok(Some(Arc::new(Extractor::new(artefact)?)))
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
    State(state): State<AppState>,
    Json(request): Json<ExtractRequest>,
) -> std::result::Result<Json<ExtractResponse>, (StatusCode, String)> {
    let Some(extractor) = state.findings.as_ref() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "finding extractor is not configured; start the server with --artefact".to_string(),
        ));
    };

    extractor
        .extract(request)
        .map(Json)
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))
}

async fn extract_observables(
    State(state): State<AppState>,
    Json(request): Json<ObservableExtractRequest>,
) -> std::result::Result<Json<ExtractResponse>, (StatusCode, String)> {
    let Some(extractor) = state.observables.as_ref() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "observable extractor is not configured; start the server with --observables-artefact"
                .to_string(),
        ));
    };

    extractor
        .extract_observables(request)
        .map(Json)
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))
}

async fn extract_examination_findings(
    State(state): State<AppState>,
    Json(request): Json<ExaminationFindingsExtractRequest>,
) -> std::result::Result<Json<ExtractResponse>, (StatusCode, String)> {
    let Some(extractor) = state.examination_findings.as_ref() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "examination findings extractor is not configured; start the server with --examination-findings-artefact"
                .to_string(),
        ));
    };

    extractor
        .extract_examination_findings(request)
        .map(Json)
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))
}

async fn extract_diagnoses(
    State(state): State<AppState>,
    Json(request): Json<DiagnosisExtractRequest>,
) -> std::result::Result<Json<ExtractResponse>, (StatusCode, String)> {
    let Some(extractor) = state.diagnoses.as_ref() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "diagnosis extractor is not configured; start the server with --diagnoses-artefact"
                .to_string(),
        ));
    };

    extractor
        .extract_diagnoses(request)
        .map(Json)
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))
}
