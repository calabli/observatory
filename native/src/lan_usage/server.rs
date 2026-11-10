//! Peer service: shares the usage that was parsed out of the local files on this
//! machine through a read-only HTTP interface, and answers the discovery requests
//! on the LAN with UDP broadcasts.
//!
//! It only starts once the user enables LAN sharing in the settings. The service
//! exposes nothing but usage statistics and never touches any configuration,
//! credential or session content.

use super::{identity, pairing, storage, DeviceInfo, UsageExport};
use crate::database::Database;
use crate::error::AppError;
use axum::http::{HeaderMap, StatusCode};
use axum::{
    extract::{rejection::QueryRejection, ConnectInfo, Query, RawQuery, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::watch;

/// The handle of the running LAN service.
pub struct LanServerHandle {
    pub port: u16,
    stop_tx: watch::Sender<bool>,
}

impl LanServerHandle {
    /// Stop the HTTP service and the UDP replies.
    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }
}

/// The state the service shares internally.
struct ServerState {
    db: Arc<Database>,
    scan_lock: Arc<tokio::sync::Mutex<()>>,
    jobs: crate::scan_jobs::ScanJobs,
}

/// Start the HTTP service and the UDP discovery replies.
///
/// Returns an error when the port is taken (the caller logs it and asks the user to pick
/// another port).
pub async fn start(db: Arc<Database>, port: u16) -> Result<LanServerHandle, AppError> {
    start_with_lock(db, port, Arc::new(tokio::sync::Mutex::new(()))).await
}

pub async fn start_with_lock(
    db: Arc<Database>,
    port: u16,
    scan_lock: Arc<tokio::sync::Mutex<()>>,
) -> Result<LanServerHandle, AppError> {
    let state = Arc::new(ServerState {
        db: db.clone(),
        scan_lock,
        jobs: Default::default(),
    });
    let app = Router::new()
        .route("/api/lan/info", get(info_handler))
        .route("/api/lan/usage", get(usage_handler))
        .route("/api/lan/usage/fingerprint", get(usage_fingerprint_handler))
        .route("/api/v1/usage/summary", get(summary_handler))
        .route("/api/v1/usage/cycle", get(cycle_handler))
        .route("/api/v1/usage/cycles", get(cycles_handler))
        .route("/api/v1/dashboard/usage", post(dashboard_handler))
        .route("/api/v1/dashboard/models", post(dashboard_models_handler))
        .route("/api/lan/pair", post(pair_handler))
        .route("/api/lan/revoke", post(revoke_handler))
        .route("/api/lan/scan", post(scan_handler))
        .route("/api/lan/scan-status", post(scan_status_handler))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            log_request,
        ))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| AppError::Message(format!("局域网用量服务端口 {port} 绑定失败: {e}")))?;
    let bound_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);

    let (stop_tx, _initial_rx) = watch::channel(false);

    let http_stop = stop_tx.subscribe();
    let http_task = tokio::spawn(async move {
        let shutdown = wait_for_stop(http_stop);
        if let Err(e) = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await
        {
            log::error!("局域网用量 HTTP 服务退出: {e}");
        }
    });

    let udp_stop = stop_tx.subscribe();
    let udp_db = db;
    let udp_task = tokio::spawn(async move {
        if let Err(e) = run_discovery_responder(udp_db, bound_port, udp_stop).await {
            log::warn!("局域网发现应答服务退出: {e}");
        }
    });

    tokio::spawn(async move {
        let _ = http_task.await;
        let _ = udp_task.await;
    });

    Ok(LanServerHandle {
        port: bound_port,
        stop_tx,
    })
}

/// The Chinese description of an interface path (shown in the activity log).
fn route_label(path: &str) -> &'static str {
    match path {
        "/api/lan/info" => "查询本机身份",
        "/api/lan/usage" => "对端拉取用量导出",
        "/api/lan/usage/fingerprint" => "对端查询内容指纹",
        "/api/lan/pair" => "对端发起配对",
        "/api/lan/revoke" => "对端撤销授权",
        "/api/lan/scan" => "对端触发本机扫描",
        "/api/lan/scan-status" => "对端查询扫描状态",
        "/api/v1/usage/summary" => "只读接口：用量汇总",
        "/api/v1/usage/cycle" => "只读接口：当前额度周期",
        "/api/v1/usage/cycles" => "只读接口：额度周期序列",
        "/api/v1/dashboard/usage" => "只读接口：看板用量",
        "/api/v1/dashboard/models" => "只读接口：看板模型",
        _ => "未知接口",
    }
}

/// External call log: every HTTP request that hits the local LAN service records one
/// activity with the caller, the status code, the duration and the response size. The
/// record is written on the blocking thread pool so it does not slow the response down; the
/// token and the API key in the request headers are only used to determine the caller type
/// and never end up in the log.
async fn log_request(
    State(state): State<Arc<ServerState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::body::HttpBody;
    let started = std::time::Instant::now();
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(str::to_string);
    let remote = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0.ip().to_string());
    let device = header(request.headers(), "x-observatory-device");
    let with_api_key = !header(request.headers(), "x-api-key").is_empty();
    let response = next.run(request).await;
    let status = response.status().as_u16();
    let bytes = response.body().size_hint().exact();
    let duration_ms = started.elapsed().as_millis() as i64;
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        // Only states which credential the request carried; whether the credential is valid shows in the status code (401/403 means invalid).
        let caller = if with_api_key {
            "携带 API 密钥".to_string()
        } else if !device.is_empty() {
            format!("设备 {}", storage::device_label(&db, &device))
        } else {
            "未认证".to_string()
        };
        crate::activity::record(
            &db,
            crate::activity::Event {
                kind: "api_in",
                title: route_label(&path).to_string(),
                status: if (200..300).contains(&status) {
                    "ok"
                } else {
                    "error"
                },
                duration_ms: Some(duration_ms),
                summary: serde_json::json!({
                    "method": method,
                    "path": path,
                    "query": query,
                    "remote": remote,
                    "caller": caller,
                    "deviceId": (!device.is_empty()).then_some(device),
                    "httpStatus": status,
                    "responseBytes": bytes,
                }),
                detail: None,
            },
        );
    });
    response
}

/// The error type of the handlers. `AppError` does not implement axum's `IntoResponse`,
/// so the directly respondable (status code, description) form is used here.
type HandlerError = (StatusCode, String);
type ApiHandlerError = (StatusCode, Json<serde_json::Value>);

/// The caller identity of an `/api/v1/*` request.
enum Caller {
    /// A read-only API key generated by the local user: it may query this machine and the other synced devices.
    ApiKey,
    /// The token of a paired device: it may only query the usage of this machine itself.
    Peer,
}

impl Caller {
    fn check_scope(&self, devices: Option<&str>) -> Result<(), ApiHandlerError> {
        match self {
            Caller::ApiKey => Ok(()),
            Caller::Peer => super::public_api::ensure_local_scope(devices).map_err(|error| {
                (
                    StatusCode::from_u16(error.status).unwrap_or(StatusCode::FORBIDDEN),
                    Json(error.to_json()),
                )
            }),
        }
    }
}

fn header(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

/// Validate an API key or a pairing token. Reading the settings needs the connection lock,
/// so it goes to the blocking thread pool instead of stalling a tokio worker while a scan
/// holds the lock.
async fn api_caller(state: &ServerState, headers: &HeaderMap) -> Result<Caller, ApiHandlerError> {
    let (key, peer, token) = (
        header(headers, "x-api-key"),
        header(headers, "x-observatory-device"),
        header(headers, "x-observatory-token"),
    );
    let db = state.db.clone();
    let caller = tokio::task::spawn_blocking(move || {
        if super::public_api::key_matches(&db, &key) {
            Some(Caller::ApiKey)
        } else if pairing::authorized(&db, &peer, &token).unwrap_or(false) {
            Some(Caller::Peer)
        } else {
            None
        }
    })
    .await
    .ok()
    .flatten();
    caller.ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(
                serde_json::json!({"error":"缺少有效的只读 API 密钥或配对凭证","code":"INVALID_API_KEY"}),
            ),
        )
    })
}

async fn dashboard_models_handler(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    payload: Result<
        Json<super::public_api::DashboardModelsQuery>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Result<impl axum::response::IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let caller = api_caller(&state, &headers).await?;
    let Json(query) = payload.map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"请求内容无效","code":"INVALID_PARAMETERS"})),
        )
    })?;
    caller.check_scope(query.devices.as_deref())?;
    let db = state.db.clone();
    let result =
        tokio::task::spawn_blocking(move || super::public_api::dashboard_models(&db, query))
            .await
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error":"用量查询失败","code":"USAGE_QUERY_FAILED"})),
                )
            })?;
    let result = result.map_err(|error| {
        (
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(error.to_json()),
        )
    })?;
    Ok(([("Cache-Control", "no-store")], Json(result)))
}

async fn dashboard_handler(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    payload: Result<
        Json<super::public_api::DashboardQuery>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Result<impl axum::response::IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let caller = api_caller(&state, &headers).await?;
    let Json(query) = payload.map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"请求内容无效","code":"INVALID_PARAMETERS"})),
        )
    })?;
    caller.check_scope(query.devices.as_deref())?;
    let db = state.db.clone();
    let result =
        tokio::task::spawn_blocking(move || super::public_api::dashboard_usage(&db, query))
            .await
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error":"用量查询失败","code":"USAGE_QUERY_FAILED"})),
                )
            })?;
    let result = result.map_err(|error| {
        (
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(error.to_json()),
        )
    })?;
    Ok(([("Cache-Control", "no-store")], Json(result)))
}

async fn summary_handler(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    raw: Option<RawQuery>,
    query: Result<Query<super::public_api::SummaryQuery>, QueryRejection>,
) -> Result<impl axum::response::IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let caller = api_caller(&state, &headers).await?;
    // serde only keeps the last value of a repeated parameter, so the contract rejects them right away.
    if let Some(RawQuery(Some(raw))) = &raw {
        if let Some(duplicate) = duplicate_query_keys(raw) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::json!({"error":format!("参数重复: {duplicate}"),"code":"INVALID_PARAMETERS"}),
                ),
            ));
        }
    }
    let Query(query) = query.map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"查询参数无效","code":"INVALID_PARAMETERS"})),
        )
    })?;
    caller.check_scope(query.devices.as_deref())?;
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || super::public_api::summary(&db, query))
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error":"用量查询失败","code":"USAGE_QUERY_FAILED"})),
            )
        })?;
    let result = result.map_err(|error| {
        let status =
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(error.to_json()))
    })?;
    Ok(([("Cache-Control", "no-store")], Json(result)))
}

/// `GET /api/v1/usage/cycle`: query the usage inside the current quota cycle of an Agent
/// platform.
///
/// The response body is exactly the same as `/api/v1/usage/summary`, only the interval is
/// computed by the server from the cycle rules of the subscription.
async fn cycle_handler(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    raw: Option<RawQuery>,
    query: Result<Query<super::public_api::CycleQuery>, QueryRejection>,
) -> Result<impl axum::response::IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let caller = api_caller(&state, &headers).await?;
    if let Some(RawQuery(Some(raw))) = &raw {
        if let Some(duplicate) = duplicate_query_keys(raw) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::json!({"error":format!("参数重复: {duplicate}"),"code":"INVALID_PARAMETERS"}),
                ),
            ));
        }
    }
    let Query(query) = query.map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"查询参数无效","code":"INVALID_PARAMETERS"})),
        )
    })?;
    caller.check_scope(query.devices.as_deref())?;
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || super::public_api::cycle_summary(&db, query))
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error":"用量查询失败","code":"USAGE_QUERY_FAILED"})),
            )
        })?;
    let result = result.map_err(|error| {
        let status =
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(error.to_json()))
    })?;
    Ok(([("Cache-Control", "no-store")], Json(result)))
}

/// `GET /api/v1/usage/cycles`: query the usage series of the last few quota cycles of an
/// Agent platform.
///
/// Meant for the dashboard: it returns "the current cycle plus the previous N cycles" in
/// one go, grouped by calendar day, and never answers with a 422.
async fn cycles_handler(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    raw: Option<RawQuery>,
    query: Result<Query<super::public_api::CycleSeriesQuery>, QueryRejection>,
) -> Result<impl axum::response::IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let caller = api_caller(&state, &headers).await?;
    if let Some(RawQuery(Some(raw))) = &raw {
        if let Some(duplicate) = duplicate_query_keys(raw) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::json!({"error":format!("参数重复: {duplicate}"),"code":"INVALID_PARAMETERS"}),
                ),
            ));
        }
    }
    let Query(query) = query.map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"查询参数无效","code":"INVALID_PARAMETERS"})),
        )
    })?;
    caller.check_scope(query.devices.as_deref())?;
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || super::public_api::cycle_series(&db, query))
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error":"用量查询失败","code":"USAGE_QUERY_FAILED"})),
            )
        })?;
    let result = result.map_err(|error| {
        let status =
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(error.to_json()))
    })?;
    Ok(([("Cache-Control", "no-store")], Json(result)))
}

/// Find the query parameter names that occur more than once.
fn duplicate_query_keys(raw: &str) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    for pair in raw.split('&').filter(|part| !part.is_empty()) {
        let key = percent_decode(pair.split('=').next().unwrap_or(""));
        if !seen.insert(key.clone()) {
            return Some(key);
        }
    }
    None
}

/// Percent-decode the query string. The key names are only used for comparison, so a failed decode is compared as is.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// `GET /api/lan/info`: the introduction of this machine.
async fn info_handler(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<DeviceInfo>, HandlerError> {
    // Reading the device information needs the connection lock, so it is moved off the runtime thread like the other handlers of this file.
    let db = state.db.clone();
    let info = tokio::task::spawn_blocking(move || identity::device_info(&db))
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "任务失败".into()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(info))
}

/// `GET /api/lan/usage`: export the usage inside the local sharing scope.
async fn usage_handler(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Result<Json<UsageExport>, HandlerError> {
    authorized(&state, &headers).await?;
    // The full export is the heaviest read-only path: it reads the detail records of all
    // selected apps and serializes them while holding the connection lock, so it has to run in
    // spawn_blocking instead of occupying a tokio worker.
    let db = state.db.clone();
    let export = tokio::task::spawn_blocking(move || storage::export_local_usage(&db))
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "任务失败".into()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(export))
}

/// `GET /api/lan/usage/fingerprint`: the write epoch fingerprint of the local usage content.
///
/// It lets the client compare before it pulls in every round: it uses the export epoch
/// (`Database::export_epoch`), which only changes when data that ends up in the export is
/// written (the local detail records, the daily rollups, the providers, the sharing
/// settings). The copies synced in from other devices do not count -- otherwise two mutually
/// paired devices would change their own fingerprint because they stored each other's data
/// and would download everything from each other forever. It is a pure atomic read and does
/// not touch the database; older clients do not request it, so there is no compatibility
/// impact.
async fn usage_fingerprint_handler(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, HandlerError> {
    authorized(&state, &headers).await?;
    Ok(Json(serde_json::json!({"epoch": state.db.export_epoch()})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairRequest {
    code: String,
    device_id: String,
    port: u16,
    reverse_token: String,
}
async fn pair_handler(
    State(state): State<Arc<ServerState>>,
    ConnectInfo(address): ConnectInfo<std::net::SocketAddr>,
    Json(req): Json<PairRequest>,
) -> Result<Json<serde_json::Value>, HandlerError> {
    let host = address.ip().to_string();
    let info = tokio::time::timeout(
        std::time::Duration::from_secs(4),
        super::client::fetch_info(&host, req.port),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .ok_or_else(|| {
        (
            StatusCode::BAD_GATEWAY,
            "无法连接发起设备，请检查局域网共享和防火墙".into(),
        )
    })?;
    if info.device_id != req.device_id {
        return Err((StatusCode::BAD_REQUEST, "发起设备身份不匹配".into()));
    }
    let token = {
        let db = state.db.clone();
        let device_name = info.device_name.clone();
        let host = host.clone();
        let req = PairRequest {
            code: req.code.clone(),
            device_id: req.device_id.clone(),
            port: req.port,
            reverse_token: req.reverse_token.clone(),
        };
        tokio::task::spawn_blocking(move || {
            pairing::accept_bidirectional(
                &db,
                &req.code,
                &req.device_id,
                &device_name,
                &host,
                req.port,
                &req.reverse_token,
            )
        })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "任务失败".to_string()))?
        .map_err(|_| (StatusCode::FORBIDDEN, "配对码无效或已过期".to_string()))?
    };
    let db = state.db.clone();
    let peer = req.device_id.clone();
    tokio::spawn(async move {
        let _ = super::client::sync_device(&db, &peer).await;
    });
    Ok(Json(serde_json::json!({"token": token})))
}
/// Validate a pairing token and return the peer device_id. Reading the settings needs the connection lock, so it goes to the blocking thread pool.
async fn authorized(state: &ServerState, headers: &HeaderMap) -> Result<String, HandlerError> {
    let peer = header(headers, "x-observatory-device");
    let token = header(headers, "x-observatory-token");
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        pairing::authorized(&db, &peer, &token)
            .unwrap_or(false)
            .then_some(peer)
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(|| (StatusCode::FORBIDDEN, "设备未配对".into()))
}
async fn revoke_handler(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Result<Json<bool>, HandlerError> {
    let peer = authorized(&state, &headers).await?;
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        pairing::revoke(&db, &peer)?;
        db.delete_setting(&format!("lan_outgoing_token_v1:{peer}"))?;
        storage::remove_device(&db, &peer)
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "任务失败".to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(true))
}
#[derive(Deserialize)]
struct ScanRequest {
    key: String,
}
async fn scan_handler(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(req): Json<ScanRequest>,
) -> Result<Json<crate::scan_jobs::ScanJob>, HandlerError> {
    let peer = authorized(&state, &headers).await?;
    let db = state.db.clone();
    let lock = state.scan_lock.clone();
    let requester = peer.clone();
    let job = state
        .jobs
        .start(&peer, &req.key, move || async move {
            let _guard = lock.lock().await;
            let enabled = crate::api::read_enabled_sources(&db).map_err(|e| e.to_string())?;
            let result = tokio::task::spawn_blocking(move || {
                let trigger =
                    crate::activity::ScanTrigger::Remote(storage::device_label(&db, &requester));
                crate::api::scan_database(&db, &enabled, trigger)
            })
            .await
            .map_err(|e| e.to_string())?;
            Ok((result.imported, result.errors))
        })
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok(Json(job))
}
async fn scan_status_handler(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(req): Json<ScanRequest>,
) -> Result<Json<crate::scan_jobs::ScanJob>, HandlerError> {
    let peer = authorized(&state, &headers).await?;
    state
        .jobs
        .get(&peer, &req.key)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "扫描任务不存在或已重启".into()))
}

/// UDP discovery reply: when a probe packet arrives, send the local identity back to the sender.
async fn run_discovery_responder(
    db: Arc<Database>,
    port: u16,
    stop_rx: watch::Receiver<bool>,
) -> Result<(), AppError> {
    let socket = tokio::net::UdpSocket::bind(("0.0.0.0", port))
        .await
        .map_err(|e| AppError::Message(format!("局域网发现端口 {port} 绑定失败: {e}")))?;
    let mut buf = vec![0u8; 512];
    loop {
        // Check the flag before receiving: stop() may already have been called before the loop was entered.
        if *stop_rx.borrow() {
            return Ok(());
        }
        // Wrap it in a timeout instead of using select!: borrowing stop_rx again inside a select!
        // branch hits the borrow checker, and the changed() branch and the borrow() here cannot
        // hold this receiver at the same time.
        let received = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            socket.recv_from(&mut buf),
        )
        .await;
        let Ok(Ok((len, peer))) = received else {
            continue;
        };
        let probe = String::from_utf8_lossy(&buf[..len]);
        if !probe.trim().starts_with(super::DISCOVERY_MAGIC) {
            continue;
        }
        // Do not answer when the identity cannot be read: the other side will broadcast again.
        // Go through the short TTL cache so that the broadcast packets do not grab the connection
        // lock one by one.
        if let Ok(info) = identity::device_info_cached(&db) {
            if let Ok(payload) = serde_json::to_vec(&info) {
                let _ = socket.send_to(&payload, peer).await;
            }
        }
    }
}

/// Wait until the stop flag is set.
///
/// `Receiver::wait_for` is not used: the `watch::Ref` it returns is not Send and would not
/// compile inside the future of a `tokio::spawn`. The `borrow` plus `changed` combination is
/// used here, both of them are Send.
async fn wait_for_stop(mut rx: watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            // The sender is gone: no other path will set the flag any more, so just finish.
            return;
        }
    }
}
