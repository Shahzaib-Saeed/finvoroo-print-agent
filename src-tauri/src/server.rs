use std::fs::OpenOptions;
use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;

use crate::auth;
use crate::print::{self, PrintRequest};
use crate::{AppState, VERSION};

const MAX_BODY: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub struct HttpState {
    pub app: AppState,
    last_print: Arc<Mutex<Option<Instant>>>,
}

#[derive(Serialize)]
struct ErrorBody {
    ok: bool,
    error: String,
}

#[derive(Deserialize)]
struct PairRequest {
    code: String,
    #[serde(default)]
    origin: Option<String>,
    #[serde(default)]
    workstation_id: Option<String>,
}

pub async fn serve(app: AppState, port: u16) -> anyhow::Result<()> {
    let state = HttpState {
        app,
        last_print: Arc::new(Mutex::new(None)),
    };

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Finvoroo Print Agent listening on http://{addr}");
    axum::serve(listener, router(state)).await?;
    Ok(())
}

pub fn router(state: HttpState) -> Router {
    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            axum::http::header::HeaderName::from_static("x-finvoroo-print-token"),
        ])
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            origin
                .to_str()
                .ok()
                .map(auth::origin_is_allowed)
                .unwrap_or(false)
        }));

    Router::new()
        .route("/status", get(status))
        .route("/health", get(status))
        .route("/printers", get(printers))
        .route("/print", post(print_handler))
        .route("/pair", post(pair_handler))
        .route("/settings", get(settings))
        .layer(RequestBodyLimitLayer::new(MAX_BODY))
        .layer(cors)
        .with_state(state)
}

pub fn http_state(app: AppState) -> HttpState {
    HttpState {
        app,
        last_print: Arc::new(Mutex::new(None)),
    }
}

async fn status() -> impl IntoResponse {
    Json(serde_json::json!({
        "running": true,
        "version": VERSION,
        "bind": "127.0.0.1",
        "platform": std::env::consts::OS,
        "auth_required": true,
        "pairing_required": true,
        "pairing_available": true,
    }))
}

async fn printers(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    match tokio::task::spawn_blocking(print::list_printers).await {
        Ok(Ok(printers)) => Json(serde_json::json!({
            "ok": true,
            "printers": printers,
        }))
        .into_response(),
        Ok(Err(err)) => error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
        Err(err) => error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

async fn settings(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let cfg = state.app.config.read().await;
    Json(serde_json::json!({
        "ok": true,
        "version": VERSION,
        "port": cfg.port,
        "bind": "127.0.0.1",
        "default_printer_id": cfg.default_printer_id,
        "platform": std::env::consts::OS,
        "token_set": !cfg.token.is_empty(),
        "paired_origin": cfg.paired_origin,
    }))
    .into_response()
}

async fn pair_handler(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(req): Json<PairRequest>,
) -> Response {
    let origin = auth::origin_from_headers(&headers)
        .or(req.origin.clone())
        .unwrap_or_default();
    if origin.is_empty() || !auth::origin_is_allowed(&origin) {
        return error_response(
            StatusCode::FORBIDDEN,
            "Origin is not allowed to pair with Finvoroo Print Agent",
        );
    }

    if !state.app.pairing.verify_and_consume(&req.code) {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "Invalid or expired pairing code",
        );
    }

    let mut cfg = state.app.config.write().await;
    if cfg.token.trim().is_empty() {
        cfg.token = auth::generate_token();
    }
    cfg.paired_origin = Some(origin.clone());
    cfg.paired_at = Some(chrono_now());
    if let Err(err) = cfg.save(&state.app.config_path) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string());
    }

    tracing::info!(
        origin = %origin,
        workstation = req.workstation_id.as_deref().unwrap_or(""),
        "Finvoroo paired with print agent"
    );

    Json(serde_json::json!({
        "ok": true,
        "token": cfg.token,
        "origin": origin,
        "version": VERSION,
    }))
    .into_response()
}

async fn print_handler(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(req): Json<PrintRequest>,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    {
        let mut last = state.last_print.lock().await;
        if let Some(prev) = *last {
            if prev.elapsed() < std::time::Duration::from_millis(80) {
                return error_response(
                    StatusCode::TOO_MANY_REQUESTS,
                    "Print job already in progress",
                );
            }
        }
        *last = Some(Instant::now());
    }

    if req.printer_id.trim().is_empty() {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, "printer_id is required");
    }
    if req.data.trim().is_empty() {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, "print data is empty");
    }

    let job = req.clone();
    match tokio::task::spawn_blocking(move || print::print_job(&job)).await {
        Ok(Ok(())) => Json(serde_json::json!({
            "ok": true,
            "printer_id": req.printer_id,
            "type": req.job_type,
        }))
        .into_response(),
        Ok(Err(err)) => {
            log_print_error(&state, &err.to_string());
            error_response(StatusCode::UNPROCESSABLE_ENTITY, err.to_string())
        }
        Err(err) => {
            log_print_error(&state, &err.to_string());
            error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
        }
    }
}

async fn require_auth(state: &HttpState, headers: &HeaderMap) -> Result<(), Response> {
    if let Some(origin) = auth::origin_from_headers(headers) {
        if !auth::origin_is_allowed(&origin) {
            return Err(error_response(
                StatusCode::FORBIDDEN,
                "Origin is not allowed to print",
            ));
        }
    }

    let expected = state.app.config.read().await.token.clone();
    let provided = auth::token_from_headers(headers).unwrap_or_default();
    if auth::tokens_match(&expected, &provided) {
        Ok(())
    } else {
        Err(error_response(
            StatusCode::UNAUTHORIZED,
            "Invalid or missing Finvoroo Print Agent token",
        ))
    }
}

fn log_print_error(state: &HttpState, message: &str) {
    tracing::error!("print failed: {message}");
    let path = &state.app.log_path;
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{} {message}", chrono_now());
    }
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{now}")
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let body = Json(ErrorBody {
        ok: false,
        error: message.into(),
    });
    (status, body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConfig;
    use crate::pairing::PairingStore;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "fpa-http-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.json");
        let cfg = AgentConfig {
            token: "test-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            port: 17392,
            default_printer_id: None,
            paired_origin: None,
            paired_at: None,
            first_run: false,
        };
        cfg.save(&config_path).unwrap();
        AppState {
            config: Arc::new(tokio::sync::RwLock::new(cfg)),
            config_path,
            pairing: Arc::new(PairingStore::default()),
            log_path: dir.join("print-agent.log"),
        }
    }

    async fn json(res: Response) -> serde_json::Value {
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn status_is_public() {
        let app = router(http_state(test_state()));
        let res = app
            .oneshot(Request::builder().uri("/status").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = json(res).await;
        assert_eq!(body["running"], true);
        assert!(body.get("token").is_none());
    }

    #[tokio::test]
    async fn printers_require_token() {
        let app = router(http_state(test_state()));
        let res = app
            .oneshot(Request::builder().uri("/printers").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn print_requires_token() {
        let app = router(http_state(test_state()));
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/print")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"printer_id":"HP","type":"pdf","data":"x"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn pair_rejects_wrong_pin_and_origin() {
        let state = test_state();
        let app = router(http_state(state.clone()));
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/pair")
                    .header("content-type", "application/json")
                    .header("origin", "https://evil.example")
                    .body(Body::from(r#"{"code":"123456"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let app = router(http_state(state.clone()));
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/pair")
                    .header("content-type", "application/json")
                    .header("origin", "http://127.0.0.1:5173")
                    .body(Body::from(r#"{"code":"000000"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn pair_returns_token_once_for_valid_pin() {
        let state = test_state();
        let code = state.pairing.issue();
        let app = router(http_state(state.clone()));
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/pair")
                    .header("content-type", "application/json")
                    .header("origin", "http://localhost:5173")
                    .body(Body::from(format!(r#"{{"code":"{code}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = json(res).await;
        assert_eq!(body["ok"], true);
        assert_eq!(
            body["token"],
            "test-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );

        let app = router(http_state(state));
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/pair")
                    .header("content-type", "application/json")
                    .header("origin", "http://localhost:5173")
                    .body(Body::from(format!(r#"{{"code":"{code}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}
