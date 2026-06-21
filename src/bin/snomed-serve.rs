use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use snomed_finding_extractor::{
    extract_plan_entities, DiagnosisExtractRequest, ExaminationFindingsExtractRequest,
    ExtractRequest, ExtractResponse, Extractor, ObservableExtractRequest, PlanExtractRequest,
    PlanExtractResponse, TerminologyArtefact,
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
    body_site_artefact: Option<PathBuf>,
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
    if cli.artefact.is_none() && cli.body_site_artefact.is_some() {
        anyhow::bail!("--body-site-artefact requires --artefact because body sites enrich the finding endpoint");
    }
    let findings =
        load_optional_finding_extractor(cli.artefact.as_ref(), cli.body_site_artefact.as_ref())?;
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
        .route("/v1/extract-plan", post(extract_plan))
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

fn load_optional_finding_extractor(
    path: Option<&PathBuf>,
    body_site_path: Option<&PathBuf>,
) -> Result<Option<Arc<Extractor>>> {
    let Some(path) = path else {
        return Ok(None);
    };

    let artefact = TerminologyArtefact::from_path(path)
        .with_context(|| format!("failed to load finding artefact {}", path.display()))?;
    let extractor = if let Some(body_site_path) = body_site_path {
        let body_site_artefact =
            TerminologyArtefact::from_path(body_site_path).with_context(|| {
                format!(
                    "failed to load body site artefact {}",
                    body_site_path.display()
                )
            })?;
        Extractor::new_with_body_sites(artefact, body_site_artefact)?
    } else {
        Extractor::new(artefact)?
    };
    let dropped = extractor.dropped_ambiguous_terms().len();
    if dropped > 0 {
        tracing::warn!(
            label = "finding",
            dropped_ambiguous = dropped,
            "ambiguity guard removed terms from artefact; run `snomed-extract audit-terms` to review"
        );
    }
    Ok(Some(Arc::new(extractor)))
}

fn load_optional_extractor(path: Option<&PathBuf>, label: &str) -> Result<Option<Arc<Extractor>>> {
    let Some(path) = path else {
        return Ok(None);
    };

    let artefact = TerminologyArtefact::from_path(path)
        .with_context(|| format!("failed to load {label} artefact {}", path.display()))?;
    let extractor = Extractor::new(artefact)?;
    let dropped = extractor.dropped_ambiguous_terms().len();
    if dropped > 0 {
        tracing::warn!(
            label,
            dropped_ambiguous = dropped,
            "ambiguity guard removed terms from artefact; run `snomed-extract audit-terms` to review"
        );
    }
    Ok(Some(Arc::new(extractor)))
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

async fn extract_plan(Json(request): Json<PlanExtractRequest>) -> Json<PlanExtractResponse> {
    Json(extract_plan_entities(request))
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
