//! 对端服务：把本机「本地文件解析」出来的用量，通过只读 HTTP 接口共享出去，
//! 同时用 UDP 广播应答局域网里的发现请求。
//!
//! 只有用户在设置里打开「局域网共享」后才会启动。服务对外只暴露用量统计，
//! 不碰任何配置、凭据或会话内容。

use super::{identity, pairing, storage, DeviceInfo, UsageExport};
use crate::database::Database;
use crate::error::AppError;
use axum::http::{HeaderMap, StatusCode};
use axum::{
    extract::{ConnectInfo, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::watch;

/// 运行中的局域网服务句柄。
pub struct LanServerHandle {
    pub port: u16,
    stop_tx: watch::Sender<bool>,
}

impl LanServerHandle {
    /// 停掉 HTTP 服务和 UDP 应答。
    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }
}

/// 服务内部的共享状态。
struct ServerState {
    db: Arc<Database>,
    scan_lock: Arc<tokio::sync::Mutex<()>>,
    jobs: crate::scan_jobs::ScanJobs,
}

/// 启动 HTTP 服务与 UDP 发现应答。
///
/// 端口被占用时返回错误（上层会记日志并提示用户改端口）。
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
        .route("/api/lan/pair", post(pair_handler))
        .route("/api/lan/revoke", post(revoke_handler))
        .route("/api/lan/scan", post(scan_handler))
        .route("/api/lan/scan-status", post(scan_status_handler))
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

/// handler 的错误类型。`AppError` 没实现 axum 的 `IntoResponse`，
/// 这里用 (状态码, 说明) 这种直接可响应的形式。
type HandlerError = (StatusCode, String);

/// `GET /api/lan/info`：自我介绍。
async fn info_handler(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<DeviceInfo>, HandlerError> {
    let info = identity::device_info(&state.db)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(info))
}

/// `GET /api/lan/usage`：导出本机共享范围内的用量。
async fn usage_handler(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Result<Json<UsageExport>, HandlerError> {
    authorized(&state, &headers)?;
    let export = storage::export_local_usage(&state.db)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(export))
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
    let info = tokio::time::timeout(std::time::Duration::from_secs(4), super::client::fetch_info(&host, req.port))
        .await
        .ok().and_then(Result::ok)
        .ok_or_else(|| {
            (
                StatusCode::BAD_GATEWAY,
                "无法连接发起设备，请检查局域网共享和防火墙".into(),
            )
        })?;
    if info.device_id != req.device_id {
        return Err((StatusCode::BAD_REQUEST, "发起设备身份不匹配".into()));
    }
    let token = pairing::accept_bidirectional(
        &state.db,
        &req.code,
        &req.device_id,
        &info.device_name,
        &host,
        req.port,
        &req.reverse_token,
    )
    .map_err(|_| (StatusCode::FORBIDDEN, "配对码无效或已过期".into()))?;
    let db = state.db.clone();
    let peer = req.device_id.clone();
    tokio::spawn(async move {
        let _ = super::client::sync_device(&db, &peer).await;
    });
    Ok(Json(serde_json::json!({"token": token})))
}
fn authorized(state: &ServerState, headers: &HeaderMap) -> Result<String, HandlerError> {
    let peer = headers
        .get("x-observatory-device")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = headers
        .get("x-observatory-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if pairing::authorized(&state.db, peer, token).unwrap_or(false) {
        Ok(peer.to_owned())
    } else {
        Err((StatusCode::FORBIDDEN, "设备未配对".into()))
    }
}
async fn revoke_handler(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Result<Json<bool>, HandlerError> {
    let peer = authorized(&state, &headers)?;
    pairing::revoke(&state.db, &peer)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state
        .db
        .delete_setting(&format!("lan_outgoing_token_v1:{peer}"))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    storage::remove_device(&state.db, &peer)
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
    let peer = authorized(&state, &headers)?;
    let db = state.db.clone();
    let lock = state.scan_lock.clone();
    let job = state
        .jobs
        .start(&peer, &req.key, move || async move {
            let _guard = lock.lock().await;
            let enabled = crate::api::read_enabled_sources(&db).map_err(|e| e.to_string())?;
            let result =
                tokio::task::spawn_blocking(move || crate::api::scan_database(&db, &enabled))
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
    let peer = authorized(&state, &headers)?;
    state
        .jobs
        .get(&peer, &req.key)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "扫描任务不存在或已重启".into()))
}

/// UDP 发现应答：收到探测包就把本机身份回给发送方。
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
        // 先看标志再收包：stop() 可能在进入循环前就已经调过了。
        if *stop_rx.borrow() {
            return Ok(());
        }
        // 用超时包一层而不是 select!：select! 的分支里再借 stop_rx 会撞借用检查，
        // 而且 changed() 分支和这里的 borrow() 不能同时持有这个 receiver。
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
        // 身份取不到就别回：对方会再广播一次。
        if let Ok(info) = identity::device_info(&db) {
            if let Ok(payload) = serde_json::to_vec(&info) {
                let _ = socket.send_to(&payload, peer).await;
            }
        }
    }
}

/// 等到 stop 标志被置位。
///
/// 不用 `Receiver::wait_for`：它返回的 `watch::Ref` 不是 Send，
/// 放进 `tokio::spawn` 的 future 里编译不过。这里用 `borrow` + `changed`
/// 组合，两者都是 Send。
async fn wait_for_stop(mut rx: watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            // 发送端没了：没有别的路径会再置位，直接结束。
            return;
        }
    }
}
