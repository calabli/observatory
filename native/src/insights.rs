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
use std::collections::{BTreeSet, HashMap};
use std::io::BufRead;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsData {
    pub cache_savings: CacheSavings,
    pub forecast: CostForecast,
    pub rhythm: Vec<RhythmCell>,
    pub sessions: Vec<SessionBubble>,
    pub projects: Vec<ProjectCostRow>,
    pub stability: Vec<ModelStability>,
    pub token_mix: Vec<DailyTokenMix>,
    pub status: StatusHealth,
    pub subscriptions: Vec<SubscriptionRoi>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCostRow {
    pub project_dir: String,
    pub project_name: String,
    pub cost_usd: f64,
    pub requests: u64,
    pub sessions: u64,
    pub total_tokens: u64,
    pub apps: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionRoi {
    pub id: String,
    pub name: String,
    pub app_type: String,
    pub price: f64,
    pub currency: String,
    pub billing: String,
    pub custom_interval_days: Option<u32>,
    /// Equivalent API cost (USD) in the current selected range for this subscription's app.
    pub range_api_cost_usd: f64,
    /// Equivalent API cost (USD) in the subscription's current weekly quota cycle.
    pub cycle_api_cost_usd: f64,
    pub cycle_requests: u64,
    /// ROI multiple (only computed when currency is USD and price > 0, never using fake FX rates).
    pub usd_roi_multiple: Option<f64>,
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
    pub today_cost_usd: f64,
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
    pub title: Option<String>,
    pub project_dir: Option<String>,
    pub turns: u64,
    pub cost_usd: f64,
    pub avg_context_tokens: u64,
    pub total_tokens: u64,
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
        // Inclusive, like the usage statistics: a closed day ends at 23:59:59.
        conds.push(format!("{alias}.created_at <= ?"));
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
    all_subscriptions: &[crate::subscriptions::Subscription],
) -> Result<InsightsData, AppError> {
    let now_ts = chrono::Utc::now().timestamp();
    let effective_end = end_date.unwrap_or(now_ts);
    let fresh_sql = fresh_input_sql("l");
    let (base_where, params) = push_detail_filters("l", start_date, end_date, app_type);
    let dedup_sql = crate::services::usage_stats::effective_usage_log_filter_for_range(
        conn, "l", start_date, end_date,
    )?;
    let where_sql = if dedup_sql == "1 = 1" {
        base_where
    } else if base_where.is_empty() {
        format!("WHERE {dedup_sql}")
    } else {
        format!("{base_where} AND {dedup_sql}")
    };

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
        // Compare like with like: the current period runs from `s` to now, so only the same
        // length of the previous period counts (a full previous window against a
        // partly elapsed current one would always read as a drop early in the day).
        let prev_end = prev_start.saturating_add(effective_end - s).min(s);
        let prev = Database::usage_summary_impl(
            conn,
            Some(prev_start),
            Some(prev_end),
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
    let today_start_ts = Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| Local.from_local_datetime(&dt).earliest())
        .map(|dt| dt.timestamp());
    let today_cost_usd = today_start_ts
        .and_then(|ts| {
            Database::usage_summary_impl(
                conn,
                Some(ts),
                Some(now_ts + 1),
                app_type,
                None,
                None,
                false,
            )
            .ok()
        })
        .and_then(|u| u.total_cost.parse::<f64>().ok())
        .unwrap_or(0.0);
    let forecast = CostForecast {
        current_cost_usd: actual_cost_usd,
        today_cost_usd,
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

    // 4. Session health bubbles + searchable session list (top 120 sessions by cost)
    // plus project/repo cost attribution via batch-cached local session metadata.
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
            COALESCE(SUM(({fresh_sql}) + l.output_tokens + l.cache_read_tokens + l.cache_creation_tokens), 0) AS total_tokens,
            COALESCE(SUM(l.output_tokens), 0) AS out_tokens,
            MAX(l.created_at) AS last_at
         FROM proxy_request_logs l
         {session_where}
         GROUP BY l.session_id, app_norm
         ORDER BY cost_usd DESC, turns DESC
         LIMIT 120"
    );
    let mut sessions = Vec::new();
    {
        let mut stmt = conn.prepare(&sessions_sql)?;
        let mut rows = stmt.query(params_from_iter(params.iter()))?;
        while let Some(row) = rows.next()? {
            sessions.push(SessionBubble {
                session_id: row.get(0)?,
                app_type: row.get(1)?,
                title: None,
                project_dir: None,
                turns: row.get::<_, i64>(2)?.max(0) as u64,
                cost_usd: row.get(3)?,
                avg_context_tokens: row.get::<_, f64>(4)?.max(0.0).round() as u64,
                total_tokens: row.get::<_, i64>(5)?.max(0) as u64,
                output_tokens: row.get::<_, i64>(6)?.max(0) as u64,
                last_at: row.get(7)?,
            });
        }
    }

    // Resolve session titles & project_dir in batch (cached in memory per session).
    let pairs: Vec<(String, String)> = sessions
        .iter()
        .map(|s| (s.app_type.clone(), s.session_id.clone()))
        .collect();
    let meta_map = resolve_sessions_batch(&pairs);
    let mut project_agg: HashMap<String, (f64, u64, u64, u64, BTreeSet<String>)> = HashMap::new();
    for s in &mut sessions {
        if let Some(meta) = meta_map.get(&(s.app_type.clone(), s.session_id.clone())) {
            s.title = meta.title.clone();
            s.project_dir = meta.project_dir.clone();
            if let Some(dir) = &meta.project_dir {
                let norm = dir.trim_end_matches(['/', '\\']).to_string();
                if !norm.is_empty() {
                    let entry = project_agg
                        .entry(norm)
                        .or_insert_with(|| (0.0, 0, 0, 0, BTreeSet::new()));
                    entry.0 += s.cost_usd;
                    entry.1 += s.turns;
                    entry.2 += 1;
                    entry.3 += s.total_tokens;
                    entry.4.insert(s.app_type.clone());
                }
            }
        }
    }
    let mut projects: Vec<ProjectCostRow> = project_agg
        .into_iter()
        .map(
            |(project_dir, (cost_usd, requests, sess_count, total_tokens, apps))| {
                let project_name = std::path::Path::new(&project_dir)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .filter(|n| !n.is_empty())
                    .unwrap_or(&project_dir)
                    .to_string();
                ProjectCostRow {
                    project_dir,
                    project_name,
                    cost_usd,
                    requests,
                    sessions: sess_count,
                    total_tokens,
                    apps: apps.into_iter().collect(),
                }
            },
        )
        .collect();
    projects.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.requests.cmp(&a.requests))
    });
    projects.truncate(12);

    // 5. Active subscription ROI (honest currency handling: USD computes exact multiple;
    // non-USD reports native price next to USD API equivalent without fake FX conversion).
    let now_local = Local::now();
    let today_local = now_local.date_naive();
    let mut subscriptions = Vec::new();
    for sub in all_subscriptions {
        if !sub.is_active_on(today_local) {
            continue;
        }
        if let Some(filter_app) = app_type.filter(|a| !a.is_empty()) {
            let sub_norm = if sub.app_type == "claude-desktop" {
                "claude"
            } else {
                sub.app_type.as_str()
            };
            if sub_norm != filter_app {
                continue;
            }
        }
        let sub_app = Some(sub.app_type.as_str());
        let range_api_cost_usd =
            Database::usage_summary_impl(conn, start_date, end_date, sub_app, None, None, false)
                .ok()
                .and_then(|u| u.total_cost.parse::<f64>().ok())
                .unwrap_or(0.0);
        let (cycle_api_cost_usd, cycle_requests) =
            match crate::subscriptions::weekly_cycle(&sub.rule, now_local, 0) {
                Ok((cs, ce)) => Database::usage_summary_impl(
                    conn,
                    Some(cs.timestamp()),
                    Some(ce.timestamp()),
                    sub_app,
                    None,
                    None,
                    false,
                )
                .ok()
                .map(|u| (u.total_cost.parse::<f64>().unwrap_or(0.0), u.total_requests))
                .unwrap_or((0.0, 0)),
                Err(_) => (0.0, 0),
            };
        // Pro-rate subscription price to a 30-day window for USD ROI comparison when billing is monthly/yearly/custom.
        let period_days = match sub.billing.as_str() {
            "年" => 365.0,
            "期" => f64::from(sub.custom_interval_days.unwrap_or(30).max(1)),
            _ => 30.0,
        };
        let usd_roi_multiple = if sub.currency.eq_ignore_ascii_case("USD") && sub.price > 0.0 {
            let range_days = elapsed_days.max(1.0);
            let prorated_price = sub.price * (range_days / period_days);
            (prorated_price > 0.0001).then(|| range_api_cost_usd / prorated_price)
        } else {
            None
        };
        subscriptions.push(SubscriptionRoi {
            id: sub.id.clone(),
            name: sub.name.clone(),
            app_type: sub.app_type.clone(),
            price: sub.price,
            currency: sub.currency.clone(),
            billing: sub.billing.clone(),
            custom_interval_days: sub.custom_interval_days,
            range_api_cost_usd,
            cycle_api_cost_usd,
            cycle_requests,
            usd_roi_multiple,
        });
    }

    Ok(InsightsData {
        cache_savings,
        forecast,
        rhythm,
        sessions,
        projects,
        stability,
        token_mix,
        status,
        subscriptions,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTurnRow {
    pub request_id: String,
    pub created_at: i64,
    pub model: String,
    pub fresh_input: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_cost_usd: f64,
    pub latency_ms: u64,
    pub first_token_ms: Option<u64>,
    pub status_code: i64,
    pub data_source: String,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTurnsDetail {
    pub session_id: String,
    pub app_type: String,
    pub title: Option<String>,
    pub project_dir: Option<String>,
    pub git_branch: Option<String>,
    pub resume_command: Option<String>,
    pub total_turns: u64,
    pub total_cost_usd: f64,
    pub total_tokens: u64,
    pub turns: Vec<SessionTurnRow>,
}

#[derive(Default, Clone)]
struct ResolvedSessionMeta {
    title: Option<String>,
    project_dir: Option<String>,
    git_branch: Option<String>,
    resume_command: Option<String>,
}

fn session_meta_cache() -> &'static Mutex<HashMap<(String, String), ResolvedSessionMeta>> {
    static CACHE: OnceLock<Mutex<HashMap<(String, String), ResolvedSessionMeta>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn batch_lookup_opencode() -> HashMap<String, (Option<String>, Option<String>)> {
    let mut map = HashMap::new();
    let db_path = crate::opencode_config::get_opencode_db_path();
    let Ok(conn) = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return map;
    };
    let Ok(mut stmt) = conn.prepare("SELECT id, title, directory FROM session") else {
        return map;
    };
    let Ok(rows) = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    }) else {
        return map;
    };
    for (id, title, dir) in rows.flatten() {
        map.insert(
            id,
            (
                title.as_deref().and_then(clean_title),
                dir.filter(|s| !s.is_empty()),
            ),
        );
    }
    map
}

fn batch_lookup_mcode() -> HashMap<String, (Option<String>, Option<String>)> {
    let mut map = HashMap::new();
    let Ok(conn) = crate::session_manager::providers::mcode::open_database() else {
        return map;
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT session_id, title, workspace_dir FROM local_runtime_sessions")
    else {
        return map;
    };
    let Ok(rows) = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    }) else {
        return map;
    };
    for (id, title, dir) in rows.flatten() {
        map.insert(
            id,
            (
                title.as_deref().and_then(clean_title),
                dir.filter(|s| !s.is_empty()),
            ),
        );
    }
    map
}

/// Resolve session metadata (`title`, `project_dir`, `git_branch`, `resume_command`) for a
/// batch of `(app_type, session_id)` pairs using an in-memory cache and per-platform batch
/// reads so repeated Insights refreshes do zero disk I/O.
fn resolve_sessions_batch(
    pairs: &[(String, String)],
) -> HashMap<(String, String), ResolvedSessionMeta> {
    let mut out = HashMap::with_capacity(pairs.len());
    let mut misses: HashMap<String, Vec<String>> = HashMap::new();
    if let Ok(cache) = session_meta_cache().lock() {
        for (app, sid) in pairs {
            let key = (app.clone(), sid.clone());
            if let Some(hit) = cache.get(&key) {
                out.insert(key, hit.clone());
            } else {
                misses.entry(app.clone()).or_default().push(sid.clone());
            }
        }
    }
    if misses.is_empty() {
        return out;
    }

    let mut newly_resolved: Vec<((String, String), ResolvedSessionMeta)> = Vec::new();
    for (app, sids) in misses {
        match app.as_str() {
            "agy" => {
                let all = crate::session_manager::providers::agy::lookup_all_summaries();
                for sid in sids {
                    let (title, project_dir) = all.get(&sid).cloned().unwrap_or((None, None));
                    newly_resolved.push((
                        (app.clone(), sid.clone()),
                        ResolvedSessionMeta {
                            title,
                            project_dir,
                            git_branch: None,
                            resume_command: safe_session_token(&sid)
                                .then(|| crate::agy_config::resume_command(None, &sid)),
                        },
                    ));
                }
            }
            "opencode" => {
                let all = batch_lookup_opencode();
                for sid in sids {
                    let (title, project_dir) = all.get(&sid).cloned().unwrap_or((None, None));
                    newly_resolved.push((
                        (app.clone(), sid.clone()),
                        ResolvedSessionMeta {
                            title,
                            project_dir,
                            git_branch: None,
                            resume_command: safe_session_token(&sid)
                                .then(|| format!("opencode -s {sid}")),
                        },
                    ));
                }
            }
            "mcode" => {
                let all = batch_lookup_mcode();
                for sid in sids {
                    let (title, project_dir) = all.get(&sid).cloned().unwrap_or((None, None));
                    newly_resolved.push((
                        (app.clone(), sid.clone()),
                        ResolvedSessionMeta {
                            title,
                            project_dir,
                            git_branch: None,
                            resume_command: safe_session_token(&sid)
                                .then(|| format!("mcode --session {sid}")),
                        },
                    ));
                }
            }
            "gemini" | "pi" | "grokbuild" => {
                let list = match app.as_str() {
                    "gemini" => crate::session_manager::providers::gemini::scan_sessions(),
                    "pi" => crate::session_manager::providers::pi::scan_sessions(),
                    _ => crate::session_manager::providers::grokbuild::scan_sessions(),
                };
                let by_id: HashMap<String, ResolvedSessionMeta> = list
                    .into_iter()
                    .map(|s| {
                        (
                            s.session_id,
                            ResolvedSessionMeta {
                                title: s.title,
                                project_dir: s.project_dir,
                                git_branch: None,
                                resume_command: s.resume_command,
                            },
                        )
                    })
                    .collect();
                for sid in sids {
                    let meta = by_id.get(&sid).cloned().unwrap_or_default();
                    newly_resolved.push(((app.clone(), sid), meta));
                }
            }
            "claude" | "claude-desktop" => {
                for sid in sids {
                    let meta = lookup_claude_meta(&sid);
                    newly_resolved.push(((app.clone(), sid), meta));
                }
            }
            "codex" => {
                for sid in sids {
                    let meta = lookup_codex_meta(&sid);
                    newly_resolved.push(((app.clone(), sid), meta));
                }
            }
            _ => {
                for sid in sids {
                    newly_resolved.push(((app.clone(), sid), ResolvedSessionMeta::default()));
                }
            }
        }
    }

    if let Ok(mut cache) = session_meta_cache().lock() {
        if cache.len() > 2000 {
            cache.clear();
        }
        for (k, v) in &newly_resolved {
            // Only cache resolved entries (or non-empty apps) so brand-new live sessions
            // that haven't flushed a title yet can be picked up once available.
            if v.title.is_some() || v.project_dir.is_some() {
                cache.insert(k.clone(), v.clone());
            }
        }
    }
    for (k, v) in newly_resolved {
        out.insert(k, v);
    }
    out
}

pub(crate) fn clean_title(raw: &str) -> Option<String> {
    let trimmed = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('<'))?;
    let mut out = String::new();
    for (i, ch) in trimmed.chars().enumerate() {
        if i >= 96 {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    (!out.is_empty()).then_some(out)
}

pub(crate) fn safe_session_token(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn extract_claude_text(value: &serde_json::Value) -> Option<String> {
    let content = &value["message"]["content"];
    if let Some(s) = content.as_str() {
        return clean_title(s);
    }
    if let Some(arr) = content.as_array() {
        for item in arr {
            if item["type"] == "text" {
                if let Some(text) = item["text"].as_str().and_then(clean_title) {
                    return Some(text);
                }
            }
        }
    }
    None
}

fn lookup_claude_meta(session_id: &str) -> ResolvedSessionMeta {
    let mut meta = ResolvedSessionMeta {
        resume_command: safe_session_token(session_id)
            .then(|| format!("claude --resume {session_id}")),
        ..Default::default()
    };
    if !safe_session_token(session_id) {
        return meta;
    }
    let projects_dir = crate::config::get_home_dir().join(".claude/projects");
    let Ok(entries) = std::fs::read_dir(projects_dir) else {
        return meta;
    };
    let filename = format!("{session_id}.jsonl");
    let mut target = None;
    for entry in entries.flatten() {
        let candidate = entry.path().join(&filename);
        if candidate.is_file() {
            target = Some(candidate);
            break;
        }
    }
    let Some(path) = target else {
        return meta;
    };
    let Ok(file) = std::fs::File::open(path) else {
        return meta;
    };
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    for _ in 0..80 {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if meta.project_dir.is_none() {
            meta.project_dir = v["cwd"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        }
        if meta.git_branch.is_none() {
            meta.git_branch = v["gitBranch"]
                .as_str()
                .filter(|s| !s.is_empty() && *s != "HEAD")
                .map(str::to_string);
        }
        if v["type"] == "summary" {
            if let Some(summary) = v["summary"].as_str().and_then(clean_title) {
                meta.title = Some(summary);
            }
        } else if meta.title.is_none() && v["type"] == "user" {
            meta.title = extract_claude_text(&v);
        }
        if meta.title.is_some() && meta.project_dir.is_some() && meta.git_branch.is_some() {
            break;
        }
    }
    meta
}

pub(crate) fn find_codex_rollout(
    root: &std::path::Path,
    session_id: &str,
    depth: usize,
) -> Option<std::path::PathBuf> {
    if depth > 5 {
        return None;
    }
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            if let Some(found) = find_codex_rollout(&path, session_id, depth + 1) {
                return Some(found);
            }
        } else if ft.is_file()
            && crate::codex_rollout_file::is_rollout_file(&path)
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(session_id))
        {
            return Some(path);
        }
    }
    None
}

fn lookup_codex_meta(session_id: &str) -> ResolvedSessionMeta {
    let mut meta = ResolvedSessionMeta {
        resume_command: safe_session_token(session_id)
            .then(|| format!("codex resume {session_id}")),
        ..Default::default()
    };
    if !safe_session_token(session_id) {
        return meta;
    }
    let sessions_dir = crate::codex_config::get_codex_config_dir().join("sessions");
    let Some(path) = find_codex_rollout(&sessions_dir, session_id, 0) else {
        return meta;
    };
    let Ok(mut reader) = crate::codex_rollout_file::open_reader(&path) else {
        return meta;
    };
    let mut line = String::new();
    for _ in 0..60 {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v["type"] == "session_meta" {
            let payload = &v["payload"];
            if meta.project_dir.is_none() {
                meta.project_dir = payload["cwd"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
            }
            if meta.git_branch.is_none() {
                meta.git_branch = payload["git"]["branch"]
                    .as_str()
                    .filter(|s| !s.is_empty() && *s != "HEAD")
                    .map(str::to_string);
            }
        } else if meta.title.is_none()
            && v["type"] == "event_msg"
            && v["payload"]["type"] == "user_message"
        {
            meta.title = v["payload"]["message"].as_str().and_then(clean_title);
        } else if meta.title.is_none()
            && v["type"] == "response_item"
            && v["payload"]["role"] == "user"
        {
            if let Some(arr) = v["payload"]["content"].as_array() {
                for item in arr {
                    if let Some(text) = item["text"].as_str().and_then(clean_title) {
                        meta.title = Some(text);
                        break;
                    }
                }
            }
        }
        if meta.title.is_some() && meta.project_dir.is_some() && meta.git_branch.is_some() {
            break;
        }
    }
    meta
}

fn lookup_opencode_meta(session_id: &str) -> ResolvedSessionMeta {
    let mut meta = ResolvedSessionMeta {
        resume_command: safe_session_token(session_id).then(|| format!("opencode -s {session_id}")),
        ..Default::default()
    };
    let db_path = crate::opencode_config::get_opencode_db_path();
    let Ok(conn) = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return meta;
    };
    if let Ok((title, dir)) = conn.query_row(
        "SELECT title, directory FROM session WHERE id = ?1 LIMIT 1",
        [session_id],
        |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
            ))
        },
    ) {
        meta.title = title.as_deref().and_then(clean_title);
        meta.project_dir = dir.filter(|s| !s.is_empty());
    }
    meta
}

fn lookup_mcode_meta(session_id: &str) -> ResolvedSessionMeta {
    let mut meta = ResolvedSessionMeta {
        resume_command: safe_session_token(session_id)
            .then(|| format!("mcode --session {session_id}")),
        ..Default::default()
    };
    let Ok(conn) = crate::session_manager::providers::mcode::open_database() else {
        return meta;
    };
    if let Ok((title, dir)) = conn.query_row(
        "SELECT title, workspace_dir FROM local_runtime_sessions WHERE session_id = ?1 LIMIT 1",
        [session_id],
        |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
            ))
        },
    ) {
        meta.title = title.as_deref().and_then(clean_title);
        meta.project_dir = dir.filter(|s| !s.is_empty());
    }
    meta
}

fn lookup_session_meta(app_type: &str, session_id: &str) -> ResolvedSessionMeta {
    match app_type {
        "agy" => {
            let (title, project_dir) =
                crate::session_manager::providers::agy::lookup_session_summary(session_id)
                    .unwrap_or((None, None));
            ResolvedSessionMeta {
                title,
                project_dir,
                git_branch: None,
                resume_command: safe_session_token(session_id)
                    .then(|| crate::agy_config::resume_command(None, session_id)),
            }
        }
        "claude" | "claude-desktop" => lookup_claude_meta(session_id),
        "codex" => lookup_codex_meta(session_id),
        "opencode" => lookup_opencode_meta(session_id),
        "mcode" => lookup_mcode_meta(session_id),
        "gemini" => crate::session_manager::providers::gemini::scan_sessions()
            .into_iter()
            .find(|s| s.session_id == session_id)
            .map(|s| ResolvedSessionMeta {
                title: s.title,
                project_dir: s.project_dir,
                git_branch: None,
                resume_command: s.resume_command,
            })
            .unwrap_or_default(),
        "pi" => crate::session_manager::providers::pi::scan_sessions()
            .into_iter()
            .find(|s| s.session_id == session_id)
            .map(|s| ResolvedSessionMeta {
                title: s.title,
                project_dir: s.project_dir,
                git_branch: None,
                resume_command: s.resume_command,
            })
            .unwrap_or_default(),
        "grokbuild" => crate::session_manager::providers::grokbuild::scan_sessions()
            .into_iter()
            .find(|s| s.session_id == session_id)
            .map(|s| ResolvedSessionMeta {
                title: s.title,
                project_dir: s.project_dir,
                git_branch: None,
                resume_command: s.resume_command,
            })
            .unwrap_or_default(),
        _ => ResolvedSessionMeta::default(),
    }
}

pub fn query_session_turns_on_conn(
    conn: &Connection,
    session_id: &str,
    app_type: Option<&str>,
    start_date: Option<i64>,
    end_date: Option<i64>,
) -> Result<SessionTurnsDetail, AppError> {
    if session_id.trim().is_empty() {
        return Err(AppError::InvalidInput("sessionId is required".into()));
    }
    let fresh_sql = fresh_input_sql("l");
    let mut conds = vec!["l.session_id = ?".to_string()];
    let mut params: Vec<SqlValue> = vec![SqlValue::Text(session_id.to_string())];
    if let Some(s) = start_date {
        conds.push("l.created_at >= ?".to_string());
        params.push(SqlValue::Integer(s));
    }
    if let Some(e) = end_date {
        conds.push("l.created_at < ?".to_string());
        params.push(SqlValue::Integer(e));
    }
    if let Some(app) = app_type.filter(|a| !a.is_empty()) {
        if app == "claude" {
            conds.push("l.app_type IN ('claude', 'claude-desktop')".to_string());
        } else {
            conds.push("l.app_type = ?".to_string());
            params.push(SqlValue::Text(app.to_string()));
        }
    }
    let dedup_sql = crate::services::usage_stats::effective_usage_log_filter_for_range(
        conn, "l", start_date, end_date,
    )?;
    if dedup_sql != "1 = 1" {
        conds.push(dedup_sql);
    }
    let where_sql = format!("WHERE {}", conds.join(" AND "));
    let sql = format!(
        "SELECT
            l.request_id,
            l.created_at,
            l.model,
            ({fresh_sql}) AS fresh_input,
            l.output_tokens,
            l.cache_read_tokens,
            l.cache_creation_tokens,
            CAST(l.total_cost_usd AS REAL),
            l.latency_ms,
            l.first_token_ms,
            l.status_code,
            COALESCE(l.data_source, 'proxy'),
            l.error_message,
            CASE WHEN l.app_type = 'claude-desktop' THEN 'claude' ELSE l.app_type END AS app_norm
         FROM proxy_request_logs l
         {where_sql}
         ORDER BY l.created_at ASC, l.rowid ASC
         LIMIT 200"
    );
    let mut turns = Vec::new();
    let mut detected_app = app_type.unwrap_or("").to_string();
    let mut total_cost_usd = 0.0f64;
    let mut total_tokens = 0u64;
    {
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(params.iter()))?;
        while let Some(row) = rows.next()? {
            let fresh_input = row.get::<_, i64>(3)?.max(0) as u64;
            let output_tokens = row.get::<_, i64>(4)?.max(0) as u64;
            let cache_read_tokens = row.get::<_, i64>(5)?.max(0) as u64;
            let cache_creation_tokens = row.get::<_, i64>(6)?.max(0) as u64;
            let cost: f64 = row.get(7)?;
            let app_norm: String = row.get(13)?;
            if detected_app.is_empty() {
                detected_app = app_norm;
            }
            total_cost_usd += cost;
            total_tokens += fresh_input + output_tokens + cache_read_tokens + cache_creation_tokens;
            turns.push(SessionTurnRow {
                request_id: row.get(0)?,
                created_at: row.get(1)?,
                model: row.get(2)?,
                fresh_input,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                total_cost_usd: cost,
                latency_ms: row.get::<_, i64>(8)?.max(0) as u64,
                first_token_ms: row
                    .get::<_, Option<i64>>(9)?
                    .filter(|v| *v >= 0)
                    .map(|v| v as u64),
                status_code: row.get(10)?,
                data_source: row.get(11)?,
                error_message: row.get(12)?,
            });
        }
    }
    let meta = lookup_session_meta(&detected_app, session_id);
    Ok(SessionTurnsDetail {
        session_id: session_id.to_string(),
        app_type: detected_app,
        title: meta.title,
        project_dir: meta.project_dir,
        git_branch: meta.git_branch,
        resume_command: meta.resume_command,
        total_turns: turns.len() as u64,
        total_cost_usd,
        total_tokens,
        turns,
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
                query_insights_on_conn(conn, Some(now - 3600), Some(now + 60), Some("agy"), &[])
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

        let detail = db
            .with_read_connection(|conn| {
                query_session_turns_on_conn(
                    conn,
                    "session-a",
                    Some("agy"),
                    Some(now - 3600),
                    Some(now + 60),
                )
            })
            .unwrap();
        assert_eq!(detail.total_turns, 3);
        assert_eq!(detail.turns.len(), 3);
        assert_eq!(detail.turns[0].output_tokens, 400);
        assert_eq!(detail.turns[2].status_code, 429);
    }

    #[test]
    fn previous_period_covers_only_the_elapsed_length() {
        let db = Database::memory().unwrap();
        let today = Local::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|dt| Local.from_local_datetime(&dt).earliest())
            .unwrap()
            .timestamp();
        let yesterday = Local
            .timestamp_opt(today, 0)
            .single()
            .and_then(|dt| dt.date_naive().checked_sub_days(chrono::Days::new(1)))
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .and_then(|dt| Local.from_local_datetime(&dt).earliest())
            .unwrap()
            .timestamp();
        {
            let conn = db.conn.lock().unwrap();
            // Yesterday 01:00 falls inside the matching two hours; 05:00 does not.
            for (id, at, cost) in [
                ("prev-in", yesterday + 3_600, "1"),
                ("prev-out", yesterday + 5 * 3_600, "4"),
                ("now", today + 3_600, "2"),
            ] {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model, request_model,
                        input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                        input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
                        latency_ms, status_code, session_id, is_streaming, cost_multiplier,
                        created_at, data_source, input_token_semantics
                    ) VALUES (?1, 'p1', 'agy', 'm', 'm', 10, 10, 0, 0,
                        '0', '0', '0', '0', ?3, 0, 200, 's', 1, '1', ?2, 'agy_session', ?4)",
                    rusqlite::params![id, at, cost, INPUT_TOKEN_SEMANTICS_FRESH],
                )
                .unwrap();
            }
        }
        let data = db
            .with_read_connection(|conn| {
                query_insights_on_conn(conn, Some(today), Some(today + 7_200), None, &[])
            })
            .unwrap();
        assert!((data.forecast.current_cost_usd - 2.0).abs() < 1e-9);
        assert!((data.forecast.previous_cost_usd - 1.0).abs() < 1e-9);
    }
}
