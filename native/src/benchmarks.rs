//! AI model benchmark & leaderboard ingestion (Artificial Analysis API v2).
//!
//! - API key is stored locally in SQLite `settings` (`benchmarks_aa_api_key_v1`) and never
//!   returned in full to the frontend or recorded in activity logs.
//! - Multi-page model responses are merged, normalized, cached in `benchmarks_aa_cache_v1`,
//!   and every upstream HTTP request is traced into `activity::record_http_trace`.

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::database::Database;
use crate::error::AppError;

pub const AA_API_KEY_SETTING: &str = "benchmarks_aa_api_key_v1";
pub const AA_TIER_SETTING: &str = "benchmarks_aa_tier_v1";
pub const AA_CACHE_SETTING: &str = "benchmarks_aa_cache_v1";

const AA_BASE_URL: &str = "https://artificialanalysis.ai/api/v2";
const AA_PAGE_SIZE: u32 = 200;
const AA_MAX_PAGES: u32 = 10;
const USER_AGENT: &str = "Observatory-Desktop/0.1";

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(6))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkModel {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub release_date: Option<String>,
    pub creator_name: String,
    pub creator_id: Option<String>,
    // Evaluation indices
    pub intelligence_index: Option<f64>,
    pub coding_index: Option<f64>,
    pub agentic_index: Option<f64>,
    pub engineering_index: Option<f64>,
    pub finance_index: Option<f64>,
    pub strategy_index: Option<f64>,
    pub legal_index: Option<f64>,
    pub healthcare_index: Option<f64>,
    pub economics_index: Option<f64>,
    // Benchmark cost
    pub eval_total_cost: Option<f64>,
    pub eval_cost_per_task: Option<f64>,
    // Pricing ($ / 1M tokens)
    pub price_input_1m: Option<f64>,
    pub price_output_1m: Option<f64>,
    pub price_cache_hit_1m: Option<f64>,
    pub price_cache_write_1m: Option<f64>,
    // Performance
    pub median_tps: Option<f64>,
    pub mean_tps: Option<f64>,
    pub median_ttft_s: Option<f64>,
    pub mean_ttft_s: Option<f64>,
    pub median_e2e_s: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkSnapshot {
    pub provider: String,
    pub tier: String,
    pub intelligence_index_version: Option<u32>,
    pub fetched_at: i64,
    pub total_pages: u32,
    pub models: Vec<BenchmarkModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarksState {
    pub key_configured: bool,
    pub key_hint: Option<String>,
    pub tier: String,
    pub snapshot: Option<BenchmarkSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarksConfigSave {
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
    pub tier: Option<String>,
}

pub fn mask_api_key(key: &str) -> String {
    let trimmed = key.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 8 {
        return "••••".into();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}••••{tail}")
}

fn normalize_tier(raw: Option<&str>) -> String {
    match raw.map(str::trim) {
        Some("pro") => "pro".into(),
        _ => "free".into(),
    }
}

pub fn load_state(db: &Database) -> Result<BenchmarksState, AppError> {
    let key = db
        .get_setting(AA_API_KEY_SETTING)?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let tier = normalize_tier(db.get_setting(AA_TIER_SETTING)?.as_deref());
    let snapshot = db
        .get_setting(AA_CACHE_SETTING)?
        .and_then(|raw| serde_json::from_str::<BenchmarkSnapshot>(&raw).ok());
    Ok(BenchmarksState {
        key_configured: key.is_some(),
        key_hint: key.as_deref().map(mask_api_key),
        tier,
        snapshot,
    })
}

/// Benchmark settings are written quietly (see [`Database::write_quietly`]): they
/// are not usage data and must not invalidate caches or re-sync paired devices.
pub fn save_config(
    db: &Database,
    input: &BenchmarksConfigSave,
) -> Result<BenchmarksState, AppError> {
    if input.clear_api_key {
        db.delete_setting_quietly(AA_API_KEY_SETTING)?;
    } else if let Some(raw_key) = input.api_key.as_deref() {
        let trimmed = raw_key.trim();
        if trimmed.is_empty() {
            db.delete_setting_quietly(AA_API_KEY_SETTING)?;
        } else {
            db.set_setting_quietly(AA_API_KEY_SETTING, trimmed)?;
        }
    }
    if let Some(tier) = input.tier.as_deref() {
        db.set_setting_quietly(AA_TIER_SETTING, &normalize_tier(Some(tier)))?;
    }
    load_state(db)
}

pub fn load_api_key_and_tier(db: &Database) -> Result<(Option<String>, String), AppError> {
    let key = db
        .get_setting(AA_API_KEY_SETTING)?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let tier = normalize_tier(db.get_setting(AA_TIER_SETTING)?.as_deref());
    Ok((key, tier))
}

pub fn save_snapshot(db: &Database, snapshot: &BenchmarkSnapshot) -> Result<(), AppError> {
    let raw = serde_json::to_string(snapshot)
        .map_err(|e| AppError::Message(format!("序列化模型榜单缓存失败: {e}")))?;
    // Quiet: the leaderboard is not usage data, so saving it must not invalidate
    // usage caches or change the LAN export fingerprint (peers would re-sync).
    db.set_setting_quietly(AA_CACHE_SETTING, &raw)
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|n| n.is_finite())
}

fn str_opt(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn parse_model(item: &Value) -> Option<BenchmarkModel> {
    let id = str_opt(item.get("id"))
        .or_else(|| str_opt(item.get("slug")))
        .or_else(|| str_opt(item.get("name")))?;
    let name = str_opt(item.get("name")).unwrap_or_else(|| id.clone());
    let slug = str_opt(item.get("slug")).unwrap_or_else(|| id.clone());
    let creator = item.get("model_creator");
    let creator_name =
        str_opt(creator.and_then(|c| c.get("name"))).unwrap_or_else(|| "Unknown".into());
    let creator_id = str_opt(creator.and_then(|c| c.get("id")));

    let evals = item.get("evaluations");
    let cost = item.get("artificial_analysis_intelligence_index_cost");
    let pricing = item.get("pricing");
    let perf = item.get("performance");
    let perf_v3 = item.get("performance_v3");

    Some(BenchmarkModel {
        id,
        name,
        slug,
        release_date: str_opt(item.get("release_date")),
        creator_name,
        creator_id,
        intelligence_index: num(evals.and_then(|e| e.get("artificial_analysis_intelligence_index"))),
        coding_index: num(evals.and_then(|e| e.get("artificial_analysis_coding_index"))),
        agentic_index: num(evals.and_then(|e| e.get("artificial_analysis_agentic_index"))),
        engineering_index: num(evals.and_then(|e| e.get("artificial_analysis_engineering_index"))),
        finance_index: num(
            evals.and_then(|e| e.get("artificial_analysis_finance_and_accounting_index"))
        ),
        strategy_index: num(evals.and_then(|e| e.get("artificial_analysis_strategy_and_ops_index"))),
        legal_index: num(evals.and_then(|e| e.get("artificial_analysis_legal_index"))),
        healthcare_index: num(
            evals.and_then(|e| e.get("artificial_analysis_healthcare_and_medical_index"))
        ),
        economics_index: num(evals.and_then(|e| e.get("artificial_analysis_economics_index"))),
        eval_total_cost: num(cost.and_then(|c| c.get("total_cost"))),
        eval_cost_per_task: num(cost
            .and_then(|c| c.get("cost_per_task"))
            .and_then(|c| c.get("total_cost"))),
        price_input_1m: num(pricing.and_then(|p| p.get("price_1m_input_tokens"))),
        price_output_1m: num(pricing.and_then(|p| p.get("price_1m_output_tokens"))),
        price_cache_hit_1m: num(pricing.and_then(|p| p.get("price_1m_cache_hit_tokens"))),
        price_cache_write_1m: num(pricing.and_then(|p| p.get("price_1m_cache_write_tokens"))),
        median_tps: num(perf.and_then(|p| p.get("median_output_tokens_per_second")))
            .or_else(|| num(item.get("median_output_tokens_per_second"))),
        mean_tps: num(perf_v3.and_then(|p| p.get("mean_output_tokens_per_second"))),
        median_ttft_s: num(perf.and_then(|p| p.get("median_time_to_first_token_seconds")))
            .or_else(|| num(item.get("median_time_to_first_token_seconds"))),
        mean_ttft_s: num(perf_v3.and_then(|p| p.get("mean_time_to_first_token_seconds"))),
        median_e2e_s: num(perf.and_then(|p| p.get("median_end_to_end_response_time_seconds"))),
    })
}

struct PageResponse {
    tier: String,
    intelligence_index_version: Option<u32>,
    total_pages: u32,
    has_more: bool,
    models: Vec<BenchmarkModel>,
}

fn parse_page(payload: &Value, default_tier: &str) -> Result<PageResponse, AppError> {
    let tier = str_opt(payload.get("tier")).unwrap_or_else(|| default_tier.to_string());
    let intelligence_index_version = payload
        .get("intelligence_index_version")
        .and_then(Value::as_u64)
        .map(|v| v as u32);
    let pagination = payload.get("pagination");
    let total_pages = pagination
        .and_then(|p| p.get("total_pages"))
        .and_then(Value::as_u64)
        .map(|v| v as u32)
        .unwrap_or(1)
        .clamp(1, AA_MAX_PAGES);
    let has_more = pagination
        .and_then(|p| p.get("has_more"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let arr = payload
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Message("Artificial Analysis 响应缺少 data 列表".into()))?;
    let models = arr.iter().filter_map(parse_model).collect();
    Ok(PageResponse {
        tier,
        intelligence_index_version,
        total_pages,
        has_more,
        models,
    })
}

async fn fetch_page(
    api_key: &str,
    endpoint_path: &str,
    page: u32,
) -> Result<(u16, Value), (Option<u16>, String)> {
    let url = format!("{AA_BASE_URL}{endpoint_path}?page={page}&page_size={AA_PAGE_SIZE}");
    let started = std::time::Instant::now();
    let resp = match http_client()
        .get(&url)
        .header("x-api-key", api_key)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("Artificial Analysis 请求失败: {e}");
            crate::activity::record_http_trace(crate::activity::OutboundHttpTrace {
                provider: "Artificial Analysis".into(),
                method: "GET".into(),
                url,
                http_status: None,
                duration_ms: started.elapsed().as_millis() as i64,
                response_bytes: 0,
                error: Some(msg.clone()),
            });
            return Err((None, msg));
        }
    };

    let status = resp.status();
    let status_code = status.as_u16();
    let bytes = match resp.bytes().await {
        Ok(b) => b.to_vec(),
        Err(e) => {
            let msg = format!("读取 Artificial Analysis 响应失败: {e}");
            crate::activity::record_http_trace(crate::activity::OutboundHttpTrace {
                provider: "Artificial Analysis".into(),
                method: "GET".into(),
                url,
                http_status: Some(status_code),
                duration_ms: started.elapsed().as_millis() as i64,
                response_bytes: 0,
                error: Some(msg.clone()),
            });
            return Err((Some(status_code), msg));
        }
    };

    if !status.is_success() {
        let server_msg = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string));
        let msg = match (status_code, server_msg) {
            (401, _) => "Artificial Analysis API Key 无效或已失效（HTTP 401）".into(),
            (403, Some(detail)) => format!("Artificial Analysis 权限不足（HTTP 403）: {detail}"),
            (429, _) => "Artificial Analysis 请求过于频繁，请稍后再试（HTTP 429）".into(),
            (_, Some(detail)) => format!("Artificial Analysis HTTP {status_code}: {detail}"),
            _ => format!("Artificial Analysis HTTP {status_code}"),
        };
        crate::activity::record_http_trace(crate::activity::OutboundHttpTrace {
            provider: "Artificial Analysis".into(),
            method: "GET".into(),
            url,
            http_status: Some(status_code),
            duration_ms: started.elapsed().as_millis() as i64,
            response_bytes: bytes.len(),
            error: Some(msg.clone()),
        });
        return Err((Some(status_code), msg));
    }

    let parsed: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("解析 Artificial Analysis JSON 失败: {e}");
            crate::activity::record_http_trace(crate::activity::OutboundHttpTrace {
                provider: "Artificial Analysis".into(),
                method: "GET".into(),
                url,
                http_status: Some(status_code),
                duration_ms: started.elapsed().as_millis() as i64,
                response_bytes: bytes.len(),
                error: Some(msg.clone()),
            });
            return Err((Some(status_code), msg));
        }
    };

    crate::activity::record_http_trace(crate::activity::OutboundHttpTrace {
        provider: "Artificial Analysis".into(),
        method: "GET".into(),
        url,
        http_status: Some(status_code),
        duration_ms: started.elapsed().as_millis() as i64,
        response_bytes: bytes.len(),
        error: None,
    });
    Ok((status_code, parsed))
}

/// Fetch all pages from Artificial Analysis API v2.
/// If `preferred_tier == "pro"` and the Pro endpoint returns 403, automatically falls back to `/free`.
pub async fn fetch_artificial_analysis(
    api_key: &str,
    preferred_tier: &str,
) -> Result<BenchmarkSnapshot, AppError> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err(AppError::InvalidInput(
            "请先配置 Artificial Analysis API Key".into(),
        ));
    }

    let mut endpoint_path = if preferred_tier == "pro" {
        "/language/models"
    } else {
        "/language/models/free"
    };

    let first_payload = match fetch_page(key, endpoint_path, 1).await {
        Ok((_, val)) => val,
        Err((Some(403), _)) if endpoint_path == "/language/models" => {
            endpoint_path = "/language/models/free";
            fetch_page(key, endpoint_path, 1)
                .await
                .map_err(|(_, msg)| AppError::Message(msg))?
                .1
        }
        Err((_, msg)) => return Err(AppError::Message(msg)),
    };

    let first = parse_page(&first_payload, preferred_tier)?;
    let total_pages = first.total_pages;
    let tier = first.tier;
    let intelligence_index_version = first.intelligence_index_version;

    let mut seen: HashSet<String> = HashSet::new();
    let mut models: Vec<BenchmarkModel> = Vec::new();
    for m in first.models {
        if seen.insert(m.id.clone()) {
            models.push(m);
        }
    }

    if first.has_more && total_pages > 1 {
        for page in 2..=total_pages {
            let (_, payload) = fetch_page(key, endpoint_path, page)
                .await
                .map_err(|(_, msg)| AppError::Message(msg))?;
            let parsed = parse_page(&payload, &tier)?;
            for m in parsed.models {
                if seen.insert(m.id.clone()) {
                    models.push(m);
                }
            }
            if !parsed.has_more {
                break;
            }
        }
    }

    // Sort by intelligence_index desc (None last), then name asc.
    models.sort_by(|a, b| {
        match (a.intelligence_index, b.intelligence_index) {
            (Some(x), Some(y)) => y.total_cmp(&x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then_with(|| a.name.cmp(&b.name))
    });

    Ok(BenchmarkSnapshot {
        provider: "artificial_analysis".into(),
        tier,
        intelligence_index_version,
        fetched_at: chrono::Utc::now().timestamp_millis(),
        total_pages,
        models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn masks_api_key_without_leaking_middle() {
        assert_eq!(mask_api_key("aa_1234567890abcd"), "aa_1••••abcd");
        assert_eq!(mask_api_key("short"), "••••");
    }

    #[test]
    fn parses_artificial_analysis_v2_page_payload() {
        let payload = json!({
            "tier": "free",
            "intelligence_index_version": 5,
            "pagination": {
                "page": 1,
                "page_size": 200,
                "total_pages": 4,
                "has_more": true
            },
            "data": [
                {
                    "id": "m-1",
                    "name": "Claude Opus 5.5 (Max)",
                    "slug": "claude-opus-5-5-max",
                    "release_date": "2026-11-01",
                    "model_creator": { "id": "c-1", "name": "Anthropic" },
                    "evaluations": {
                        "artificial_analysis_intelligence_index": 59.1,
                        "artificial_analysis_engineering_index": 64.7,
                        "artificial_analysis_coding_index": null
                    },
                    "artificial_analysis_intelligence_index_cost": {
                        "total_cost": 3359.66,
                        "cost_per_task": { "total_cost": 3.80 }
                    },
                    "pricing": {
                        "price_1m_input_tokens": 4.0,
                        "price_1m_output_tokens": 20.0,
                        "price_1m_cache_hit_tokens": 0.4,
                        "price_1m_cache_write_tokens": 5.0
                    },
                    "performance": {
                        "median_output_tokens_per_second": 98.5,
                        "median_time_to_first_token_seconds": 2.4,
                        "median_end_to_end_response_time_seconds": 14.2
                    },
                    "performance_v3": {
                        "mean_output_tokens_per_second": 114.23,
                        "mean_time_to_first_token_seconds": 2.1
                    }
                }
            ]
        });

        let page = parse_page(&payload, "free").expect("page should parse");
        assert_eq!(page.tier, "free");
        assert_eq!(page.intelligence_index_version, Some(5));
        assert_eq!(page.total_pages, 4);
        assert!(page.has_more);
        assert_eq!(page.models.len(), 1);
        let m = &page.models[0];
        assert_eq!(m.name, "Claude Opus 5.5 (Max)");
        assert_eq!(m.creator_name, "Anthropic");
        assert_eq!(m.intelligence_index, Some(59.1));
        assert_eq!(m.engineering_index, Some(64.7));
        assert_eq!(m.coding_index, None);
        assert_eq!(m.eval_cost_per_task, Some(3.8));
        assert_eq!(m.price_input_1m, Some(4.0));
        assert_eq!(m.price_output_1m, Some(20.0));
        assert_eq!(m.mean_tps, Some(114.23));
        assert_eq!(m.median_tps, Some(98.5));
        assert_eq!(m.median_e2e_s, Some(14.2));
    }

    #[test]
    fn saving_benchmark_settings_leaves_the_data_epoch_alone() {
        let db = Database::memory().expect("memory db");
        let before = db.data_epoch();
        save_config(
            &db,
            &BenchmarksConfigSave {
                api_key: Some("aa-test-key-1234".into()),
                clear_api_key: false,
                tier: Some("pro".into()),
            },
        )
        .expect("save config");
        let snap = BenchmarkSnapshot {
            provider: "artificial_analysis".into(),
            tier: "pro".into(),
            intelligence_index_version: None,
            fetched_at: 0,
            total_pages: 1,
            models: vec![],
        };
        save_snapshot(&db, &snap).expect("save snapshot");
        save_config(
            &db,
            &BenchmarksConfigSave {
                api_key: None,
                clear_api_key: true,
                tier: None,
            },
        )
        .expect("clear key");
        assert_eq!(db.data_epoch(), before, "no usage cache or LAN re-sync");
    }

    #[test]
    fn config_and_snapshot_roundtrip_in_database() {
        let db = Database::memory().expect("memory db");
        let empty = load_state(&db).expect("load empty");
        assert!(!empty.key_configured);
        assert_eq!(empty.tier, "free");
        assert!(empty.snapshot.is_none());

        let saved = save_config(
            &db,
            &BenchmarksConfigSave {
                api_key: Some("aa_testkey1234567890".into()),
                clear_api_key: false,
                tier: Some("pro".into()),
            },
        )
        .expect("save config");
        assert!(saved.key_configured);
        assert_eq!(saved.key_hint.as_deref(), Some("aa_t••••7890"));
        assert_eq!(saved.tier, "pro");

        let snap = BenchmarkSnapshot {
            provider: "artificial_analysis".into(),
            tier: "free".into(),
            intelligence_index_version: Some(5),
            fetched_at: 1_700_000_000_000,
            total_pages: 4,
            models: vec![],
        };
        save_snapshot(&db, &snap).expect("save snap");
        let loaded = load_state(&db).expect("load snap");
        assert_eq!(loaded.snapshot, Some(snap));

        let cleared = save_config(
            &db,
            &BenchmarksConfigSave {
                api_key: None,
                clear_api_key: true,
                tier: None,
            },
        )
        .expect("clear key");
        assert!(!cleared.key_configured);
        assert!(cleared.key_hint.is_none());
    }
}
