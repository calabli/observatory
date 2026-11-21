//! Deep analytics ("Insights") aggregation over the local or projected SQLite database.
//!
//! All metrics are derived strictly from real rows already stored in `proxy_request_logs`,
//! `usage_daily_rollups`, and `model_pricing` — no extra collectors or synthetic numbers.
use crate::{
    database::Database,
    error::AppError,
    services::{
        sql_helpers::fresh_input_sql,
        usage_stats::{
            SPEED_ESTIMATE_MIN_DURATION_MS, SPEED_ESTIMATE_MIN_OUTPUT_TOKENS,
            SPEED_MIN_GENERATION_MS, SPEED_MIN_OUTPUT_TOKENS,
        },
    },
};
use chrono::{Datelike, Local, TimeZone, Timelike};
use rusqlite::{params_from_iter, types::Value as SqlValue, Connection};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsData {
    pub cache_savings: CacheSavings,
    pub forecast: CostForecast,
    pub rhythm: Vec<RhythmCell>,
    pub sessions: Vec<SessionBubble>,
    pub stability: Vec<ModelStability>,
    pub token_mix: Vec<DailyTokenMix>,
    pub status: StatusHealth,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheSavings {
    pub actual_cost_usd: f64,
    pub cache_read_cost_usd: f64,
    pub saved_usd: f64,
    pub cache_hit_rate: f64,
    pub total_requests: u64,
    pub avg_context_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyCostPoint {
    pub date: String,
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostForecast {
    pub current_cost_usd: f64,
    pub previous_cost_usd: f64,
    pub delta_pct: Option<f64>,
    pub elapsed_days: f64,
    pub daily_avg_usd: f64,
    pub projected_30d_usd: f64,
    pub daily_costs: Vec<DailyCostPoint>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RhythmCell {
    /// 0 = Monday .. 6 = Sunday (local time).
    pub weekday: u8,
    /// 0 .. 23 (local time).
    pub hour: u8,
    pub requests: u64,
    pub cost_usd: f64,
    pub speed_output_tokens: u64,
    pub speed_duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBubble {
    pub session_id: String,
    pub app_type: String,
    pub turns: u64,
    pub cost_usd: f64,
    pub avg_context_tokens: u64,
    pub output_tokens: u64,
    pub last_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStability {
    pub model: String,
    pub requests: u64,
    pub ttft_p50_ms: Option<u64>,
    pub ttft_p95_ms: Option<u64>,
    pub ttft_estimated: bool,
    pub speed_p10: Option<f64>,
    pub speed_p50: Option<f64>,
    pub speed_estimated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyTokenMix {
    pub date: String,
    pub fresh_input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusHealth {
    pub total: u64,
    pub ok_2xx: u64,
    pub rate_limit_429: u64,
    pub server_5xx: u64,
    pub other_error: u64,
    pub peak_429_hour: Option<u8>,
}

fn push_detail_filters(
    alias: &str,
    start_date: Option<i64>,
    end_date: Option<i64>,
    app_type: Option<&str>,
) -> (String, Vec<SqlValue>) {
    let mut conds = Vec::new();
    let mut params = Vec::new();
    if let Some(s) = start_date {
        conds.push(format!("{alias}.created_at >= ?"));
        params.push(SqlValue::Integer(s));
    }
    if let Some(e) = end_date {
        conds.push(format!("{alias}.created_at < ?"));
        params.push(SqlValue::Integer(e));
    }
    if let Some(app) = app_type.filter(|a| !a.is_empty()) {
        if app == "claude" {
            conds.push(format!("{alias}.app_type IN ('claude', 'claude-desktop')"));
        } else {
            conds.push(format!("{alias}.app_type = ?"));
            params.push(SqlValue::Text(app.to_string()));
        }
    }
    let where_sql = if conds.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conds.join(" AND "))
    };
    (where_sql, params)
}

fn percentile_u64(sorted: &[u64], pct: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let idx = ((sorted.len() - 1) as f64 * pct).round() as usize;
    sorted.get(idx.min(sorted.len() - 1)).copied()
}

fn percentile_f64(sorted: &[f64], pct: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let idx = ((sorted.len() - 1) as f64 * pct).round() as usize;
    sorted.get(idx.min(sorted.len() - 1)).copied()
}

pub fn query_insights_on_conn(
    conn: &Connection,
    start_date: Option<i64>,
    end_date: Option<i64>,
    app_type: Option<&str>,
) -> Result<InsightsData, AppError> {
    let now_ts = chrono::Utc::now().timestamp();
    let effective_end = end_date.unwrap_or(now_ts);
    let fresh_sql = fresh_input_sql("l");
    let (where_sql, params) = push_detail_filters("l", start_date, end_date, app_type);

    // 1. Summary + cache savings (from detail + rollup totals via existing summary + detail pricing join).
    let summary =
        Database::usage_summary_impl(conn, start_date, end_date, app_type, None, None, false)?;
    let actual_cost_usd = summary.total_cost.parse::<f64>().unwrap_or(0.0);

    // Compute exact cache savings on detail rows using the same `find_model_pricing`
    // candidate/prefix resolution as CostCalculator (handles tiered IDs like
    // `gemini-3.8-flash-high`, dated Claude/Codex IDs, etc.).
    let savings_sql = format!(
        "SELECT
            COALESCE(NULLIF(l.pricing_model, ''), l.model) AS pricing_key,
            COALESCE(SUM(CAST(l.cache_read_cost_usd AS REAL)), 0.0) AS cache_read_cost,
            COALESCE(SUM(
                l.cache_read_tokens * CASE
                    WHEN CAST(l.cost_multiplier AS REAL) > 0 THEN CAST(l.cost_multiplier AS REAL)
                    ELSE 1.0
                END
            ), 0.0) AS weighted_cache_read_tokens
         FROM proxy_request_logs l
         {where_sql}
         GROUP BY pricing_key"
    );
    let mut cache_read_cost_usd = 0.0f64;
    let mut saved_usd = 0.0f64;
    {
        let mut stmt = conn.prepare(&savings_sql)?;
        let mut rows = stmt.query(params_from_iter(params.iter()))?;
        while let Some(row) = rows.next()? {
            let pricing_key: String = row.get(0)?;
            let row_cache_cost: f64 = row.get(1)?;
            let weighted_tokens: f64 = row.get(2)?;
            cache_read_cost_usd += row_cache_cost;
            if weighted_tokens > 0.0 {
                if let Some(pricing) =
                    crate::services::usage_stats::find_model_pricing(conn, &pricing_key)
                {
                    let input_per_m = pricing
                        .input_cost_per_million
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0);
                    let cache_per_m = pricing
                        .cache_read_cost_per_million
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0);
                    if input_per_m > cache_per_m {
                        let hypothetical = (weighted_tokens / 1_000_000.0) * input_per_m;
                        let actual_cache = if row_cache_cost > 0.0 {
                            row_cache_cost
                        } else {
                            (weighted_tokens / 1_000_000.0) * cache_per_m
                        };
                        saved_usd += (hypothetical - actual_cache).max(0.0);
                    }
                }
            }
        }
    }
    let context_sum = summary.total_input_tokens
        + summary.total_cache_read_tokens
        + summary.total_cache_creation_tokens;
    let avg_context_tokens = if summary.total_requests > 0 {
        context_sum / summary.total_requests
    } else {
        0
    };
    let cache_savings = CacheSavings {
        actual_cost_usd,
        cache_read_cost_usd,
        saved_usd,
        cache_hit_rate: summary.cache_hit_rate,
        total_requests: summary.total_requests,
        avg_context_tokens,
    };

    // 2. Daily trends for token mix + forecast sparkline.
    let trends = Database::daily_trends_impl(
        conn,
        start_date,
        Some(effective_end),
        app_type,
        None,
        None,
        false,
    )?;
    let daily_costs: Vec<DailyCostPoint> = trends
        .iter()
        .map(|d| DailyCostPoint {
            date: d.date.clone(),
            cost: d.total_cost.parse::<f64>().unwrap_or(0.0),
        })
        .collect();
    let token_mix: Vec<DailyTokenMix> = trends
        .iter()
        .map(|d| DailyTokenMix {
            date: d.date.clone(),
            fresh_input: d.total_input_tokens,
            output: d.total_output_tokens,
            cache_creation: d.total_cache_creation_tokens,
            cache_read: d.total_cache_read_tokens,
        })
        .collect();

    // Forecast & period-over-period delta.
    // Anchor the daily average to the later of `start_date` and the app's first usage date
    // so unused days before the user started using a tool never dilute its burn rate.
    let first_usage_ts = Database::first_usage_date_on_conn(conn, app_type, None, None)?
        .as_deref()
        .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|dt| Local.from_local_datetime(&dt).earliest())
        .map(|dt| dt.timestamp());
    let (elapsed_days, previous_cost_usd) = if let Some(s) = start_date {
        let span = (effective_end - s).max(3600);
        let span_days = ((span + 86_399) / 86_400).max(1);
        let prev_start = Local
            .timestamp_opt(s, 0)
            .single()
            .and_then(|dt| {
                dt.date_naive()
                    .checked_sub_days(chrono::Days::new(span_days as u64))
            })
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .and_then(|dt| Local.from_local_datetime(&dt).earliest())
            .map(|dt| dt.timestamp())
            .unwrap_or(s - span_days * 86_400);
        let prev = Database::usage_summary_impl(
            conn,
            Some(prev_start),
            Some(s),
            app_type,
            None,
            None,
            false,
        )
        .ok()
        .and_then(|u| u.total_cost.parse::<f64>().ok())
        .unwrap_or(0.0);
        let effective_start = first_usage_ts
            .map(|f| s.max(f))
            .unwrap_or(s)
            .min(effective_end);
        let active_span = (effective_end - effective_start).max(3600);
        ((active_span as f64 / 86_400.0).max(1.0), prev)
    } else {
        let days = (trends.len() as f64).max(1.0);
        (days, 0.0)
    };
    let daily_avg_usd = actual_cost_usd / elapsed_days;
    // Never project below actual spend when the selected calendar month has 31 days.
    let projected_30d_usd = daily_avg_usd * 30.0f64.max(elapsed_days);
    let delta_pct = (previous_cost_usd > 0.0001)
        .then(|| ((actual_cost_usd - previous_cost_usd) / previous_cost_usd) * 100.0);
    let forecast = CostForecast {
        current_cost_usd: actual_cost_usd,
        previous_cost_usd,
        delta_pct,
        elapsed_days,
        daily_avg_usd,
        projected_30d_usd,
        daily_costs,
    };

    // 3. Single pass over detail rows for: 7x24 rhythm, status breakdown, and per-model stability.
    let detail_sql = format!(
        "SELECT
            l.created_at,
            l.model,
            l.output_tokens,
            CAST(l.total_cost_usd AS REAL),
            l.latency_ms,
            l.first_token_ms,
            l.status_code,
            COALESCE(l.data_source, 'proxy')
         FROM proxy_request_logs l
         {where_sql}"
    );
    let mut rhythm_map: HashMap<(u8, u8), (u64, f64, u64, u64)> = HashMap::new();
    let mut status_total = 0u64;
    let mut ok_2xx = 0u64;
    let mut rate_limit_429 = 0u64;
    let mut server_5xx = 0u64;
    let mut other_error = 0u64;
    let mut rate_limit_by_hour: [u64; 24] = [0; 24];

    struct ModelSamples {
        requests: u64,
        exact_ttft: Vec<u64>,
        est_ttft: Vec<u64>,
        exact_speeds: Vec<f64>,
        est_speeds: Vec<f64>,
    }
    let mut model_samples: HashMap<String, ModelSamples> = HashMap::new();

    {
        let mut stmt = conn.prepare(&detail_sql)?;
        let mut rows = stmt.query(params_from_iter(params.iter()))?;
        while let Some(row) = rows.next()? {
            let created_at: i64 = row.get(0)?;
            let model: String = row.get(1)?;
            let output_tokens: i64 = row.get(2)?;
            let cost_usd: f64 = row.get(3)?;
            let latency_ms: i64 = row.get(4)?;
            let first_token_ms: Option<i64> = row.get(5)?;
            let status_code: i64 = row.get(6)?;
            let data_source: String = row.get(7)?;

            let local_dt = Local.timestamp_opt(created_at, 0).single();
            let (weekday, hour) = match local_dt {
                Some(dt) => (dt.weekday().num_days_from_monday() as u8, dt.hour() as u8),
                None => (0, 0),
            };

            // Status breakdown
            status_total += 1;
            if (200..300).contains(&status_code) {
                ok_2xx += 1;
            } else if status_code == 429 {
                rate_limit_429 += 1;
                rate_limit_by_hour[hour as usize] += 1;
            } else if (500..600).contains(&status_code) {
                server_5xx += 1;
            } else {
                other_error += 1;
            }

            // Speed eligibility (same rules as usage_stats.rs)
            let is_proxy = data_source == "proxy";
            let mut rhythm_speed_out = 0u64;
            let mut rhythm_speed_ms = 0u64;

            let entry = model_samples.entry(model).or_insert_with(|| ModelSamples {
                requests: 0,
                exact_ttft: Vec::new(),
                est_ttft: Vec::new(),
                exact_speeds: Vec::new(),
                est_speeds: Vec::new(),
            });
            entry.requests += 1;

            if is_proxy {
                if let Some(ft) = first_token_ms {
                    if (200..300).contains(&status_code) && ft >= 0 {
                        entry.exact_ttft.push(ft as u64);
                    }
                    let gen_ms = latency_ms - ft;
                    if output_tokens >= SPEED_MIN_OUTPUT_TOKENS && gen_ms >= SPEED_MIN_GENERATION_MS
                    {
                        rhythm_speed_out = output_tokens as u64;
                        rhythm_speed_ms = gen_ms as u64;
                        entry
                            .exact_speeds
                            .push((output_tokens as f64 * 1000.0) / gen_ms as f64);
                    }
                }
            } else {
                if let Some(ft) = first_token_ms.filter(|v| *v >= 0) {
                    entry.est_ttft.push(ft as u64);
                }
                if output_tokens >= SPEED_ESTIMATE_MIN_OUTPUT_TOKENS
                    && latency_ms >= SPEED_ESTIMATE_MIN_DURATION_MS
                {
                    rhythm_speed_out = output_tokens as u64;
                    rhythm_speed_ms = latency_ms as u64;
                    entry
                        .est_speeds
                        .push((output_tokens as f64 * 1000.0) / latency_ms as f64);
                }
            }

            let cell = rhythm_map.entry((weekday, hour)).or_insert((0, 0.0, 0, 0));
            cell.0 += 1;
            cell.1 += cost_usd;
            cell.2 += rhythm_speed_out;
            cell.3 += rhythm_speed_ms;
        }
    }

    let mut rhythm: Vec<RhythmCell> = rhythm_map
        .into_iter()
        .map(
            |((weekday, hour), (requests, cost_usd, speed_output_tokens, speed_duration_ms))| {
                RhythmCell {
                    weekday,
                    hour,
                    requests,
                    cost_usd,
                    speed_output_tokens,
                    speed_duration_ms,
                }
            },
        )
        .collect();
    rhythm.sort_by_key(|c| (c.weekday, c.hour));

    let peak_429_hour = rate_limit_by_hour
        .iter()
        .enumerate()
        .filter(|(_, &count)| count > 0)
        .max_by_key(|(_, &count)| count)
        .map(|(hour, _)| hour as u8);
    let status = StatusHealth {
        total: status_total,
        ok_2xx,
        rate_limit_429,
        server_5xx,
        other_error,
        peak_429_hour,
    };

    // Stability: top 6 models by request count that have either TTFT or speed data.
    let mut stability_items: Vec<(String, ModelSamples)> = model_samples.into_iter().collect();
    stability_items.sort_by(|a, b| b.1.requests.cmp(&a.1.requests));
    let mut stability = Vec::new();
    for (model, mut samples) in stability_items {
        let has_ttft = !samples.exact_ttft.is_empty() || !samples.est_ttft.is_empty();
        let has_speed = !samples.exact_speeds.is_empty() || !samples.est_speeds.is_empty();
        if !has_ttft && !has_speed {
            continue;
        }
        let (ttft_p50_ms, ttft_p95_ms, ttft_estimated) = if !samples.exact_ttft.is_empty() {
            samples.exact_ttft.sort_unstable();
            (
                percentile_u64(&samples.exact_ttft, 0.50),
                percentile_u64(&samples.exact_ttft, 0.95),
                false,
            )
        } else {
            samples.est_ttft.sort_unstable();
            (
                percentile_u64(&samples.est_ttft, 0.50),
                percentile_u64(&samples.est_ttft, 0.95),
                true,
            )
        };
        let (speed_p10, speed_p50, speed_estimated) = if !samples.exact_speeds.is_empty() {
            samples
                .exact_speeds
                .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            (
                percentile_f64(&samples.exact_speeds, 0.10),
                percentile_f64(&samples.exact_speeds, 0.50),
                false,
            )
        } else {
            samples
                .est_speeds
                .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            (
                percentile_f64(&samples.est_speeds, 0.10),
                percentile_f64(&samples.est_speeds, 0.50),
                true,
            )
        };
        stability.push(ModelStability {
            model,
            requests: samples.requests,
            ttft_p50_ms,
            ttft_p95_ms,
            ttft_estimated,
            speed_p10,
            speed_p50,
            speed_estimated,
        });
        if stability.len() >= 6 {
            break;
        }
    }

    // 4. Session health bubbles (top 40 sessions by cost).
    let session_where = if where_sql.is_empty() {
        "WHERE l.session_id IS NOT NULL AND l.session_id <> ''".to_string()
    } else {
        format!("{where_sql} AND l.session_id IS NOT NULL AND l.session_id <> ''")
    };
    let sessions_sql = format!(
        "SELECT
            l.session_id,
            CASE WHEN l.app_type = 'claude-desktop' THEN 'claude' ELSE l.app_type END AS app_norm,
            COUNT(*) AS turns,
            COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0.0) AS cost_usd,
            COALESCE(AVG(({fresh_sql}) + l.cache_read_tokens + l.cache_creation_tokens), 0.0) AS avg_ctx,
            COALESCE(SUM(l.output_tokens), 0) AS out_tokens,
            MAX(l.created_at) AS last_at
         FROM proxy_request_logs l
         {session_where}
         GROUP BY l.session_id, app_norm
         ORDER BY cost_usd DESC, turns DESC
         LIMIT 40"
    );
    let mut sessions = Vec::new();
    {
        let mut stmt = conn.prepare(&sessions_sql)?;
        let mut rows = stmt.query(params_from_iter(params.iter()))?;
        while let Some(row) = rows.next()? {
            sessions.push(SessionBubble {
                session_id: row.get(0)?,
                app_type: row.get(1)?,
                turns: row.get::<_, i64>(2)?.max(0) as u64,
                cost_usd: row.get(3)?,
                avg_context_tokens: row.get::<_, f64>(4)?.max(0.0).round() as u64,
                output_tokens: row.get::<_, i64>(5)?.max(0) as u64,
                last_at: row.get(6)?,
            });
        }
    }

    Ok(InsightsData {
        cache_savings,
        forecast,
        rhythm,
        sessions,
        stability,
        token_mix,
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::sql_helpers::INPUT_TOKEN_SEMANTICS_FRESH;

    #[test]
    fn insights_computes_cache_savings_rhythm_sessions_stability_and_status() {
        let db = Database::memory().unwrap();
        let now = chrono::Utc::now().timestamp();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO model_pricing
                 (model_id, display_name, input_cost_per_million, output_cost_per_million, cache_read_cost_per_million, cache_creation_cost_per_million)
                 VALUES ('test-model', 'Test Model', '10', '20', '1', '12.5')",
                [],
            )
            .unwrap();
            // Use a tiered Antigravity model ID (`test-model-high`) while model_pricing only stores `test-model`
            for (idx, (latency, out, status)) in
                [(4000, 400, 200), (8000, 600, 200), (2000, 50, 429)]
                    .iter()
                    .enumerate()
            {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model, request_model,
                        input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                        input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
                        latency_ms, first_token_ms, status_code, session_id, is_streaming, cost_multiplier,
                        created_at, data_source, input_token_semantics
                    ) VALUES (?1, 'p1', 'agy', 'test-model-high', 'Test Model (High)',
                        1000, ?2, 1000000, 0,
                        '0.01', '0.01', '1.0', '0', '1.02',
                        ?3, 500, ?4, 'session-a', 1, '1',
                        ?5, 'agy_session', ?6)",
                    rusqlite::params![
                        format!("r-{idx}"),
                        out,
                        latency,
                        status,
                        now - 60 + idx as i64,
                        INPUT_TOKEN_SEMANTICS_FRESH
                    ],
                )
                .unwrap();
            }
        }

        let data = db
            .with_read_connection(|conn| {
                query_insights_on_conn(conn, Some(now - 3600), Some(now + 60), Some("agy"))
            })
            .unwrap();

        assert_eq!(data.cache_savings.total_requests, 3);
        assert!((data.cache_savings.saved_usd - 27.0).abs() < 0.01);
        assert_eq!(data.sessions.len(), 1);
        assert_eq!(data.sessions[0].turns, 3);
        assert_eq!(data.status.total, 3);
        assert_eq!(data.status.ok_2xx, 2);
        assert_eq!(data.status.rate_limit_429, 1);
        assert_eq!(data.stability.len(), 1);
        assert_eq!(data.stability[0].ttft_p50_ms, Some(500));
        assert!(data.stability[0].speed_p50.is_some());
        assert!(!data.rhythm.is_empty());
    }
}
