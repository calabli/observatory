//! Multi-market real-time stock quotes, charts, and symbol search (zero API key required).
//!
//! Routes each market to the highest-freshness free public endpoint:
//! - A-shares (`*.SS`, `*.SZ`, `*.BJ`) & Hong Kong (`*.HK`, `^HSI`):
//!   Tencent Finance UTF-8 real-time + 1m intraday JSON (`web.ifzq.gtimg.cn`), fallback Yahoo Finance.
//! - Taiwan (`*.TW`, `*.TWO`, `^TWII`):
//!   Official TWSE/TPEx MIS real-time API (`mis.twse.com.tw`), merged with Yahoo Finance intraday sparklines.
//! - US & Global (`NVDA`, `AAPL`, `^GSPC`, `^IXIC`, `^DJI`, `^N225`, `BTC-USD`, etc.):
//!   Yahoo Finance v8 Chart API (real-time + pre/post market + intraday sparkline), fallback Tencent US.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{Local, NaiveDate, NaiveTime, TimeZone};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::database::Database;
use crate::error::AppError;

const WATCHLIST_SETTINGS_KEY: &str = "markets_watchlist_v1";
const EXTREMES_CACHE_KEY: &str = "markets_ath_cache_v1";
const EXTREMES_TTL_SECS: i64 = 7 * 86_400;
const FINANCIALS_CACHE_KEY: &str = "markets_financials_cache_v3";
const FINANCIALS_TTL_SECS: i64 = 24 * 3600;
/// Shorter TTL when only Yahoo fallback succeeded (primary deep-history source failed transiently).
const FINANCIALS_FALLBACK_TTL_SECS: i64 = 10 * 60;
const FINANCIALS_MAX_CACHE_ENTRIES: usize = 80;
const FINANCIALS_MAX_ANNUAL: usize = 20;
const FINANCIALS_MAX_QUARTERLY: usize = 40;
/// SEC EDGAR Fair Access policy requires a contact email address in User-Agent (otherwise Akamai returns HTTP 403).
const SEC_USER_AGENT: &str = "ObservatoryDesktop/0.1 (observatory@local.dev)";
/// Full-history requests are heavy and easy to get throttled for; each call fetches
/// at most this many symbols, two at a time.
const EXTREMES_MAX_PER_CALL: usize = 10;
const EXTREMES_CONCURRENCY: usize = 2;
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(8))
            .connect_timeout(Duration::from_secs(4))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Tencent's firewall sometimes blocks a client for a while (it answers with an HTTP
/// error or an HTML challenge page). After a failed request we stop calling that
/// Tencent service for a growing pause (1, 2, 4 ... up to 15 minutes) instead of
/// tripping the block on every poll; CN / HK quotes take the Yahoo route meanwhile.
///
/// The batch quote service (`qt.gtimg.cn`) and the per-stock intraday service
/// (`web.ifzq.gtimg.cn`) are blocked independently, so each has its own pause: one
/// shared pause let a blocked intraday service switch off the working batch service
/// too, which removed market cap and traded value from every quote.
struct Backoff {
    until: Option<std::time::Instant>,
    failures: u32,
}

#[derive(Clone, Copy, Debug)]
enum TencentService {
    /// `qt.gtimg.cn` batch quotes (price, market cap, traded value).
    Batch,
    /// `web.ifzq.gtimg.cn` per-stock quote with intraday points.
    Minute,
}

static TENCENT_BATCH_BACKOFF: std::sync::Mutex<Backoff> = std::sync::Mutex::new(Backoff {
    until: None,
    failures: 0,
});
static TENCENT_MINUTE_BACKOFF: std::sync::Mutex<Backoff> = std::sync::Mutex::new(Backoff {
    until: None,
    failures: 0,
});

fn backoff_of(service: TencentService) -> &'static std::sync::Mutex<Backoff> {
    match service {
        TencentService::Batch => &TENCENT_BATCH_BACKOFF,
        TencentService::Minute => &TENCENT_MINUTE_BACKOFF,
    }
}

fn backoff_delay(failures: u32) -> Duration {
    let secs = 60u64.saturating_mul(1 << failures.saturating_sub(1).min(4));
    Duration::from_secs(secs.min(15 * 60))
}

fn tencent_available(service: TencentService) -> bool {
    backoff_of(service)
        .lock()
        .map(|b| {
            b.until
                .is_none_or(|until| std::time::Instant::now() >= until)
        })
        .unwrap_or(true)
}

fn tencent_report(service: TencentService, ok: bool) {
    if let Ok(mut b) = backoff_of(service).lock() {
        if ok {
            b.failures = 0;
            b.until = None;
        } else {
            b.failures = b.failures.saturating_add(1);
            b.until = Some(std::time::Instant::now() + backoff_delay(b.failures));
            log::warn!(
                "腾讯行情 {service:?} 请求失败，暂停 {} 秒后再试",
                backoff_delay(b.failures).as_secs()
            );
        }
    }
}

/// A Tencent response that is an HTML page (the firewall's challenge) instead of quotes.
fn looks_blocked(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|b| *b == b'<')
}

async fn traced_get_bytes(
    provider: &str,
    url: &str,
    referer: Option<&str>,
) -> Result<Vec<u8>, String> {
    traced_get_bytes_opts(provider, url, referer, None, None).await
}

async fn traced_get_bytes_opts(
    provider: &str,
    url: &str,
    referer: Option<&str>,
    user_agent: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Vec<u8>, String> {
    let started = std::time::Instant::now();
    let mut req = http_client().get(url);
    if let Some(r) = referer {
        req = req.header("Referer", r);
    }
    if let Some(ua) = user_agent {
        req = req.header("User-Agent", ua);
    }
    if let Some(secs) = timeout_secs {
        req = req.timeout(Duration::from_secs(secs));
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("{provider}请求失败: {e}");
            crate::activity::record_http_trace(crate::activity::OutboundHttpTrace {
                provider: provider.to_string(),
                method: "GET".into(),
                url: url.to_string(),
                http_status: None,
                duration_ms: started.elapsed().as_millis() as i64,
                response_bytes: 0,
                error: Some(msg.clone()),
            });
            return Err(msg);
        }
    };
    let status = resp.status();
    let status_code = status.as_u16();
    if !status.is_success() {
        let msg = format!("{provider} HTTP {status}");
        crate::activity::record_http_trace(crate::activity::OutboundHttpTrace {
            provider: provider.to_string(),
            method: "GET".into(),
            url: url.to_string(),
            http_status: Some(status_code),
            duration_ms: started.elapsed().as_millis() as i64,
            response_bytes: 0,
            error: Some(msg.clone()),
        });
        return Err(msg);
    }
    match resp.bytes().await {
        Ok(bytes) => {
            let vec = bytes.to_vec();
            crate::activity::record_http_trace(crate::activity::OutboundHttpTrace {
                provider: provider.to_string(),
                method: "GET".into(),
                url: url.to_string(),
                http_status: Some(status_code),
                duration_ms: started.elapsed().as_millis() as i64,
                response_bytes: vec.len(),
                error: None,
            });
            Ok(vec)
        }
        Err(e) => {
            let msg = format!("{provider}读取响应失败: {e}");
            crate::activity::record_http_trace(crate::activity::OutboundHttpTrace {
                provider: provider.to_string(),
                method: "GET".into(),
                url: url.to_string(),
                http_status: Some(status_code),
                duration_ms: started.elapsed().as_millis() as i64,
                response_bytes: 0,
                error: Some(msg.clone()),
            });
            Err(msg)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchItem {
    pub symbol: String,
    pub name: String,
    /// `"US"` | `"TW"` | `"CN"` | `"HK"` | `"GLOBAL"`
    pub market: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap_hint: Option<f64>,
}

impl WatchItem {
    fn simple(symbol: &str, name: &str, market: &str) -> Self {
        Self {
            symbol: symbol.into(),
            name: name.into(),
            market: market.into(),
            sector: None,
            cap_hint: None,
        }
    }
}

/// Delta edits applied on top of a built-in preset pool so future universe refreshes remain intact.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolEdit {
    #[serde(default)]
    pub added: Vec<WatchItem>,
    #[serde(default)]
    pub removed: Vec<String>,
}

/// A user-created custom stock pool with sector-tagged constituents.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomPool {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub items: Vec<WatchItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketsConfig {
    pub indices: Vec<WatchItem>,
    pub watchlist: Vec<WatchItem>,
    /// `"cn"` (红涨绿跌) | `"us"` (绿涨红跌)
    pub color_scheme: String,
    /// `5` | `15` | `30` | `0` (manual)
    pub refresh_sec: u64,
    #[serde(default)]
    pub pool_edits: HashMap<String, PoolEdit>,
    #[serde(default)]
    pub custom_pools: Vec<CustomPool>,
}

impl Default for MarketsConfig {
    fn default() -> Self {
        Self {
            indices: vec![
                WatchItem::simple("^GSPC", "标普500", "US"),
                WatchItem::simple("^IXIC", "纳斯达克", "US"),
                WatchItem::simple("^DJI", "道琼斯", "US"),
                WatchItem::simple("000001.SS", "上证指数", "CN"),
                WatchItem::simple("000300.SS", "沪深300", "CN"),
                WatchItem::simple("^HSI", "恒生指数", "HK"),
                WatchItem::simple("^TWII", "台湾加权", "TW"),
            ],
            watchlist: vec![
                WatchItem::simple("NVDA", "英伟达", "US"),
                WatchItem::simple("AAPL", "苹果", "US"),
                WatchItem::simple("MSFT", "微软", "US"),
                WatchItem::simple("TSLA", "特斯拉", "US"),
                WatchItem::simple("GOOGL", "谷歌", "US"),
                WatchItem::simple("AMZN", "亚马逊", "US"),
                WatchItem::simple("2330.TW", "台积电", "TW"),
                WatchItem::simple("2317.TW", "鸿海", "TW"),
                WatchItem::simple("2454.TW", "联发科", "TW"),
                WatchItem::simple("600519.SS", "贵州茅台", "CN"),
                WatchItem::simple("300750.SZ", "宁德时代", "CN"),
                WatchItem::simple("002594.SZ", "比亚迪", "CN"),
                WatchItem::simple("0700.HK", "腾讯控股", "HK"),
                WatchItem::simple("9988.HK", "阿里巴巴", "HK"),
                WatchItem::simple("3690.HK", "美团", "HK"),
            ],
            color_scheme: "cn".into(),
            refresh_sec: 15,
            pool_edits: HashMap::new(),
            custom_pools: Vec::new(),
        }
    }
}

pub fn load_config(db: &Database) -> Result<MarketsConfig, AppError> {
    if let Some(raw) = db.get_setting(WATCHLIST_SETTINGS_KEY)? {
        if let Ok(cfg) = serde_json::from_str::<MarketsConfig>(&raw) {
            return Ok(cfg);
        }
    }
    Ok(MarketsConfig::default())
}

pub fn save_config(db: &Database, cfg: &MarketsConfig) -> Result<(), AppError> {
    let raw = serde_json::to_string(cfg).map_err(|e| AppError::InvalidInput(e.to_string()))?;
    // UI preference, not usage data: a quiet write keeps the usage caches and the
    // LAN export fingerprint from changing on every watchlist edit.
    db.set_setting_quietly(WATCHLIST_SETTINGS_KEY, &raw)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartPoint {
    pub ts: i64,
    pub price: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PriceExtremes {
    pub ath: f64,
    pub ath_ts: Option<i64>,
    pub atl: f64,
    pub atl_ts: Option<i64>,
    pub fetched_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtremesBatchResponse {
    pub items: HashMap<String, PriceExtremes>,
    pub remaining: usize,
    /// Symbols this call tried to fetch and how many of them succeeded; a round with
    /// attempts but no successes means the sources are refusing, so callers stop.
    pub attempted: usize,
    pub fetched: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockQuote {
    pub symbol: String,
    pub name: String,
    pub market: String,
    pub currency: String,
    pub exchange: String,
    pub price: Option<f64>,
    pub prev_close: Option<f64>,
    pub change: Option<f64>,
    pub change_percent: Option<f64>,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub volume: Option<u64>,
    /// Total market capitalization in local currency units (e.g. USD / CNY / HKD).
    pub market_cap: Option<f64>,
    /// Exact traded value today in local currency units, when the feed reports it
    /// (Tencent: A-shares, Hong Kong, US). `None` means the UI may estimate it.
    pub turnover: Option<f64>,
    pub extended_price: Option<f64>,
    pub extended_change_percent: Option<f64>,
    pub extended_label: Option<String>,
    pub market_state: String,
    pub source: String,
    pub quote_time: Option<i64>,
    pub sparkline: Vec<ChartPoint>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockChartDetail {
    pub symbol: String,
    pub range: String,
    pub currency: String,
    pub prev_close: Option<f64>,
    pub source: String,
    pub points: Vec<ChartPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fifty_two_week_high: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fifty_two_week_low: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub symbol: String,
    pub name: String,
    pub market: String,
    pub exchange: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FinancialPeriod {
    /// Period end date (`YYYY-MM-DD`).
    pub as_of_date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revenue: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gross_profit: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operating_income: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_income: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diluted_eps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operating_cash_flow: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_cash_flow: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StockFinancials {
    pub symbol: String,
    /// Reporting currency for financial statement rows (may differ from trading currency, e.g. CNY for 0700.HK).
    pub currency: String,
    /// Currency of `latest_market_cap` / `peak_market_cap` snapshots (matches stock trading currency).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_cap_currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_market_cap: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_market_cap_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_market_cap: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_market_cap_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trailing_pe: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trailing_ps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trailing_pb: Option<f64>,
    pub annual: Vec<FinancialPeriod>,
    pub quarterly: Vec<FinancialPeriod>,
    pub source: String,
    pub fetched_at: i64,
}

// ---------------------------------------------------------------------------
// Market & symbol classification helpers
// ---------------------------------------------------------------------------

pub fn detect_market(symbol: &str) -> &'static str {
    let upper = symbol.trim().to_ascii_uppercase();
    if upper.ends_with(".SS")
        || upper.ends_with(".SZ")
        || upper.ends_with(".BJ")
        || upper.starts_with("SH")
            && upper.len() == 8
            && upper[2..].chars().all(|c| c.is_ascii_digit())
        || upper.starts_with("SZ")
            && upper.len() == 8
            && upper[2..].chars().all(|c| c.is_ascii_digit())
    {
        "CN"
    } else if upper.ends_with(".TW") || upper.ends_with(".TWO") || upper == "^TWII" {
        "TW"
    } else if upper.ends_with(".HK") || upper == "^HSI" || upper == "^HSTECH" {
        "HK"
    } else if upper.ends_with(".T")
        || upper.ends_with(".L")
        || upper.ends_with(".DE")
        || upper.ends_with(".PA")
        || upper.ends_with("-USD")
        || upper.ends_with("=F")
        || upper.ends_with("=X")
    {
        "GLOBAL"
    } else {
        "US"
    }
}

fn default_currency(market: &str) -> &'static str {
    match market {
        "CN" => "CNY",
        "TW" => "TWD",
        "HK" => "HKD",
        _ => "USD",
    }
}

/// Convert canonical symbol (`600519.SS`, `300750.SZ`, `0700.HK`, `^HSI`) to Tencent code.
fn to_tencent_code(symbol: &str) -> Option<String> {
    let upper = symbol.trim().to_ascii_uppercase();
    if upper == "^HSI" {
        return Some("hkHSI".into());
    }
    if upper == "^HSTECH" {
        return Some("hkHSTECH".into());
    }
    if let Some(code) = upper.strip_suffix(".SS") {
        return Some(format!("sh{code}"));
    }
    if let Some(code) = upper.strip_suffix(".SZ") {
        return Some(format!("sz{code}"));
    }
    if let Some(code) = upper.strip_suffix(".BJ") {
        return Some(format!("bj{code}"));
    }
    if let Some(code) = upper.strip_suffix(".HK") {
        if code.chars().all(|c| c.is_ascii_digit()) && !code.is_empty() {
            return Some(format!("hk{:0>5}", code));
        }
    }
    if (upper.starts_with("SH") || upper.starts_with("SZ") || upper.starts_with("BJ"))
        && upper.len() == 8
        && upper[2..].chars().all(|c| c.is_ascii_digit())
    {
        return Some(upper.to_ascii_lowercase());
    }
    None
}

/// Convert canonical symbol (CN / HK / US) to Tencent `qt.gtimg.cn` batch code.
fn to_tencent_batch_code(symbol: &str) -> Option<String> {
    if let Some(code) = to_tencent_code(symbol) {
        return Some(code);
    }
    let upper = symbol.trim().to_ascii_uppercase();
    if upper.is_empty() || upper.starts_with('^') || upper.contains('=') || upper.ends_with("-USD")
    {
        return None;
    }
    if detect_market(&upper) == "US"
        && upper
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        // Tencent US quotes use dots for share classes (e.g. BRK-B -> usBRK.B).
        return Some(format!("us{}", upper.replace('-', ".")));
    }
    None
}

/// Convert canonical Taiwan symbol (`2330.TW`, `6488.TWO`, `^TWII`) to TWSE MIS channel key.
fn to_twse_channel(symbol: &str) -> Option<String> {
    let upper = symbol.trim().to_ascii_uppercase();
    if upper == "^TWII" {
        return Some("tse_t00.tw".into());
    }
    if let Some(code) = upper.strip_suffix(".TW") {
        return Some(format!("tse_{code}.tw"));
    }
    if let Some(code) = upper.strip_suffix(".TWO") {
        return Some(format!("otc_{code}.tw"));
    }
    None
}

fn downsample_points(points: Vec<ChartPoint>, max_len: usize) -> Vec<ChartPoint> {
    if points.len() <= max_len || max_len < 2 {
        return points;
    }
    let last = points.len() - 1;
    let mut out = Vec::with_capacity(max_len);
    for i in 0..max_len {
        let idx = (i * last) / (max_len - 1);
        out.push(points[idx].clone());
    }
    out
}

fn parse_f64_opt(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() || t == "-" || t == "--" {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

// ---------------------------------------------------------------------------
// 1. Tencent UTF-8 real-time + 1m intraday fetcher (A-shares & HK, ~1-3s lag)
// ---------------------------------------------------------------------------

fn parse_tencent_timestamp_with_offset(raw: &str, offset_hours: i32) -> Option<i64> {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 14 {
        return None;
    }
    let y: i32 = digits[0..4].parse().ok()?;
    let mo: u32 = digits[4..6].parse().ok()?;
    let d: u32 = digits[6..8].parse().ok()?;
    let h: u32 = digits[8..10].parse().ok()?;
    let mi: u32 = digits[10..12].parse().ok()?;
    let s: u32 = digits[12..14].parse().ok()?;
    let naive = NaiveDate::from_ymd_opt(y, mo, d)?.and_hms_opt(h, mi, s)?;
    let offset = chrono::FixedOffset::east_opt(offset_hours * 3600)?;
    offset
        .from_local_datetime(&naive)
        .single()
        .map(|dt| dt.timestamp())
}

fn parse_tencent_timestamp(raw: &str) -> Option<i64> {
    parse_tencent_timestamp_with_offset(raw, 8)
}

/// Split a GBK-encoded byte slice on ASCII `~` without splitting inside a double-byte GBK char
/// (GBK trailing bytes span `0x40..=0x7E`, which includes `0x7E` = `~`).
fn split_gbk_tilde(raw: &[u8]) -> Vec<&[u8]> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < raw.len() {
        let b = raw[i];
        if b >= 0x81 {
            // Skip both bytes of the GBK double-byte character.
            i = (i + 2).min(raw.len());
        } else if b == b'~' {
            parts.push(&raw[start..i]);
            i += 1;
            start = i;
        } else {
            i += 1;
        }
    }
    parts.push(&raw[start..]);
    parts
}

/// Today's traded value from a Tencent quote, in local currency units.
/// A-shares put the exact yuan amount third in field 35 (`price/volume/amount`) and
/// the same amount in 10,000-yuan units in field 37; Hong Kong and US quotes carry
/// the amount in field 37 directly.
fn tencent_turnover(field35: &str, field37: &str, market: &str) -> Option<f64> {
    let positive = |v: f64| (v > 0.0).then_some(v);
    if market == "CN" {
        field35
            .split('/')
            .nth(2)
            .and_then(parse_f64_opt)
            .and_then(positive)
            .or_else(|| {
                parse_f64_opt(field37)
                    .and_then(positive)
                    .map(|wan| wan * 1e4)
            })
    } else {
        parse_f64_opt(field37).and_then(positive)
    }
}

fn parse_tencent_batch_line(
    line: &[u8],
    code_to_item: &HashMap<String, WatchItem>,
) -> Option<StockQuote> {
    let eq_pos = line.iter().position(|&b| b == b'=')?;
    let header = std::str::from_utf8(&line[..eq_pos]).ok()?.trim();
    let code = header.strip_prefix("v_")?;
    let item = code_to_item.get(code)?;

    let rest = &line[eq_pos + 1..];
    let q_start = rest.iter().position(|&b| b == b'"')? + 1;
    let payload_after_open = &rest[q_start..];
    let q_end = payload_after_open.iter().rposition(|&b| b == b'"')?;
    let payload = &payload_after_open[..q_end];
    if payload.is_empty() {
        return None;
    }

    let fields = split_gbk_tilde(payload);
    let get_ascii = |idx: usize| -> &str {
        fields
            .get(idx)
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or("")
            .trim()
    };

    let market = if item.market.is_empty() {
        detect_market(&item.symbol).to_string()
    } else {
        item.market.clone()
    };

    let price = parse_f64_opt(get_ascii(3)).filter(|p| *p > 0.0)?;
    let prev_close = parse_f64_opt(get_ascii(4)).filter(|p| *p > 0.0);
    let open = parse_f64_opt(get_ascii(5)).filter(|p| *p > 0.0);
    let volume = parse_f64_opt(get_ascii(6)).map(|v| {
        if market == "CN" {
            (v * 100.0).max(0.0) as u64
        } else {
            v.max(0.0) as u64
        }
    });
    let tz_offset = if market == "US" { -5 } else { 8 };
    let quote_time = parse_tencent_timestamp_with_offset(get_ascii(30), tz_offset);
    let change = parse_f64_opt(get_ascii(31)).or_else(|| prev_close.map(|pc| price - pc));
    let change_percent = parse_f64_opt(get_ascii(32)).or_else(|| match (change, prev_close) {
        (Some(c), Some(pc)) if pc > 0.0 => Some((c / pc) * 100.0),
        _ => None,
    });
    let high = parse_f64_opt(get_ascii(33)).filter(|p| *p > 0.0);
    let low = parse_f64_opt(get_ascii(34)).filter(|p| *p > 0.0);
    // Field 45 is total market capitalization in 100M (亿) local currency units.
    let market_cap = parse_f64_opt(get_ascii(45))
        .filter(|v| *v > 0.0)
        .map(|yi| yi * 1e8);
    let turnover = tencent_turnover(get_ascii(35), get_ascii(37), &market);

    let exchange = if code.starts_with("sh") {
        "SSE"
    } else if code.starts_with("sz") {
        "SZSE"
    } else if code.starts_with("bj") {
        "BSE"
    } else if code.starts_with("hk") {
        "HKEX"
    } else {
        "US"
    }
    .to_string();

    let name = if !item.name.is_empty() {
        item.name.clone()
    } else {
        let english_name = get_ascii(46);
        if !english_name.is_empty() && market == "US" {
            english_name.to_string()
        } else {
            item.symbol.clone()
        }
    };

    Some(StockQuote {
        symbol: item.symbol.clone(),
        name,
        market: market.clone(),
        currency: default_currency(&market).to_string(),
        exchange,
        price: Some(price),
        prev_close,
        change,
        change_percent,
        open,
        high,
        low,
        volume,
        market_cap,
        turnover,
        extended_price: None,
        extended_change_percent: None,
        extended_label: None,
        market_state: "实时批量".into(),
        source: "腾讯批量实时".into(),
        quote_time,
        sparkline: Vec::new(),
        error: None,
    })
}

/// Tencent's firewall answers bursts of batch requests with HTTP 501 (a 500-stock
/// pool is ~10 requests per refresh), so only a couple run at a time.
const TENCENT_BATCH_CONCURRENCY: usize = 2;
const TENCENT_CHUNK_ATTEMPTS: usize = 2;
const TENCENT_RETRY_PAUSE: Duration = Duration::from_millis(800);

/// Batch-fetch up to hundreds of CN / HK / US real-time quotes + market caps via `qt.gtimg.cn`.
async fn fetch_tencent_batch(items: &[WatchItem]) -> HashMap<String, StockQuote> {
    let mut code_to_item: HashMap<String, WatchItem> = HashMap::new();
    // Other spellings of an already-queued code (`700.HK` next to `0700.HK`).
    let mut aliases: Vec<(String, WatchItem)> = Vec::new();
    let mut codes: Vec<String> = Vec::new();
    for item in items {
        let Some(code) = to_tencent_batch_code(&item.symbol) else {
            continue;
        };
        match code_to_item.get(&code) {
            None => {
                codes.push(code.clone());
                code_to_item.insert(code, item.clone());
            }
            Some(first) if first.symbol != item.symbol => aliases.push((code, item.clone())),
            Some(_) => {}
        }
    }
    if codes.is_empty() || !tencent_available(TencentService::Batch) {
        return HashMap::new();
    }

    let chunks: Vec<Vec<String>> = codes.chunks(55).map(|c| c.to_vec()).collect();
    let map_ref = &code_to_item;
    let results: Vec<Option<Vec<StockQuote>>> = stream::iter(chunks)
        .map(|chunk| async move {
            let url = format!("https://qt.gtimg.cn/q={}", chunk.join(","));
            // A refused chunk is retried once after a pause; dropping it would
            // blank a whole sector, since pools list stocks sector by sector.
            let mut bytes = None;
            for attempt in 0..TENCENT_CHUNK_ATTEMPTS {
                if attempt > 0 {
                    tokio::time::sleep(TENCENT_RETRY_PAUSE).await;
                }
                match traced_get_bytes("腾讯批量实时", &url, None).await {
                    Ok(b) if !looks_blocked(&b) => {
                        bytes = Some(b);
                        break;
                    }
                    _ => {}
                }
            }
            let bytes = bytes?;
            let mut quotes = Vec::new();
            for line in bytes.split(|&b| b == b'\n') {
                if line.is_empty() {
                    continue;
                }
                if let Some(q) = parse_tencent_batch_line(line, map_ref) {
                    quotes.push(q);
                }
            }
            Some(quotes)
        })
        .buffer_unordered(TENCENT_BATCH_CONCURRENCY)
        .collect()
        .await;
    if results.iter().any(Option::is_some) {
        tencent_report(TencentService::Batch, true);
    } else if !results.is_empty() {
        tencent_report(TencentService::Batch, false);
    }

    let mut out = HashMap::new();
    for batch in results.into_iter().flatten() {
        for q in batch {
            out.insert(q.symbol.clone(), q);
        }
    }
    for (code, alias) in aliases {
        let Some(mut q) = code_to_item
            .get(&code)
            .and_then(|first| out.get(&first.symbol))
            .cloned()
        else {
            continue;
        };
        q.symbol = alias.symbol.clone();
        if !alias.name.is_empty() {
            q.name = alias.name.clone();
        }
        out.insert(alias.symbol, q);
    }
    out
}

fn parse_tencent_minute_points(date_str: &str, arr: &[Value]) -> Vec<ChartPoint> {
    let base_date = if date_str.len() == 8 {
        let y = date_str[0..4].parse::<i32>().ok();
        let m = date_str[4..6].parse::<u32>().ok();
        let d = date_str[6..8].parse::<u32>().ok();
        match (y, m, d) {
            (Some(y), Some(m), Some(d)) => NaiveDate::from_ymd_opt(y, m, d),
            _ => None,
        }
    } else {
        None
    }
    .unwrap_or_else(|| Local::now().date_naive());
    let offset = chrono::FixedOffset::east_opt(8 * 3600);

    let mut pts = Vec::with_capacity(arr.len());
    let mut prev_cum_vol: Option<u64> = None;
    for item in arr {
        let Some(line) = item.as_str() else { continue };
        let mut parts = line.split_whitespace();
        let Some(hhmm) = parts.next() else { continue };
        let Some(price) = parts.next().and_then(parse_f64_opt) else {
            continue;
        };
        let cum_vol = parts
            .next()
            .and_then(parse_f64_opt)
            .map(|v| v.max(0.0) as u64);
        let delta_vol = match (cum_vol, prev_cum_vol) {
            (Some(cur), Some(prev)) => Some(cur.saturating_sub(prev)),
            (Some(cur), None) => Some(cur),
            _ => None,
        };
        if cum_vol.is_some() {
            prev_cum_vol = cum_vol;
        }
        if hhmm.len() != 4 || price <= 0.0 {
            continue;
        }
        let Ok(h) = hhmm[0..2].parse::<u32>() else {
            continue;
        };
        let Ok(m) = hhmm[2..4].parse::<u32>() else {
            continue;
        };
        let Some(t) = NaiveTime::from_hms_opt(h, m, 0) else {
            continue;
        };
        let dt = base_date.and_time(t);
        let ts = offset
            .and_then(|o| o.from_local_datetime(&dt).single())
            .map(|d| d.timestamp())
            .unwrap_or(0);
        pts.push(ChartPoint {
            ts,
            price,
            open: None,
            high: None,
            low: None,
            volume: delta_vol,
        });
    }
    pts
}

fn parse_tencent_market_state(qt_obj: &Value, market: &str) -> String {
    let market_str = qt_obj["market"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .unwrap_or("");
    let prefix = match market {
        "HK" => "HK_",
        "US" => "US_",
        _ => "SH_",
    };
    for seg in market_str.split('|') {
        if let Some(rest) = seg.strip_prefix(prefix) {
            if rest.starts_with("open") {
                return "交易中".into();
            }
            if rest.starts_with("close") {
                return "已休市".into();
            }
        }
    }
    "已休市".into()
}

async fn fetch_tencent_quote(
    symbol: &str,
    fallback_name: &str,
    tencent_code: &str,
    max_points: usize,
) -> Result<StockQuote, String> {
    let url = format!("https://web.ifzq.gtimg.cn/appstock/app/minute/query?code={tencent_code}");
    let bytes = traced_get_bytes("腾讯实时", &url, None).await?;
    let body: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("腾讯行情 JSON 解析失败: {e}"))?;
    let entry = &body["data"][tencent_code];
    let qt_obj = &entry["qt"];
    let qt = qt_obj[tencent_code]
        .as_array()
        .ok_or_else(|| "腾讯行情无报价数据".to_string())?;
    let get_str = |idx: usize| qt.get(idx).and_then(Value::as_str).unwrap_or("");

    let market = detect_market(symbol).to_string();
    let raw_name = get_str(1).trim();
    let name = if !raw_name.is_empty() {
        raw_name.to_string()
    } else if !fallback_name.is_empty() {
        fallback_name.to_string()
    } else {
        symbol.to_string()
    };

    let price = parse_f64_opt(get_str(3)).filter(|p| *p > 0.0);
    let prev_close = parse_f64_opt(get_str(4)).filter(|p| *p > 0.0);
    let open = parse_f64_opt(get_str(5)).filter(|p| *p > 0.0);
    let volume = parse_f64_opt(get_str(6)).map(|v| {
        if market == "CN" {
            (v * 100.0).max(0.0) as u64
        } else {
            v.max(0.0) as u64
        }
    });
    let quote_time = parse_tencent_timestamp(get_str(30));
    let change = parse_f64_opt(get_str(31)).or(match (price, prev_close) {
        (Some(p), Some(pc)) => Some(p - pc),
        _ => None,
    });
    let change_percent = parse_f64_opt(get_str(32)).or_else(|| match (change, prev_close) {
        (Some(c), Some(pc)) if pc > 0.0 => Some((c / pc) * 100.0),
        _ => None,
    });
    let high = parse_f64_opt(get_str(33)).filter(|p| *p > 0.0);
    let low = parse_f64_opt(get_str(34)).filter(|p| *p > 0.0);
    let market_cap = parse_f64_opt(get_str(45))
        .filter(|v| *v > 0.0)
        .map(|yi| yi * 1e8);
    let turnover = tencent_turnover(get_str(35), get_str(37), &market);

    let date_str = entry["data"]["date"].as_str().unwrap_or("");
    let raw_pts = entry["data"]["data"]
        .as_array()
        .map(|a| parse_tencent_minute_points(date_str, a))
        .unwrap_or_default();
    let sparkline = downsample_points(raw_pts, max_points);

    let exchange = if tencent_code.starts_with("sh") {
        "SSE"
    } else if tencent_code.starts_with("sz") {
        "SZSE"
    } else if tencent_code.starts_with("bj") {
        "BSE"
    } else if tencent_code.starts_with("hk") {
        "HKEX"
    } else {
        "US"
    }
    .to_string();

    Ok(StockQuote {
        symbol: symbol.to_string(),
        name,
        market: market.clone(),
        currency: default_currency(&market).to_string(),
        exchange,
        price,
        prev_close,
        change,
        change_percent,
        open,
        high,
        low,
        volume,
        market_cap,
        turnover,
        extended_price: None,
        extended_change_percent: None,
        extended_label: None,
        market_state: parse_tencent_market_state(qt_obj, &market),
        source: "腾讯实时".into(),
        quote_time,
        sparkline,
        error: None,
    })
}

// ---------------------------------------------------------------------------
// 2. Official TWSE / TPEx MIS real-time batch API (Taiwan stocks, ~5s lag)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct TwseSnapshot {
    name: Option<String>,
    price: Option<f64>,
    prev_close: Option<f64>,
    open: Option<f64>,
    high: Option<f64>,
    low: Option<f64>,
    volume: Option<u64>,
    turnover: Option<f64>,
    quote_time: Option<i64>,
    exchange: String,
    from_daily: bool,
}

async fn fetch_twse_batch(channels: &[(String, String)]) -> HashMap<String, TwseSnapshot> {
    if channels.is_empty() {
        return HashMap::new();
    }
    // TWSE MIS accepts up to ~60-80 channels per call; chunks of 50 with concurrency 2
    // cuts TW150 from 6 calls down to 3 and avoids triggering MIS rate limits.
    let chunks: Vec<Vec<(String, String)>> = channels.chunks(50).map(|c| c.to_vec()).collect();
    let batches: Vec<HashMap<String, TwseSnapshot>> = stream::iter(chunks)
        .map(|chunk| async move {
            let first = fetch_twse_batch_chunk(&chunk).await;
            if !first.is_empty() || chunk.is_empty() {
                return first;
            }
            tokio::time::sleep(Duration::from_millis(350)).await;
            fetch_twse_batch_chunk(&chunk).await
        })
        .buffer_unordered(2)
        .collect()
        .await;
    let mut out = HashMap::new();
    for map in batches {
        out.extend(map);
    }
    out
}

/// Parse ROC calendar date used by TWSE OpenAPI (e.g. `"1151123"` -> 2026-11-23 13:30 +08:00).
fn parse_twse_roc_date_ts(raw: &str) -> Option<i64> {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 7 {
        return None;
    }
    let (y_str, rest) = digits.split_at(digits.len() - 4);
    let roc_year: i32 = y_str.parse().ok()?;
    let month: u32 = rest[0..2].parse().ok()?;
    let day: u32 = rest[2..4].parse().ok()?;
    let date = NaiveDate::from_ymd_opt(roc_year + 1911, month, day)?;
    let close_time = NaiveTime::from_hms_opt(13, 30, 0)?;
    let dt = date.and_time(close_time);
    // Taiwan is fixed UTC+8 (no DST).
    let tz = chrono::FixedOffset::east_opt(8 * 3600)?;
    tz.from_local_datetime(&dt).single().map(|d| d.timestamp())
}

fn parse_twse_day_all(bytes: &[u8]) -> HashMap<String, TwseSnapshot> {
    let mut out = HashMap::new();
    let Ok(arr) = serde_json::from_slice::<Vec<Value>>(bytes) else {
        return out;
    };
    for item in arr {
        let Some(code) = item["Code"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(close) = item["ClosingPrice"]
            .as_str()
            .and_then(parse_f64_opt)
            .filter(|v| *v > 0.0)
        else {
            continue;
        };
        let change_raw = item["Change"].as_str().map(str::trim).unwrap_or("");
        // TWSE prefixes ex-dividend/reference changes with "X"; ignore those for prev_close math.
        let change = if change_raw.starts_with('X') || change_raw.starts_with('x') {
            None
        } else {
            parse_f64_opt(change_raw.trim_start_matches('+'))
        };
        let prev_close = change
            .map(|ch| close - ch)
            .filter(|v| v.is_finite() && *v > 0.0)
            .or(Some(close));
        let open = item["OpeningPrice"]
            .as_str()
            .and_then(parse_f64_opt)
            .filter(|v| *v > 0.0);
        let high = item["HighestPrice"]
            .as_str()
            .and_then(parse_f64_opt)
            .filter(|v| *v > 0.0);
        let low = item["LowestPrice"]
            .as_str()
            .and_then(parse_f64_opt)
            .filter(|v| *v > 0.0);
        let volume = item["TradeVolume"]
            .as_str()
            .and_then(parse_f64_opt)
            .map(|v| v.max(0.0) as u64);
        let turnover = item["TradeValue"]
            .as_str()
            .and_then(parse_f64_opt)
            .filter(|v| *v > 0.0);
        let quote_time = item["Date"].as_str().and_then(parse_twse_roc_date_ts);
        let name = item["Name"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let sym = format!("{}.TW", code.to_ascii_uppercase());
        out.insert(
            sym,
            TwseSnapshot {
                name,
                price: Some(close),
                prev_close,
                open,
                high,
                low,
                volume,
                turnover,
                quote_time,
                exchange: "TWSE".into(),
                from_daily: true,
            },
        );
    }
    out
}

/// Official TWSE OpenAPI daily summary for all listed TWSE stocks (~1,000 rows in one request).
/// Cached for 5 minutes so pre-market / off-hours / MIS-gap fills cost a single lightweight call.
/// Whether the Taiwan Stock Exchange is in its trading session (09:00-13:30 Taipei
/// time, UTC+8, Monday to Friday). Holidays are not known here and count as open,
/// which only means the daily summary is not used to fill gaps on those days.
fn twse_session_open(now: chrono::DateTime<chrono::Utc>) -> bool {
    use chrono::{Datelike, Timelike};
    let Some(taipei) = chrono::FixedOffset::east_opt(8 * 3600) else {
        return false;
    };
    let t = now.with_timezone(&taipei);
    let weekday = t.weekday().num_days_from_monday() < 5;
    let minutes = t.hour() * 60 + t.minute();
    weekday && (9 * 60..13 * 60 + 30).contains(&minutes)
}

async fn fetch_twse_day_all_cached() -> HashMap<String, TwseSnapshot> {
    /// When the summary was fetched, and the snapshots by symbol.
    type DayAllCache = Option<(std::time::Instant, HashMap<String, TwseSnapshot>)>;
    static CACHE: OnceLock<std::sync::Mutex<DayAllCache>> = OnceLock::new();
    let lock = CACHE.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(guard) = lock.lock() {
        if let Some((at, ref map)) = *guard {
            if at.elapsed() < Duration::from_secs(300) && !map.is_empty() {
                return map.clone();
            }
        }
    }
    let url = "https://openapi.twse.com.tw/v1/exchangeReport/STOCK_DAY_ALL";
    let Ok(bytes) = traced_get_bytes("TWSE 官方日行情", url, None).await else {
        return HashMap::new();
    };
    let parsed = parse_twse_day_all(&bytes);
    if !parsed.is_empty() {
        if let Ok(mut guard) = lock.lock() {
            *guard = Some((std::time::Instant::now(), parsed.clone()));
        }
    }
    parsed
}

async fn fetch_twse_batch_chunk(channels: &[(String, String)]) -> HashMap<String, TwseSnapshot> {
    let mut out = HashMap::new();
    if channels.is_empty() {
        return out;
    }
    let ex_ch = channels
        .iter()
        .map(|(_, ch)| ch.as_str())
        .collect::<Vec<_>>()
        .join("|");
    let url =
        format!("https://mis.twse.com.tw/stock/api/getStockInfo.jsp?ex_ch={ex_ch}&json=1&delay=0");
    let Ok(bytes) = traced_get_bytes(
        "TWSE 官方实时",
        &url,
        Some("https://mis.twse.com.tw/stock/fibest.jsp"),
    )
    .await
    else {
        return out;
    };
    let Ok(body) = serde_json::from_slice::<Value>(&bytes) else {
        return out;
    };
    let Some(arr) = body["msgArray"].as_array() else {
        return out;
    };

    let mut ch_to_sym: HashMap<String, String> = HashMap::new();
    for (sym, ch) in channels {
        // ch is `tse_2330.tw` -> msgArray `ch` is `2330.tw` or `key` starts with `tse_2330.tw`
        ch_to_sym.insert(ch.to_ascii_lowercase(), sym.clone());
        if let Some((_, tail)) = ch.split_once('_') {
            ch_to_sym.insert(tail.to_ascii_lowercase(), sym.clone());
        }
    }

    for item in arr {
        let key_ch = item["ch"].as_str().unwrap_or("").to_ascii_lowercase();
        let ex = item["ex"].as_str().unwrap_or("tse").to_ascii_lowercase();
        let full_key = format!("{ex}_{key_ch}");
        let Some(sym) = ch_to_sym
            .get(&full_key)
            .or_else(|| ch_to_sym.get(&key_ch))
            .cloned()
        else {
            continue;
        };
        // `z` is latest trade price; when market is in auction/pre-close `z` can be "-",
        // fall back to `pz` (indicated price) or best bid `b` first element.
        let best_bid = item["b"]
            .as_str()
            .and_then(|s| s.split('_').next())
            .and_then(parse_f64_opt)
            .filter(|v| *v > 0.0);
        let price = item["z"]
            .as_str()
            .and_then(parse_f64_opt)
            .filter(|v| *v > 0.0)
            .or_else(|| {
                item["pz"]
                    .as_str()
                    .and_then(parse_f64_opt)
                    .filter(|v| *v > 0.0)
            })
            .or(best_bid);
        let prev_close = item["y"]
            .as_str()
            .and_then(parse_f64_opt)
            .filter(|v| *v > 0.0);
        let open = item["o"]
            .as_str()
            .and_then(parse_f64_opt)
            .filter(|v| *v > 0.0);
        let high = item["h"]
            .as_str()
            .and_then(parse_f64_opt)
            .filter(|v| *v > 0.0);
        let low = item["l"]
            .as_str()
            .and_then(parse_f64_opt)
            .filter(|v| *v > 0.0);
        // TWSE `v` is in lots (張 = 1,000 shares)
        let volume = item["v"]
            .as_str()
            .and_then(parse_f64_opt)
            .map(|v| (v * 1000.0).max(0.0) as u64);
        let quote_time = item["tlong"]
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .map(|ms| ms / 1000);
        let name = item["n"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let exchange = if ex == "otc" { "TPEx" } else { "TWSE" }.to_string();

        let turnover = match (price, volume) {
            (Some(p), Some(v)) if p > 0.0 && v > 0 => Some(p * v as f64),
            _ => None,
        };
        out.insert(
            sym,
            TwseSnapshot {
                name,
                price,
                prev_close,
                open,
                high,
                low,
                volume,
                turnover,
                quote_time,
                exchange,
                from_daily: false,
            },
        );
    }
    out
}

// ---------------------------------------------------------------------------
// 3. Yahoo Finance v8 Chart fetcher (US real-time + pre/post + global charts)
// ---------------------------------------------------------------------------

fn yahoo_interval_for_range(range: &str) -> (&'static str, &'static str) {
    match range {
        "5d" => ("5d", "15m"),
        "1mo" => ("1mo", "1d"),
        "3mo" => ("3mo", "1d"),
        "6mo" => ("6mo", "1d"),
        "1y" => ("1y", "1d"),
        "2y" => ("2y", "1wk"),
        "5y" => ("5y", "1wk"),
        "max" => ("max", "1mo"),
        _ => ("1d", "5m"),
    }
}

fn yahoo_market_state_label(meta: &Value) -> String {
    if let Some(state) = meta["marketState"].as_str() {
        return match state {
            "REGULAR" => "交易中".into(),
            "PRE" | "PREPRE" => "盘前".into(),
            "POST" | "POSTPOST" => "盘后".into(),
            _ => "已休市".into(),
        };
    }
    // Infer from tradingPeriods.regular when marketState is omitted
    let now = chrono::Utc::now().timestamp();
    if let Some(reg) = meta["currentTradingPeriod"]["regular"].as_object() {
        let s = reg.get("start").and_then(Value::as_i64).unwrap_or(0);
        let e = reg.get("end").and_then(Value::as_i64).unwrap_or(0);
        if now >= s && now <= e {
            return "交易中".into();
        }
    }
    if let Some(pre) = meta["currentTradingPeriod"]["pre"].as_object() {
        let s = pre.get("start").and_then(Value::as_i64).unwrap_or(0);
        let e = pre.get("end").and_then(Value::as_i64).unwrap_or(0);
        if now >= s && now < e {
            return "盘前".into();
        }
    }
    if let Some(post) = meta["currentTradingPeriod"]["post"].as_object() {
        let s = post.get("start").and_then(Value::as_i64).unwrap_or(0);
        let e = post.get("end").and_then(Value::as_i64).unwrap_or(0);
        if now > s && now <= e {
            return "盘后".into();
        }
    }
    "已休市".into()
}

async fn fetch_yahoo_chart_raw(
    symbol: &str,
    range: &str,
    include_pre_post: bool,
) -> Result<(Value, Vec<ChartPoint>), String> {
    let (r, interval) = yahoo_interval_for_range(range);
    // Path segment, not a form value: a space must be %20, not `+`.
    let encoded = url::form_urlencoded::byte_serialize(symbol.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    let url = format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{encoded}?interval={interval}&range={r}&includePrePost={include_pre_post}"
    );
    let bytes = traced_get_bytes("Yahoo 实时", &url, None).await?;
    let body: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("Yahoo JSON 解析失败: {e}"))?;
    let result = body["chart"]["result"]
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(|| {
            body["chart"]["error"]["description"]
                .as_str()
                .unwrap_or("无行情结果")
                .to_string()
        })?;

    let meta = result["meta"].clone();
    let timestamps = result["timestamp"].as_array();
    let quote_obj = result["indicators"]["quote"]
        .as_array()
        .and_then(|a| a.first());
    let opens = quote_obj.and_then(|q| q["open"].as_array());
    let highs = quote_obj.and_then(|q| q["high"].as_array());
    let lows = quote_obj.and_then(|q| q["low"].as_array());
    let closes = quote_obj.and_then(|q| q["close"].as_array());
    let volumes = quote_obj.and_then(|q| q["volume"].as_array());

    let mut points = Vec::new();
    if let (Some(ts_arr), Some(close_arr)) = (timestamps, closes) {
        for (i, (ts_v, c_v)) in ts_arr.iter().zip(close_arr.iter()).enumerate() {
            if let (Some(ts), Some(price)) = (ts_v.as_i64(), c_v.as_f64()) {
                if price.is_finite() && price > 0.0 {
                    let open = opens
                        .and_then(|a| a.get(i))
                        .and_then(Value::as_f64)
                        .filter(|v| v.is_finite() && *v > 0.0);
                    let high = highs
                        .and_then(|a| a.get(i))
                        .and_then(Value::as_f64)
                        .filter(|v| v.is_finite() && *v > 0.0);
                    let low = lows
                        .and_then(|a| a.get(i))
                        .and_then(Value::as_f64)
                        .filter(|v| v.is_finite() && *v > 0.0);
                    let volume = volumes.and_then(|a| a.get(i)).and_then(Value::as_u64);
                    points.push(ChartPoint {
                        ts,
                        price,
                        open,
                        high,
                        low,
                        volume,
                    });
                }
            }
        }
    }
    Ok((meta, points))
}

async fn fetch_yahoo_quote(
    symbol: &str,
    fallback_name: &str,
    max_points: usize,
) -> Result<StockQuote, String> {
    let (meta, points) = fetch_yahoo_chart_raw(symbol, "1d", false).await?;
    let market = detect_market(symbol).to_string();
    let price = meta["regularMarketPrice"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0)
        .or_else(|| points.last().map(|p| p.price));
    let prev_close = meta["chartPreviousClose"]
        .as_f64()
        .or_else(|| meta["previousClose"].as_f64())
        .filter(|v| v.is_finite() && *v > 0.0);
    let change = match (price, prev_close) {
        (Some(p), Some(pc)) => Some(p - pc),
        _ => None,
    };
    let change_percent = meta["regularMarketChangePercent"]
        .as_f64()
        .filter(|v| v.is_finite())
        .or_else(|| match (change, prev_close) {
            (Some(c), Some(pc)) if pc > 0.0 => Some((c / pc) * 100.0),
            _ => None,
        });

    let state = yahoo_market_state_label(&meta);
    let (extended_price, extended_change_percent, extended_label) = if let Some(pre) = meta
        ["preMarketPrice"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0)
    {
        let pct = price.filter(|p| *p > 0.0).map(|p| ((pre - p) / p) * 100.0);
        (Some(pre), pct, Some("盘前".to_string()))
    } else if let Some(post) = meta["postMarketPrice"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0)
    {
        let pct = price.filter(|p| *p > 0.0).map(|p| ((post - p) / p) * 100.0);
        (Some(post), pct, Some("盘后".to_string()))
    } else {
        (None, None, None)
    };

    let name = if !fallback_name.is_empty() {
        fallback_name.to_string()
    } else {
        meta["shortName"]
            .as_str()
            .or_else(|| meta["longName"].as_str())
            .unwrap_or(symbol)
            .to_string()
    };

    Ok(StockQuote {
        symbol: symbol.to_string(),
        name,
        market: market.clone(),
        currency: meta["currency"]
            .as_str()
            .unwrap_or(default_currency(&market))
            .to_string(),
        exchange: meta["exchangeName"]
            .as_str()
            .or_else(|| meta["fullExchangeName"].as_str())
            .unwrap_or(&market)
            .to_string(),
        price,
        prev_close,
        change,
        change_percent,
        open: meta["regularMarketOpen"]
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0.0)
            .or_else(|| points.first().map(|p| p.price)),
        high: meta["regularMarketDayHigh"]
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0.0),
        low: meta["regularMarketDayLow"]
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0.0),
        volume: meta["regularMarketVolume"].as_u64(),
        market_cap: None,
        turnover: None,
        extended_price,
        extended_change_percent,
        extended_label,
        market_state: state,
        source: "Yahoo 实时".into(),
        quote_time: meta["regularMarketTime"].as_i64(),
        sparkline: downsample_points(points, max_points),
        error: None,
    })
}

// ---------------------------------------------------------------------------
// Unified multi-market router
// ---------------------------------------------------------------------------

/// Largest quote request accepted; the biggest built-in pool (S&P 500) has 503.
pub const MAX_QUOTE_ITEMS: usize = 600;

/// The item's market, falling back to detection from the symbol.
fn market_of(item: &WatchItem) -> &str {
    if item.market.is_empty() {
        detect_market(&item.symbol)
    } else {
        item.market.as_str()
    }
}

pub async fn fetch_quotes(items: Vec<WatchItem>, skip_sparkline: bool) -> Vec<StockQuote> {
    let use_fast_batch = skip_sparkline || items.len() > 24;

    // 1. Batch-fetch Taiwan (TWSE MIS, chunks of 25) and CN/HK/US (Tencent qt.gtimg.cn, chunks of 45)
    //    concurrently. For heatmaps / large 50-90 stock pools this finishes in ~200-350ms total!
    let tw_channels: Vec<(String, String)> = items
        .iter()
        .filter_map(|it| to_twse_channel(&it.symbol).map(|ch| (it.symbol.clone(), ch)))
        .collect();
    // Small lists take the per-symbol route (intraday points), where quotes can come
    // from Yahoo, which reports no market cap or traded value. One Tencent batch
    // request for the list's CN / HK / US symbols fills both in (see `batch_extras`).
    let batchable: Vec<WatchItem> = if use_fast_batch {
        Vec::new()
    } else {
        items
            .iter()
            .filter(|it| matches!(market_of(it), "US" | "CN" | "HK"))
            .cloned()
            .collect()
    };
    let (mut twse_map, tencent_batch_map, us_batch_map) = if use_fast_batch {
        let (twse, tencent) =
            tokio::join!(fetch_twse_batch(&tw_channels), fetch_tencent_batch(&items));
        (twse, tencent, HashMap::new())
    } else {
        let (twse, extra) = tokio::join!(
            fetch_twse_batch(&tw_channels),
            fetch_tencent_batch(&batchable)
        );
        (twse, HashMap::new(), extra)
    };
    // TWSE MIS clears `z` to "-" before 09:00 Taipei time (and can drop chunks on rate limit).
    // When any `.TW` symbol lacks a live price, fill all missing `.TW` stocks in one cached call
    // to TWSE's official daily summary (`STOCK_DAY_ALL`) so Taiwan pools never blank out.
    // Only outside the trading session: during it the summary is the previous day's
    // close and change, which would show yesterday's move as today's for any stock
    // whose live quote is missing (a chunk dropped by the rate limit, say). Those
    // stay on the Yahoo route and the last good quote instead.
    let needs_twse_daily = !twse_session_open(chrono::Utc::now())
        && tw_channels.iter().any(|(sym, _)| {
            sym.to_ascii_uppercase().ends_with(".TW")
                && twse_map.get(sym).and_then(|s| s.price).is_none()
        });
    if needs_twse_daily {
        let daily_map = fetch_twse_day_all_cached().await;
        for (sym, _) in &tw_channels {
            if twse_map.get(sym).and_then(|s| s.price).is_some() {
                continue;
            }
            let key = sym.to_ascii_uppercase();
            if let Some(daily) = daily_map.get(&key) {
                twse_map.insert(sym.clone(), daily.clone());
            } else if let Some(existing) = twse_map.get_mut(sym) {
                // Final safety net: if MIS returned yesterday's close `y` pre-market, display it
                // instead of failing with "批量行情暂未返回".
                if existing.price.is_none() && existing.prev_close.is_some() {
                    existing.price = existing.prev_close;
                    existing.from_daily = true;
                }
            }
        }
    }
    // (market cap, traded value) per symbol from that extra batch request.
    let batch_extras: HashMap<String, (Option<f64>, Option<f64>)> = us_batch_map
        .into_iter()
        .map(|(symbol, q)| (symbol, (q.market_cap, q.turnover)))
        .collect();

    // 2. Resolve each item: return fast-batch hit directly when available; otherwise fall back to
    //    per-symbol routed fetcher (full sparklines + US pre/post market + Yahoo fallback).
    //    For mega-pools (>60 symbols), cap per-symbol fallback calls to avoid a Yahoo request storm.
    let is_mega_pool = use_fast_batch && items.len() > 60;
    let mut fallback_budget: usize = if is_mega_pool { 8 } else { usize::MAX };
    let mut tasks = Vec::with_capacity(items.len());
    for (idx, item) in items.into_iter().enumerate() {
        let batch_hit = tencent_batch_map.get(&item.symbol).cloned();
        let twse_hit = twse_map.get(&item.symbol).cloned();
        let has_fast_hit =
            batch_hit.is_some() || twse_hit.as_ref().is_some_and(|t| t.price.is_some());
        let allow_network_fallback = if has_fast_hit {
            true
        } else if fallback_budget > 0 {
            fallback_budget -= 1;
            true
        } else {
            false
        };
        let extras = batch_extras.get(&item.symbol).copied().unwrap_or_default();
        tasks.push((
            idx,
            item,
            batch_hit,
            twse_hit,
            allow_network_fallback,
            extras,
        ));
    }

    let indexed: Vec<(usize, StockQuote)> = stream::iter(tasks)
        .map(
            |(idx, item, batch_hit, twse_hit, allow_network_fallback, extras)| async move {
                if let Some(q) = batch_hit {
                    return (idx, q);
                }
                if !allow_network_fallback {
                    return (idx, error_quote(&item, "批量行情暂未返回"));
                }
                let mut quote = fetch_single_routed(&item, twse_hit, use_fast_batch).await;
                let (market_cap, turnover) = extras;
                if quote.market_cap.is_none() {
                    quote.market_cap = market_cap;
                }
                if quote.turnover.is_none() {
                    quote.turnover = turnover;
                }
                (idx, quote)
            },
        )
        .buffer_unordered(10)
        .collect()
        .await;

    let mut ordered = indexed;
    ordered.sort_by_key(|(idx, _)| *idx);
    let quotes: Vec<StockQuote> = ordered.into_iter().map(|(_, q)| q).collect();
    static LAST_GOOD: std::sync::LazyLock<std::sync::Mutex<HashMap<String, StockQuote>>> =
        std::sync::LazyLock::new(Default::default);
    match LAST_GOOD.lock() {
        Ok(mut cache) => with_last_good(quotes, &mut cache),
        Err(_) => quotes,
    }
}

/// Remember each symbol's last good quote and show it, marked as cached, when a
/// later fetch fails, so a blocked or flaky source leaves the last known prices on
/// screen instead of a page of errors. The quote keeps its original time, so the
/// page can still tell how old it is.
fn with_last_good(
    quotes: Vec<StockQuote>,
    cache: &mut HashMap<String, StockQuote>,
) -> Vec<StockQuote> {
    // Bounded: more symbols than every built-in pool together.
    if cache.len() > 4000 {
        cache.clear();
    }
    quotes
        .into_iter()
        .map(|mut q| {
            if q.price.is_some() && q.error.is_none() {
                // Yahoo quotes carry no market cap. Reuse the last known one, scaled
                // by the price change (the share count does not move intraday), so
                // a fallback quote still sizes correctly on the heat map.
                if q.market_cap.is_none() {
                    if let Some(prev) = cache.get(&q.symbol) {
                        if let (Some(cap), Some(prev_price), Some(price)) =
                            (prev.market_cap, prev.price, q.price)
                        {
                            if prev_price > 0.0 {
                                q.market_cap = Some(cap * price / prev_price);
                            }
                        }
                    }
                }
                cache.insert(q.symbol.clone(), q.clone());
                q
            } else if let Some(prev) = cache.get(&q.symbol) {
                let mut cached = prev.clone();
                cached.source = format!("{}（缓存）", prev.source);
                cached
            } else {
                q
            }
        })
        .collect()
}

async fn fetch_single_routed(
    item: &WatchItem,
    twse_hit: Option<TwseSnapshot>,
    skip_sparkline: bool,
) -> StockQuote {
    let market = if item.market.is_empty() {
        detect_market(&item.symbol)
    } else {
        item.market.as_str()
    };

    // Route A: A-shares & Hong Kong -> Tencent UTF-8 real-time (1-3s lag), fallback Yahoo.
    if matches!(market, "CN" | "HK") {
        if let Some(code) =
            to_tencent_code(&item.symbol).filter(|_| tencent_available(TencentService::Minute))
        {
            match fetch_tencent_quote(&item.symbol, &item.name, &code, 48).await {
                Ok(q) => {
                    tencent_report(TencentService::Minute, true);
                    return q;
                }
                // An unknown symbol is not a sign of a block; failed requests are.
                Err(e) if e.contains("无报价数据") => {}
                Err(_) => tencent_report(TencentService::Minute, false),
            }
        }
        if let Ok(q) = fetch_yahoo_quote(&item.symbol, &item.name, 48).await {
            return q;
        }
        return error_quote(item, "行情源暂不可用");
    }

    // Route B: Taiwan -> Official TWSE/TPEx MIS real-time (with STOCK_DAY_ALL pre-market/off-hours fill).
    if market == "TW" {
        let has_twse_price = twse_hit.as_ref().is_some_and(|t| t.price.is_some());
        let yahoo_res = if skip_sparkline && has_twse_price {
            Err(String::new())
        } else {
            fetch_yahoo_quote(&item.symbol, &item.name, 48).await
        };
        if let Some(tw) = twse_hit {
            let source_label = if tw.from_daily {
                "TWSE 官方日行情"
            } else {
                "TWSE 官方实时"
            };
            let mut base = yahoo_res.unwrap_or_else(|_| StockQuote {
                symbol: item.symbol.clone(),
                name: tw
                    .name
                    .clone()
                    .filter(|_| item.name.is_empty())
                    .unwrap_or_else(|| {
                        if item.name.is_empty() {
                            item.symbol.clone()
                        } else {
                            item.name.clone()
                        }
                    }),
                market: "TW".into(),
                currency: "TWD".into(),
                exchange: tw.exchange.clone(),
                price: None,
                prev_close: None,
                change: None,
                change_percent: None,
                open: None,
                high: None,
                low: None,
                volume: None,
                market_cap: None,
                turnover: None,
                extended_price: None,
                extended_change_percent: None,
                extended_label: None,
                market_state: if tw.from_daily {
                    "已收盘".into()
                } else {
                    "盘中".into()
                },
                source: source_label.into(),
                quote_time: None,
                sparkline: Vec::new(),
                error: None,
            });
            if let Some(p) = tw.price {
                base.price = Some(p);
                if let Some(pc) = tw.prev_close.or(base.prev_close) {
                    base.prev_close = Some(pc);
                    let ch = p - pc;
                    base.change = Some(ch);
                    if pc > 0.0 {
                        base.change_percent = Some((ch / pc) * 100.0);
                    }
                }
                base.source = source_label.into();
            }
            if tw.open.is_some() {
                base.open = tw.open;
            }
            if tw.high.is_some() {
                base.high = tw.high;
            }
            if tw.low.is_some() {
                base.low = tw.low;
            }
            if tw.volume.is_some() {
                base.volume = tw.volume;
            }
            if tw.turnover.is_some() {
                base.turnover = tw.turnover;
            }
            if tw.quote_time.is_some() {
                base.quote_time = tw.quote_time;
            }
            base.exchange = tw.exchange;
            if item.name.is_empty() {
                if let Some(n) = tw.name {
                    base.name = n;
                }
            }
            return base;
        }
        return yahoo_res.unwrap_or_else(|e| error_quote(item, &e));
    }

    // Route C: US & Global -> Yahoo Finance v8 real-time (+ pre/post), fallback Tencent US.
    match fetch_yahoo_quote(&item.symbol, &item.name, 48).await {
        Ok(q) => q,
        Err(yahoo_err) => {
            if market == "US" && item.symbol.chars().all(|c| c.is_ascii_alphanumeric()) {
                let code = format!("us{}", item.symbol.to_ascii_uppercase());
                if let Ok(q) = fetch_tencent_quote(&item.symbol, &item.name, &code, 48).await {
                    return q;
                }
            }
            error_quote(item, &yahoo_err)
        }
    }
}

fn error_quote(item: &WatchItem, err: &str) -> StockQuote {
    let market = if item.market.is_empty() {
        detect_market(&item.symbol).to_string()
    } else {
        item.market.clone()
    };
    StockQuote {
        symbol: item.symbol.clone(),
        name: if item.name.is_empty() {
            item.symbol.clone()
        } else {
            item.name.clone()
        },
        market: market.clone(),
        currency: default_currency(&market).to_string(),
        exchange: market,
        price: None,
        prev_close: None,
        change: None,
        change_percent: None,
        open: None,
        high: None,
        low: None,
        volume: None,
        market_cap: None,
        turnover: None,
        extended_price: None,
        extended_change_percent: None,
        extended_label: None,
        market_state: "不可用".into(),
        source: "—".into(),
        quote_time: None,
        sparkline: Vec::new(),
        error: Some(err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// High-resolution single-symbol chart (1d / 5d / 1mo / 3mo / 6mo / 1y / 2y / 5y / max)
// ---------------------------------------------------------------------------

pub async fn fetch_chart(symbol: &str, range: &str) -> Result<StockChartDetail, AppError> {
    let sym = symbol.trim();
    if sym.is_empty() {
        return Err(AppError::InvalidInput("symbol is required".into()));
    }
    let market = detect_market(sym);
    if range == "1d" && matches!(market, "CN" | "HK") && tencent_available(TencentService::Minute) {
        if let Some(code) = to_tencent_code(sym) {
            let fetched = fetch_tencent_quote(sym, "", &code, 260).await;
            match &fetched {
                Ok(_) => tencent_report(TencentService::Minute, true),
                Err(e) if e.contains("无报价数据") => {}
                Err(_) => tencent_report(TencentService::Minute, false),
            }
            if let Ok(q) = fetched {
                if !q.sparkline.is_empty() {
                    return Ok(StockChartDetail {
                        symbol: sym.to_string(),
                        range: range.to_string(),
                        currency: q.currency,
                        prev_close: q.prev_close,
                        source: "腾讯分时".into(),
                        points: q.sparkline,
                        fifty_two_week_high: None,
                        fifty_two_week_low: None,
                    });
                }
            }
        }
    }

    let (meta, points) = fetch_yahoo_chart_raw(sym, range, false)
        .await
        .map_err(AppError::Message)?;
    let fifty_two_week_high = meta["fiftyTwoWeekHigh"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0);
    let fifty_two_week_low = meta["fiftyTwoWeekLow"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0);
    if range == "max" {
        if let Some(ext) =
            compute_extremes_from_points(&points, fifty_two_week_high, fifty_two_week_low)
        {
            upsert_extremes_memory(sym, ext);
        }
    }
    Ok(StockChartDetail {
        symbol: sym.to_string(),
        range: range.to_string(),
        currency: meta["currency"]
            .as_str()
            .unwrap_or(default_currency(market))
            .to_string(),
        prev_close: meta["chartPreviousClose"]
            .as_f64()
            .or_else(|| meta["previousClose"].as_f64())
            .filter(|v| v.is_finite() && *v > 0.0),
        source: "Yahoo Finance".into(),
        points: downsample_points(points, 420),
        fifty_two_week_high,
        fifty_two_week_low,
    })
}

// ---------------------------------------------------------------------------
// All-Time High / All-Time Low (ATH / ATL) persistent cache + paced hydration
// ---------------------------------------------------------------------------

static EXTREMES_MEM_CACHE: OnceLock<std::sync::Mutex<Option<HashMap<String, PriceExtremes>>>> =
    OnceLock::new();
static EXTREMES_FAIL_CACHE: OnceLock<std::sync::Mutex<HashMap<String, std::time::Instant>>> =
    OnceLock::new();

fn extremes_mem() -> &'static std::sync::Mutex<Option<HashMap<String, PriceExtremes>>> {
    EXTREMES_MEM_CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

fn extremes_fail() -> &'static std::sync::Mutex<HashMap<String, std::time::Instant>> {
    EXTREMES_FAIL_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn upsert_extremes_memory(symbol: &str, ext: PriceExtremes) {
    if let Ok(mut guard) = extremes_mem().lock() {
        if let Some(map) = guard.as_mut() {
            map.insert(symbol.trim().to_ascii_uppercase(), ext);
        }
    }
}

pub fn load_extremes_cache(db: &Database) -> Result<HashMap<String, PriceExtremes>, AppError> {
    if let Ok(guard) = extremes_mem().lock() {
        if let Some(ref map) = *guard {
            return Ok(map.clone());
        }
    }
    let loaded: HashMap<String, PriceExtremes> = db
        .get_setting(EXTREMES_CACHE_KEY)?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    if let Ok(mut guard) = extremes_mem().lock() {
        *guard = Some(loaded.clone());
    }
    Ok(loaded)
}

pub fn merge_and_save_extremes(
    db: &Database,
    updates: HashMap<String, PriceExtremes>,
) -> Result<HashMap<String, PriceExtremes>, AppError> {
    let mut map = load_extremes_cache(db)?;
    for (k, v) in updates {
        map.insert(k, v);
    }
    if let Ok(mut guard) = extremes_mem().lock() {
        *guard = Some(map.clone());
    }
    let raw = serde_json::to_string(&map)
        .map_err(|e| AppError::Message(format!("序列化历史高低点缓存失败: {e}")))?;
    db.set_setting_quietly(EXTREMES_CACHE_KEY, &raw)?;
    Ok(map)
}

fn compute_extremes_from_points(
    points: &[ChartPoint],
    wk52_high: Option<f64>,
    wk52_low: Option<f64>,
) -> Option<PriceExtremes> {
    let mut ath: Option<(f64, Option<i64>)> = None;
    let mut atl: Option<(f64, Option<i64>)> = None;
    for pt in points {
        let hi = pt
            .high
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(pt.price);
        let lo = pt
            .low
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(pt.price);
        if hi.is_finite() && hi > 0.0 && ath.is_none_or(|(cur, _)| hi > cur) {
            ath = Some((hi, Some(pt.ts)));
        }
        if lo.is_finite() && lo > 0.0 && atl.is_none_or(|(cur, _)| lo < cur) {
            atl = Some((lo, Some(pt.ts)));
        }
    }
    let (mut ath_val, mut ath_ts) = ath?;
    let (mut atl_val, mut atl_ts) = atl?;
    if let Some(h52) = wk52_high.filter(|v| v.is_finite() && *v > ath_val) {
        ath_val = h52;
        ath_ts = None;
    }
    if let Some(l52) = wk52_low.filter(|v| v.is_finite() && *v > 0.0 && *v < atl_val) {
        atl_val = l52;
        atl_ts = None;
    }
    Some(PriceExtremes {
        ath: ath_val,
        ath_ts,
        atl: atl_val,
        atl_ts,
        fetched_at: chrono::Utc::now().timestamp(),
    })
}

fn parse_tencent_date_ts(date_str: &str) -> Option<i64> {
    let d = NaiveDate::parse_from_str(date_str.trim(), "%Y-%m-%d").ok()?;
    let t = NaiveTime::from_hms_opt(15, 0, 0)?;
    let offset = chrono::FixedOffset::east_opt(8 * 3600)?;
    offset
        .from_local_datetime(&d.and_time(t))
        .single()
        .map(|dt| dt.timestamp())
}

fn parse_tencent_monthly_extremes(bytes: &[u8], code: &str) -> Option<PriceExtremes> {
    let body: Value = serde_json::from_slice(bytes).ok()?;
    let entry = &body["data"][code];
    let rows = entry["qfqmonth"]
        .as_array()
        .or_else(|| entry["month"].as_array())?;
    let mut points = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(arr) = row.as_array() else { continue };
        let Some(date_s) = arr.first().and_then(Value::as_str) else {
            continue;
        };
        let open = arr.get(1).and_then(Value::as_str).and_then(parse_f64_opt);
        let close = arr.get(2).and_then(Value::as_str).and_then(parse_f64_opt);
        let high = arr.get(3).and_then(Value::as_str).and_then(parse_f64_opt);
        let low = arr.get(4).and_then(Value::as_str).and_then(parse_f64_opt);
        let Some(price) = close.filter(|v| *v > 0.0) else {
            continue;
        };
        let ts = parse_tencent_date_ts(date_s).unwrap_or(0);
        points.push(ChartPoint {
            ts,
            price,
            open,
            high,
            low,
            volume: None,
        });
    }
    compute_extremes_from_points(&points, None, None)
}

async fn fetch_single_extremes(symbol: &str) -> Option<PriceExtremes> {
    if let Ok((meta, points)) = fetch_yahoo_chart_raw(symbol, "max", false).await {
        let h52 = meta["fiftyTwoWeekHigh"].as_f64();
        let l52 = meta["fiftyTwoWeekLow"].as_f64();
        if let Some(ext) = compute_extremes_from_points(&points, h52, l52) {
            return Some(ext);
        }
    }
    // The fallback is served by web.ifzq.gtimg.cn, the same host as the intraday
    // quotes, so it shares that service's back-off and reports to it.
    if let Some(code) =
        to_tencent_batch_code(symbol).filter(|_| tencent_available(TencentService::Minute))
    {
        let url = format!(
            "https://web.ifzq.gtimg.cn/appstock/app/fqkline/get?param={code},month,,,500,qfq"
        );
        match traced_get_bytes("腾讯历史月K", &url, None).await {
            Ok(bytes) if !looks_blocked(&bytes) => {
                tencent_report(TencentService::Minute, true);
                if let Some(ext) = parse_tencent_monthly_extremes(&bytes, &code) {
                    return Some(ext);
                }
            }
            _ => tencent_report(TencentService::Minute, false),
        }
    }
    None
}

/// Return cached ATH/ATL for requested symbols and hydrate up to `batch_limit` missing ones.
pub async fn fetch_extremes_batch(
    cached: HashMap<String, PriceExtremes>,
    symbols: Vec<String>,
    batch_limit: usize,
) -> (HashMap<String, PriceExtremes>, ExtremesBatchResponse) {
    let now = chrono::Utc::now().timestamp();
    let fail_ttl = Duration::from_secs(300);

    let mut unique_syms: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for s in symbols {
        let key = s.trim().to_ascii_uppercase();
        if !key.is_empty() && seen.insert(key.clone()) {
            unique_syms.push(key);
        }
    }

    let mut result_items: HashMap<String, PriceExtremes> = HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    for sym in &unique_syms {
        if let Some(entry) = cached.get(sym) {
            result_items.insert(sym.clone(), entry.clone());
            if now - entry.fetched_at < EXTREMES_TTL_SECS {
                continue;
            }
        }
        let recently_failed = extremes_fail()
            .lock()
            .ok()
            .and_then(|m| m.get(sym).copied())
            .is_some_and(|at| at.elapsed() < fail_ttl);
        if !recently_failed {
            missing.push(sym.clone());
        }
    }

    let limit = batch_limit.min(EXTREMES_MAX_PER_CALL);
    let to_fetch: Vec<String> = missing.iter().take(limit).cloned().collect();
    let remaining = missing.len().saturating_sub(to_fetch.len());

    let attempted = to_fetch.len();
    let mut fetched_ok = 0usize;
    let mut updates: HashMap<String, PriceExtremes> = HashMap::new();
    if !to_fetch.is_empty() {
        let fetched: Vec<(String, Option<PriceExtremes>)> = stream::iter(to_fetch)
            .map(|sym| async move {
                let res = fetch_single_extremes(&sym).await;
                (sym, res)
            })
            .buffer_unordered(EXTREMES_CONCURRENCY)
            .collect()
            .await;

        for (sym, opt) in fetched {
            fetched_ok += usize::from(opt.is_some());
            if let Some(ext) = opt {
                if let Ok(mut fail_map) = extremes_fail().lock() {
                    fail_map.remove(&sym);
                }
                result_items.insert(sym.clone(), ext.clone());
                updates.insert(sym, ext);
            } else if let Ok(mut fail_map) = extremes_fail().lock() {
                fail_map.insert(sym, std::time::Instant::now());
            }
        }
    }

    (
        updates,
        ExtremesBatchResponse {
            items: result_items,
            remaining,
            attempted,
            fetched: fetched_ok,
        },
    )
}

// ---------------------------------------------------------------------------
// Company financials & historical market-cap snapshots (Yahoo fundamentals-timeseries)
// ---------------------------------------------------------------------------

const YAHOO_FINANCIAL_TYPES: &str = concat!(
    "annualTotalRevenue,annualGrossProfit,annualOperatingIncome,annualNetIncome,",
    "annualDilutedEPS,annualOperatingCashFlow,annualFreeCashFlow,",
    "quarterlyTotalRevenue,quarterlyGrossProfit,quarterlyOperatingIncome,quarterlyNetIncome,",
    "quarterlyDilutedEPS,quarterlyOperatingCashFlow,quarterlyFreeCashFlow,",
    "trailingMarketCap,monthlyMarketCap,quarterlyMarketCap,",
    "trailingPeRatio,trailingPsRatio,trailingPbRatio"
);

static FINANCIALS_MEM_CACHE: OnceLock<std::sync::Mutex<Option<HashMap<String, StockFinancials>>>> =
    OnceLock::new();
static FINANCIALS_FAIL_CACHE: OnceLock<std::sync::Mutex<HashMap<String, std::time::Instant>>> =
    OnceLock::new();

fn financials_mem() -> &'static std::sync::Mutex<Option<HashMap<String, StockFinancials>>> {
    FINANCIALS_MEM_CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

fn financials_fail() -> &'static std::sync::Mutex<HashMap<String, std::time::Instant>> {
    FINANCIALS_FAIL_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn is_non_equity_symbol(symbol: &str) -> bool {
    let upper = symbol.trim().to_ascii_uppercase();
    upper.is_empty()
        || upper.starts_with('^')
        || upper.contains('=')
        || upper.ends_with("-USD")
        || matches!(
            upper.as_str(),
            "000001.SS"
                | "000300.SS"
                | "000016.SS"
                | "000905.SS"
                | "000688.SS"
                | "399001.SZ"
                | "399006.SZ"
        )
}

pub fn load_financials_cache(db: &Database) -> Result<HashMap<String, StockFinancials>, AppError> {
    if let Ok(guard) = financials_mem().lock() {
        if let Some(ref map) = *guard {
            return Ok(map.clone());
        }
    }
    let loaded: HashMap<String, StockFinancials> = db
        .get_setting(FINANCIALS_CACHE_KEY)?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    if let Ok(mut guard) = financials_mem().lock() {
        *guard = Some(loaded.clone());
    }
    Ok(loaded)
}

pub fn save_financials_entry(
    db: &Database,
    symbol: &str,
    entry: StockFinancials,
) -> Result<(), AppError> {
    let mut map = load_financials_cache(db)?;
    let key = symbol.trim().to_ascii_uppercase();
    map.insert(key, entry);
    if map.len() > FINANCIALS_MAX_CACHE_ENTRIES {
        let mut by_age: Vec<(String, i64)> =
            map.iter().map(|(k, v)| (k.clone(), v.fetched_at)).collect();
        by_age.sort_by_key(|(_, ts)| *ts);
        let drop_count = map.len() - FINANCIALS_MAX_CACHE_ENTRIES;
        for (old_key, _) in by_age.into_iter().take(drop_count) {
            map.remove(&old_key);
        }
    }
    if let Ok(mut guard) = financials_mem().lock() {
        *guard = Some(map.clone());
    }
    let raw = serde_json::to_string(&map)
        .map_err(|e| AppError::Message(format!("序列化财报缓存失败: {e}")))?;
    db.set_setting_quietly(FINANCIALS_CACHE_KEY, &raw)
}

fn empty_financials(symbol: &str) -> StockFinancials {
    let market = detect_market(symbol);
    StockFinancials {
        symbol: symbol.trim().to_string(),
        currency: default_currency(market).to_string(),
        market_cap_currency: None,
        latest_market_cap: None,
        latest_market_cap_date: None,
        peak_market_cap: None,
        peak_market_cap_date: None,
        trailing_pe: None,
        trailing_ps: None,
        trailing_pb: None,
        annual: Vec::new(),
        quarterly: Vec::new(),
        source: "—".into(),
        fetched_at: chrono::Utc::now().timestamp(),
    }
}

fn parse_yahoo_financials(symbol: &str, bytes: &[u8]) -> Result<StockFinancials, String> {
    use std::collections::BTreeMap;

    let body: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("财报 JSON 解析失败: {e}"))?;
    if !body["timeseries"]["error"].is_null() {
        return Err(format!(
            "Yahoo 财报接口返回错误: {}",
            body["timeseries"]["error"]
        ));
    }
    let results = body["timeseries"]["result"]
        .as_array()
        .ok_or_else(|| "Yahoo 财报响应缺少 result".to_string())?;

    let mut annual_map: BTreeMap<String, FinancialPeriod> = BTreeMap::new();
    let mut quarterly_map: BTreeMap<String, FinancialPeriod> = BTreeMap::new();
    let mut stmt_currency: Option<String> = None;

    let mut cap_points: Vec<(String, f64, Option<String>)> = Vec::new();
    let mut trailing_pe: Option<(String, f64)> = None;
    let mut trailing_ps: Option<(String, f64)> = None;
    let mut trailing_pb: Option<(String, f64)> = None;

    for series in results {
        let Some(metric) = series["meta"]["type"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(entries) = series.get(metric).and_then(Value::as_array) else {
            continue;
        };

        for item in entries {
            if item.is_null() {
                continue;
            }
            let Some(date) = item["asOfDate"]
                .as_str()
                .map(str::trim)
                .filter(|d| d.len() >= 10)
            else {
                continue;
            };
            let Some(val) = item["reportedValue"]["raw"]
                .as_f64()
                .filter(|v| v.is_finite())
            else {
                continue;
            };
            let cur = item["currencyCode"]
                .as_str()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(str::to_ascii_uppercase);

            match metric {
                "trailingMarketCap" | "monthlyMarketCap" | "quarterlyMarketCap" => {
                    if val > 0.0 {
                        cap_points.push((date.to_string(), val, cur));
                    }
                }
                "trailingPeRatio" => {
                    if val > 0.0 && trailing_pe.as_ref().is_none_or(|(d, _)| date >= d.as_str()) {
                        trailing_pe = Some((date.to_string(), val));
                    }
                }
                "trailingPsRatio" => {
                    if val > 0.0 && trailing_ps.as_ref().is_none_or(|(d, _)| date >= d.as_str()) {
                        trailing_ps = Some((date.to_string(), val));
                    }
                }
                "trailingPbRatio" => {
                    if val > 0.0 && trailing_pb.as_ref().is_none_or(|(d, _)| date >= d.as_str()) {
                        trailing_pb = Some((date.to_string(), val));
                    }
                }
                _ => {
                    let (target_map, field) = if let Some(rest) = metric.strip_prefix("annual") {
                        (&mut annual_map, rest)
                    } else if let Some(rest) = metric.strip_prefix("quarterly") {
                        (&mut quarterly_map, rest)
                    } else {
                        continue;
                    };
                    if let Some(code) = cur {
                        if stmt_currency.is_none() || field == "TotalRevenue" {
                            stmt_currency = Some(code);
                        }
                    }
                    let row =
                        target_map
                            .entry(date.to_string())
                            .or_insert_with(|| FinancialPeriod {
                                as_of_date: date.to_string(),
                                ..Default::default()
                            });
                    match field {
                        "TotalRevenue" => row.revenue = Some(val),
                        "GrossProfit" => row.gross_profit = Some(val),
                        "OperatingIncome" => row.operating_income = Some(val),
                        "NetIncome" => row.net_income = Some(val),
                        "DilutedEPS" => row.diluted_eps = Some(val),
                        "OperatingCashFlow" => row.operating_cash_flow = Some(val),
                        "FreeCashFlow" => row.free_cash_flow = Some(val),
                        _ => {}
                    }
                }
            }
        }
    }

    let finalize_periods =
        |map: BTreeMap<String, FinancialPeriod>, limit: usize| -> Vec<FinancialPeriod> {
            let mut rows: Vec<FinancialPeriod> =
                map.into_values().filter(has_any_financial_value).collect();
            if rows.len() > limit {
                rows.drain(0..(rows.len() - limit));
            }
            rows
        };

    let market = detect_market(symbol);
    let default_cur = default_currency(market).to_string();

    // Pick market-cap currency from the latest snapshot that carries one, falling back to trading currency.
    cap_points.sort_by(|a, b| a.0.cmp(&b.0));
    let market_cap_currency = cap_points
        .iter()
        .rev()
        .find_map(|(_, _, c)| c.clone())
        .or_else(|| (!cap_points.is_empty()).then(|| default_cur.clone()));

    let mut latest_market_cap: Option<f64> = None;
    let mut latest_market_cap_date: Option<String> = None;
    let mut peak_market_cap: Option<f64> = None;
    let mut peak_market_cap_date: Option<String> = None;
    for (date, val, cur) in &cap_points {
        if let (Some(c), Some(expected)) = (cur.as_deref(), market_cap_currency.as_deref()) {
            if c != expected {
                continue;
            }
        }
        if latest_market_cap_date
            .as_deref()
            .is_none_or(|d| date.as_str() >= d)
        {
            latest_market_cap = Some(*val);
            latest_market_cap_date = Some(date.clone());
        }
        if peak_market_cap.is_none_or(|best| *val > best) {
            peak_market_cap = Some(*val);
            peak_market_cap_date = Some(date.clone());
        }
    }

    Ok(StockFinancials {
        symbol: symbol.trim().to_string(),
        currency: stmt_currency.unwrap_or(default_cur),
        market_cap_currency,
        latest_market_cap,
        latest_market_cap_date,
        peak_market_cap,
        peak_market_cap_date,
        trailing_pe: trailing_pe.map(|(_, v)| v),
        trailing_ps: trailing_ps.map(|(_, v)| v),
        trailing_pb: trailing_pb.map(|(_, v)| v),
        annual: finalize_periods(annual_map, FINANCIALS_MAX_ANNUAL),
        quarterly: finalize_periods(quarterly_map, FINANCIALS_MAX_QUARTERLY),
        source: "Yahoo Finance".into(),
        fetched_at: chrono::Utc::now().timestamp(),
    })
}

fn has_any_financial_value(r: &FinancialPeriod) -> bool {
    r.revenue.is_some()
        || r.net_income.is_some()
        || r.operating_income.is_some()
        || r.diluted_eps.is_some()
        || r.operating_cash_flow.is_some()
        || r.free_cash_flow.is_some()
}

fn trim_periods(mut rows: Vec<FinancialPeriod>, limit: usize) -> Vec<FinancialPeriod> {
    rows.retain(has_any_financial_value);
    rows.sort_by(|a, b| a.as_of_date.cmp(&b.as_of_date));
    if rows.len() > limit {
        rows.drain(0..(rows.len() - limit));
    }
    rows
}

fn parse_iso_date(s: &str) -> Option<NaiveDate> {
    let head = s.trim().get(..10)?;
    NaiveDate::parse_from_str(head, "%Y-%m-%d").ok()
}

// ---------------------------------------------------------------------------
// Long-history source 1: SEC EDGAR XBRL CompanyFacts (US & ADRs, up to 20 years)
// ---------------------------------------------------------------------------

type SecCikMap = HashMap<String, u64>;
type SecCikCache = tokio::sync::Mutex<Option<(std::time::Instant, SecCikMap)>>;

static SEC_CIK_CACHE: OnceLock<SecCikCache> = OnceLock::new();

fn sec_cik_cache() -> &'static SecCikCache {
    SEC_CIK_CACHE.get_or_init(|| tokio::sync::Mutex::new(None))
}

fn parse_sec_tickers(bytes: &[u8]) -> Option<HashMap<String, u64>> {
    let body: Value = serde_json::from_slice(bytes).ok()?;
    let obj = body.as_object()?;
    let mut map = HashMap::with_capacity(obj.len());
    for entry in obj.values() {
        let Some(ticker) = entry["ticker"].as_str().map(str::trim) else {
            continue;
        };
        let Some(cik) = entry["cik_str"].as_u64().or_else(|| {
            entry["cik_str"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
        }) else {
            continue;
        };
        if !ticker.is_empty() && cik > 0 {
            map.insert(ticker.to_ascii_uppercase(), cik);
        }
    }
    (!map.is_empty()).then_some(map)
}

async fn lookup_sec_cik(symbol: &str) -> Option<u64> {
    let key = symbol.trim().to_ascii_uppercase();
    let alt_key = key.replace('.', "-");
    let ttl = Duration::from_secs(24 * 3600);
    {
        let guard = sec_cik_cache().lock().await;
        if let Some((at, ref map)) = *guard {
            if at.elapsed() < ttl {
                return map.get(&key).or_else(|| map.get(&alt_key)).copied();
            }
        }
    }
    let bytes = traced_get_bytes_opts(
        "SEC EDGAR 代码表",
        "https://www.sec.gov/files/company_tickers.json",
        None,
        Some(SEC_USER_AGENT),
        Some(15),
    )
    .await
    .ok()?;
    let map = parse_sec_tickers(&bytes)?;
    let found = map.get(&key).or_else(|| map.get(&alt_key)).copied();
    let mut guard = sec_cik_cache().lock().await;
    *guard = Some((std::time::Instant::now(), map));
    found
}

#[derive(Clone)]
struct SecFact {
    start: String,
    end: String,
    days: i64,
    val: f64,
    filed: String,
    priority: usize,
}

fn collect_sec_metric(
    taxonomy: &serde_json::Map<String, Value>,
    tags: &[&str],
    unit_key: &str,
) -> (HashMap<String, f64>, HashMap<String, f64>) {
    // Keyed by (start, end) so restatements/comparatives resolve to latest filed & highest tag priority.
    let mut best: HashMap<(String, String), SecFact> = HashMap::new();
    for (priority, tag) in tags.iter().enumerate() {
        let Some(units) = taxonomy.get(*tag).and_then(|t| t["units"].as_object()) else {
            continue;
        };
        let Some(entries) = units.get(unit_key).and_then(Value::as_array) else {
            continue;
        };
        for item in entries {
            let Some(form) = item["form"].as_str() else {
                continue;
            };
            if !matches!(
                form,
                "10-K" | "10-K/A" | "10-Q" | "10-Q/A" | "20-F" | "20-F/A" | "40-F" | "6-K"
            ) {
                continue;
            }
            let (Some(start_raw), Some(end_raw)) = (item["start"].as_str(), item["end"].as_str())
            else {
                continue;
            };
            let (Some(sd), Some(ed)) = (parse_iso_date(start_raw), parse_iso_date(end_raw)) else {
                continue;
            };
            let days = (ed - sd).num_days();
            if !(65..=395).contains(&days) {
                continue;
            }
            let Some(val) = item["val"].as_f64().filter(|v| v.is_finite()) else {
                continue;
            };
            let start = start_raw[..10].to_string();
            let end = end_raw[..10].to_string();
            let filed = item["filed"].as_str().unwrap_or("").to_string();
            let key = (start.clone(), end.clone());
            let replace = match best.get(&key) {
                None => true,
                Some(prev) => {
                    priority < prev.priority || (priority == prev.priority && filed >= prev.filed)
                }
            };
            if replace {
                best.insert(
                    key,
                    SecFact {
                        start,
                        end,
                        days,
                        val,
                        filed,
                        priority,
                    },
                );
            }
        }
    }

    // Group by start date to classify annual (12M), standalone quarter (3M), and YTD (6M/9M) for Q2/Q3/Q4 derivation.
    let mut annual_by_end: HashMap<String, (String, f64)> = HashMap::new();
    let mut quarter_by_end: HashMap<String, (String, f64)> = HashMap::new();
    let mut by_start: HashMap<String, Vec<SecFact>> = HashMap::new();

    for fact in best.into_values() {
        if (330..=395).contains(&fact.days) {
            let take = annual_by_end
                .get(&fact.end)
                .is_none_or(|(f, _)| fact.filed >= *f);
            if take {
                annual_by_end.insert(fact.end.clone(), (fact.filed.clone(), fact.val));
            }
        } else if (70..=115).contains(&fact.days) {
            let take = quarter_by_end
                .get(&fact.end)
                .is_none_or(|(f, _)| fact.filed >= *f);
            if take {
                quarter_by_end.insert(fact.end.clone(), (fact.filed.clone(), fact.val));
            }
        }
        by_start.entry(fact.start.clone()).or_default().push(fact);
    }

    // Derive missing standalone quarters (Q2 = 6M - 3M, Q3 = 9M - 6M, Q4 = 12M - 9M) sharing the same fiscal start date.
    for facts in by_start.values() {
        let pick_bucket = |lo: i64, hi: i64| -> Option<&SecFact> {
            facts
                .iter()
                .filter(|f| (lo..=hi).contains(&f.days))
                .max_by(|a, b| {
                    b.priority
                        .cmp(&a.priority)
                        .then_with(|| a.filed.cmp(&b.filed))
                })
        };
        let q1 = pick_bucket(70, 115);
        let h1 = pick_bucket(155, 205);
        let m9 = pick_bucket(245, 295);
        let fy = pick_bucket(330, 395);

        if let (Some(h1), Some(q1)) = (h1, q1) {
            quarter_by_end
                .entry(h1.end.clone())
                .or_insert_with(|| (h1.filed.clone(), h1.val - q1.val));
        }
        if let (Some(m9), Some(h1)) = (m9, h1) {
            quarter_by_end
                .entry(m9.end.clone())
                .or_insert_with(|| (m9.filed.clone(), m9.val - h1.val));
        }
        if let (Some(fy), Some(m9)) = (fy, m9) {
            quarter_by_end
                .entry(fy.end.clone())
                .or_insert_with(|| (fy.filed.clone(), fy.val - m9.val));
        }
    }

    (
        annual_by_end
            .into_iter()
            .map(|(k, (_, v))| (k, v))
            .collect(),
        quarter_by_end
            .into_iter()
            .map(|(k, (_, v))| (k, v))
            .collect(),
    )
}

fn parse_sec_companyfacts(symbol: &str, bytes: &[u8]) -> Result<StockFinancials, String> {
    use std::collections::BTreeMap;

    let body: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("SEC 财报 JSON 解析失败: {e}"))?;
    let facts = body["facts"]
        .as_object()
        .ok_or_else(|| "SEC 财报缺少 facts".to_string())?;

    let (taxonomy, is_ifrs) = if let Some(us) = facts.get("us-gaap").and_then(Value::as_object) {
        (us, false)
    } else if let Some(ifrs) = facts.get("ifrs-full").and_then(Value::as_object) {
        (ifrs, true)
    } else {
        return Err("SEC 财报不含 us-gaap 或 ifrs-full 数据".into());
    };

    struct SecTagSet {
        rev: &'static [&'static str],
        gp: &'static [&'static str],
        op: &'static [&'static str],
        ni: &'static [&'static str],
        eps: &'static [&'static str],
        ocf: &'static [&'static str],
        capex: &'static [&'static str],
    }

    let tags = if is_ifrs {
        SecTagSet {
            rev: &["Revenue", "RevenueFromContractsWithCustomers"],
            gp: &["GrossProfit"],
            op: &["ProfitLossFromOperatingActivities"],
            ni: &["ProfitLossAttributableToOwnersOfParent", "ProfitLoss"],
            eps: &["DilutedEarningsLossPerShare", "BasicEarningsLossPerShare"],
            ocf: &["CashFlowsFromUsedInOperatingActivities"],
            capex: &["PurchaseOfPropertyPlantAndEquipmentClassifiedAsInvestingActivities"],
        }
    } else {
        SecTagSet {
            rev: &[
                "RevenueFromContractWithCustomerExcludingAssessedTax",
                "RevenueFromContractWithCustomerIncludingAssessedTax",
                "Revenues",
                "SalesRevenueNet",
                "SalesRevenueGoodsNet",
                "SalesRevenueServicesNet",
                "RevenuesNetOfInterestExpense",
                "InterestAndDividendIncomeOperating",
            ],
            gp: &["GrossProfit"],
            op: &["OperatingIncomeLoss"],
            ni: &["NetIncomeLoss", "ProfitLoss"],
            eps: &["EarningsPerShareDiluted", "EarningsPerShareBasic"],
            ocf: &[
                "NetCashProvidedByUsedInOperatingActivities",
                "NetCashProvidedByUsedInOperatingActivitiesContinuingOperations",
            ],
            capex: &[
                "PaymentsToAcquirePropertyPlantAndEquipment",
                "PaymentsToAcquireProductiveAssets",
            ],
        }
    };

    // Detect primary monetary unit (prefer USD when reported in USD, else the company's first reporting currency).
    let currency = tags
        .rev
        .iter()
        .chain(tags.ni.iter())
        .find_map(|tag| {
            let units = taxonomy.get(*tag)?.get("units")?.as_object()?;
            if units.contains_key("USD") {
                Some("USD".to_string())
            } else {
                units
                    .keys()
                    .find(|k| k.len() == 3 && k.chars().all(|c| c.is_ascii_uppercase()))
                    .cloned()
            }
        })
        .unwrap_or_else(|| "USD".to_string());
    let eps_unit = format!("{currency}/shares");

    let (ann_rev, qtr_rev) = collect_sec_metric(taxonomy, tags.rev, &currency);
    let (ann_gp, qtr_gp) = collect_sec_metric(taxonomy, tags.gp, &currency);
    let (ann_op, qtr_op) = collect_sec_metric(taxonomy, tags.op, &currency);
    let (ann_ni, qtr_ni) = collect_sec_metric(taxonomy, tags.ni, &currency);
    let (ann_eps, qtr_eps) = collect_sec_metric(taxonomy, tags.eps, &eps_unit);
    let (ann_ocf, qtr_ocf) = collect_sec_metric(taxonomy, tags.ocf, &currency);
    let (ann_capex, qtr_capex) = collect_sec_metric(taxonomy, tags.capex, &currency);

    let assemble = |rev: HashMap<String, f64>,
                    gp: HashMap<String, f64>,
                    op: HashMap<String, f64>,
                    ni: HashMap<String, f64>,
                    eps: HashMap<String, f64>,
                    ocf: HashMap<String, f64>,
                    capex: HashMap<String, f64>,
                    limit: usize|
     -> Vec<FinancialPeriod> {
        let mut map: BTreeMap<String, FinancialPeriod> = BTreeMap::new();
        let mut capex_map: HashMap<String, f64> = HashMap::new();
        for (d, v) in rev {
            map.entry(d.clone())
                .or_insert_with(|| FinancialPeriod {
                    as_of_date: d,
                    ..Default::default()
                })
                .revenue = Some(v);
        }
        for (d, v) in gp {
            map.entry(d.clone())
                .or_insert_with(|| FinancialPeriod {
                    as_of_date: d,
                    ..Default::default()
                })
                .gross_profit = Some(v);
        }
        for (d, v) in op {
            map.entry(d.clone())
                .or_insert_with(|| FinancialPeriod {
                    as_of_date: d,
                    ..Default::default()
                })
                .operating_income = Some(v);
        }
        for (d, v) in ni {
            map.entry(d.clone())
                .or_insert_with(|| FinancialPeriod {
                    as_of_date: d,
                    ..Default::default()
                })
                .net_income = Some(v);
        }
        for (d, v) in eps {
            map.entry(d.clone())
                .or_insert_with(|| FinancialPeriod {
                    as_of_date: d,
                    ..Default::default()
                })
                .diluted_eps = Some(v);
        }
        for (d, v) in ocf {
            map.entry(d.clone())
                .or_insert_with(|| FinancialPeriod {
                    as_of_date: d,
                    ..Default::default()
                })
                .operating_cash_flow = Some(v);
        }
        for (d, v) in capex {
            capex_map.insert(d, v);
        }
        for (d, row) in &mut map {
            if let (Some(o), Some(c)) = (row.operating_cash_flow, capex_map.get(d)) {
                row.free_cash_flow = Some(o - c.abs());
            }
        }
        trim_periods(map.into_values().collect(), limit)
    };

    Ok(StockFinancials {
        symbol: symbol.trim().to_string(),
        currency,
        market_cap_currency: None,
        latest_market_cap: None,
        latest_market_cap_date: None,
        peak_market_cap: None,
        peak_market_cap_date: None,
        trailing_pe: None,
        trailing_ps: None,
        trailing_pb: None,
        annual: assemble(
            ann_rev,
            ann_gp,
            ann_op,
            ann_ni,
            ann_eps,
            ann_ocf,
            ann_capex,
            FINANCIALS_MAX_ANNUAL,
        ),
        quarterly: assemble(
            qtr_rev,
            qtr_gp,
            qtr_op,
            qtr_ni,
            qtr_eps,
            qtr_ocf,
            qtr_capex,
            FINANCIALS_MAX_QUARTERLY,
        ),
        source: "SEC EDGAR".into(),
        fetched_at: chrono::Utc::now().timestamp(),
    })
}

async fn fetch_sec_financials(symbol: &str) -> Option<StockFinancials> {
    let cik = lookup_sec_cik(symbol).await?;
    let url = format!("https://data.sec.gov/api/xbrl/companyfacts/CIK{cik:010}.json");
    let bytes = traced_get_bytes_opts("SEC EDGAR 财报", &url, None, Some(SEC_USER_AGENT), Some(25))
        .await
        .ok()?;
    let parsed = parse_sec_companyfacts(symbol, &bytes).ok()?;
    (!parsed.annual.is_empty() || !parsed.quarterly.is_empty()).then_some(parsed)
}

// ---------------------------------------------------------------------------
// Long-history source 2: Eastmoney DataCenter (CN A-shares & HK, 15-20 years)
// ---------------------------------------------------------------------------

fn sub_opt(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x - y),
        _ => None,
    }
}

fn to_eastmoney_cn_secucode(symbol: &str) -> Option<String> {
    let upper = symbol.trim().to_ascii_uppercase();
    if let Some(code) = upper.strip_suffix(".SS") {
        Some(format!("{code}.SH"))
    } else if upper.ends_with(".SZ") || upper.ends_with(".BJ") {
        Some(upper)
    } else if upper.starts_with("SH") && upper.len() == 8 {
        Some(format!("{}.SH", &upper[2..]))
    } else if upper.starts_with("SZ") && upper.len() == 8 {
        Some(format!("{}.SZ", &upper[2..]))
    } else {
        None
    }
}

fn to_eastmoney_hk_secucode(symbol: &str) -> Option<String> {
    let upper = symbol.trim().to_ascii_uppercase();
    let code = upper.strip_suffix(".HK")?;
    let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    Some(format!("{digits:0>5}.HK"))
}

fn parse_eastmoney_cn_financials(symbol: &str, bytes: &[u8]) -> Result<StockFinancials, String> {
    use std::collections::BTreeMap;

    let body: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("东财 A 股财报 JSON 解析失败: {e}"))?;
    let rows = body["result"]["data"]
        .as_array()
        .ok_or_else(|| "东财 A 股财报响应缺少 data".to_string())?;

    let mut annual_map: BTreeMap<String, FinancialPeriod> = BTreeMap::new();
    // Group YTD rows by calendar year and quarter index (1..=4) to derive exact standalone quarters.
    let mut ytd_by_year: BTreeMap<String, [Option<FinancialPeriod>; 4]> = BTreeMap::new();

    for item in rows {
        let Some(date_raw) = item["REPORT_DATE"].as_str() else {
            continue;
        };
        let Some(date) = date_raw.trim().get(..10) else {
            continue;
        };
        let rtype = item["REPORT_TYPE"].as_str().unwrap_or("");
        let month = date.get(5..7).unwrap_or("");
        let q_idx: Option<usize> = match (rtype, month) {
            ("一季报", _) | (_, "03") => Some(0),
            ("中报", _) | (_, "06") => Some(1),
            ("三季报", _) | (_, "09") => Some(2),
            ("年报", _) | (_, "12") => Some(3),
            _ => None,
        };
        let revenue = item["TOTALOPERATEREVE"].as_f64().filter(|v| v.is_finite());
        let net_income = item["PARENTNETPROFIT"].as_f64().filter(|v| v.is_finite());
        let gross_margin = item["XSMLL"].as_f64().filter(|v| v.is_finite());
        let gross_profit = match (revenue, gross_margin) {
            (Some(r), Some(m)) if r > 0.0 => Some(r * m / 100.0),
            _ => None,
        };
        let diluted_eps = item["EPSXS"]
            .as_f64()
            .or_else(|| item["EPSJB"].as_f64())
            .filter(|v| v.is_finite());
        let ocf = match (
            item["MGJYXJJE"].as_f64().filter(|v| v.is_finite()),
            item["TOTAL_SHARE"]
                .as_f64()
                .filter(|v| v.is_finite() && *v > 0.0),
        ) {
            (Some(per_share), Some(shares)) => Some(per_share * shares),
            _ => None,
        };

        let period = FinancialPeriod {
            as_of_date: date.to_string(),
            revenue,
            gross_profit,
            operating_income: None,
            net_income,
            diluted_eps,
            operating_cash_flow: ocf,
            free_cash_flow: None,
        };
        if !has_any_financial_value(&period) {
            continue;
        }
        if q_idx == Some(3) {
            annual_map
                .entry(date.to_string())
                .or_insert_with(|| period.clone());
        }
        if let Some(idx) = q_idx {
            let year = date[..4].to_string();
            let slots = ytd_by_year.entry(year).or_default();
            if slots[idx].is_none() {
                slots[idx] = Some(period);
            }
        }
    }

    let mut quarterly_rows: Vec<FinancialPeriod> = Vec::new();
    for slots in ytd_by_year.values() {
        if let Some(ref q1) = slots[0] {
            quarterly_rows.push(q1.clone());
        }
        for idx in 1..4 {
            if let (Some(cur), Some(prev)) = (&slots[idx], &slots[idx - 1]) {
                quarterly_rows.push(FinancialPeriod {
                    as_of_date: cur.as_of_date.clone(),
                    revenue: sub_opt(cur.revenue, prev.revenue),
                    gross_profit: sub_opt(cur.gross_profit, prev.gross_profit),
                    operating_income: None,
                    net_income: sub_opt(cur.net_income, prev.net_income),
                    diluted_eps: sub_opt(cur.diluted_eps, prev.diluted_eps),
                    operating_cash_flow: sub_opt(cur.operating_cash_flow, prev.operating_cash_flow),
                    free_cash_flow: None,
                });
            }
        }
    }

    Ok(StockFinancials {
        symbol: symbol.trim().to_string(),
        currency: "CNY".into(),
        market_cap_currency: None,
        latest_market_cap: None,
        latest_market_cap_date: None,
        peak_market_cap: None,
        peak_market_cap_date: None,
        trailing_pe: None,
        trailing_ps: None,
        trailing_pb: None,
        annual: trim_periods(annual_map.into_values().collect(), FINANCIALS_MAX_ANNUAL),
        quarterly: trim_periods(quarterly_rows, FINANCIALS_MAX_QUARTERLY),
        source: "东方财富".into(),
        fetched_at: chrono::Utc::now().timestamp(),
    })
}

async fn fetch_eastmoney_cn_financials(symbol: &str) -> Option<StockFinancials> {
    let secucode = to_eastmoney_cn_secucode(symbol)?;
    let filter: String =
        url::form_urlencoded::byte_serialize(format!("(SECUCODE=\"{secucode}\")").as_bytes())
            .collect();
    let url = format!(
        "https://datacenter.eastmoney.com/securities/api/data/v1/get?reportName=RPT_F10_FINANCE_MAINFINADATA&columns=REPORT_DATE,REPORT_TYPE,TOTALOPERATEREVE,PARENTNETPROFIT,XSMLL,XSJLL,EPSJB,EPSXS,MGJYXJJE,TOTAL_SHARE&filter={filter}&pageNumber=1&pageSize=100&sortColumns=REPORT_DATE&sortTypes=-1"
    );
    let bytes = traced_get_bytes(
        "东方财富 A 股财报",
        &url,
        Some("https://emweb.securities.eastmoney.com/"),
    )
    .await
    .ok()?;
    let parsed = parse_eastmoney_cn_financials(symbol, &bytes).ok()?;
    (!parsed.annual.is_empty() || !parsed.quarterly.is_empty()).then_some(parsed)
}

fn parse_eastmoney_hk_financials(symbol: &str, bytes: &[u8]) -> Result<StockFinancials, String> {
    use std::collections::BTreeMap;

    let body: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("东财港股财报 JSON 解析失败: {e}"))?;
    let rows = body["result"]["data"]
        .as_array()
        .ok_or_else(|| "东财港股财报响应缺少 data".to_string())?;

    let mut annual_map: BTreeMap<String, FinancialPeriod> = BTreeMap::new();
    let mut ytd_by_fy: BTreeMap<String, [Option<FinancialPeriod>; 4]> = BTreeMap::new();
    let mut currency = "HKD".to_string();
    let mut latest_pe: Option<f64> = None;
    let mut latest_pb: Option<f64> = None;
    let mut latest_cap: Option<(String, f64)> = None;
    let mut peak_cap: Option<(String, f64)> = None;

    for item in rows {
        let Some(date) = item["REPORT_DATE"]
            .as_str()
            .and_then(|s| s.trim().get(..10))
        else {
            continue;
        };
        if item["IS_CNY_CODE"].as_i64() == Some(1) {
            currency = "CNY".into();
        } else if let Some(cur) = item["CURRENCY"]
            .as_str()
            .map(str::trim)
            .filter(|c| c.len() == 3)
        {
            if currency == "HKD" {
                currency = cur.to_ascii_uppercase();
            }
        }
        if latest_pe.is_none() {
            latest_pe = item["PE_TTM"]
                .as_f64()
                .filter(|v| v.is_finite() && *v > 0.0);
        }
        if latest_pb.is_none() {
            latest_pb = item["PB_TTM"]
                .as_f64()
                .filter(|v| v.is_finite() && *v > 0.0);
        }
        if let Some(mc) = item["TOTAL_MARKET_CAP"]
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0.0)
        {
            if latest_cap.as_ref().is_none_or(|(d, _)| date >= d.as_str()) {
                latest_cap = Some((date.to_string(), mc));
            }
            if peak_cap.as_ref().is_none_or(|(_, best)| mc > *best) {
                peak_cap = Some((date.to_string(), mc));
            }
        }

        let code = item["DATE_TYPE_CODE"].as_str().unwrap_or("");
        // 003 = Q1 (3M), 002 = H1 (6M), 004 = 9M, 001 = FY (12M)
        let q_idx: Option<usize> = match code {
            "003" => Some(0),
            "002" => Some(1),
            "004" => Some(2),
            "001" => Some(3),
            _ => None,
        };
        let period = FinancialPeriod {
            as_of_date: date.to_string(),
            revenue: item["OPERATE_INCOME"].as_f64().filter(|v| v.is_finite()),
            gross_profit: item["GROSS_PROFIT"].as_f64().filter(|v| v.is_finite()),
            operating_income: item["OPERATE_PROFIT"].as_f64().filter(|v| v.is_finite()),
            net_income: item["HOLDER_PROFIT"].as_f64().filter(|v| v.is_finite()),
            diluted_eps: item["DILUTED_EPS"]
                .as_f64()
                .or_else(|| item["BASIC_EPS"].as_f64())
                .filter(|v| v.is_finite()),
            operating_cash_flow: item["NETCASH_OPERATE"].as_f64().filter(|v| v.is_finite()),
            free_cash_flow: None,
        };
        if !has_any_financial_value(&period) {
            continue;
        }
        if q_idx == Some(3) {
            annual_map
                .entry(date.to_string())
                .or_insert_with(|| period.clone());
        }
        if let Some(idx) = q_idx {
            let fy_key = item["START_DATE"]
                .as_str()
                .and_then(|s| s.trim().get(..10))
                .map(str::to_string)
                .or_else(|| {
                    item["FISCAL_YEAR"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| date[..4].to_string());
            let slots = ytd_by_fy.entry(fy_key).or_default();
            if slots[idx].is_none() {
                slots[idx] = Some(period);
            }
        }
    }

    let mut quarterly_rows: Vec<FinancialPeriod> = Vec::new();
    for slots in ytd_by_fy.values() {
        if let Some(ref q1) = slots[0] {
            quarterly_rows.push(q1.clone());
        }
        for idx in 1..4 {
            if let (Some(cur), Some(prev)) = (&slots[idx], &slots[idx - 1]) {
                quarterly_rows.push(FinancialPeriod {
                    as_of_date: cur.as_of_date.clone(),
                    revenue: sub_opt(cur.revenue, prev.revenue),
                    gross_profit: sub_opt(cur.gross_profit, prev.gross_profit),
                    operating_income: sub_opt(cur.operating_income, prev.operating_income),
                    net_income: sub_opt(cur.net_income, prev.net_income),
                    diluted_eps: sub_opt(cur.diluted_eps, prev.diluted_eps),
                    operating_cash_flow: sub_opt(cur.operating_cash_flow, prev.operating_cash_flow),
                    free_cash_flow: None,
                });
            }
        }
    }

    Ok(StockFinancials {
        symbol: symbol.trim().to_string(),
        currency,
        market_cap_currency: latest_cap.as_ref().map(|_| "HKD".to_string()),
        latest_market_cap: latest_cap.as_ref().map(|(_, v)| *v),
        latest_market_cap_date: latest_cap.map(|(d, _)| d),
        peak_market_cap: peak_cap.as_ref().map(|(_, v)| *v),
        peak_market_cap_date: peak_cap.map(|(d, _)| d),
        trailing_pe: latest_pe,
        trailing_ps: None,
        trailing_pb: latest_pb,
        annual: trim_periods(annual_map.into_values().collect(), FINANCIALS_MAX_ANNUAL),
        quarterly: trim_periods(quarterly_rows, FINANCIALS_MAX_QUARTERLY),
        source: "东方财富".into(),
        fetched_at: chrono::Utc::now().timestamp(),
    })
}

async fn fetch_eastmoney_hk_financials(symbol: &str) -> Option<StockFinancials> {
    let secucode = to_eastmoney_hk_secucode(symbol)?;
    let filter: String =
        url::form_urlencoded::byte_serialize(format!("(SECUCODE=\"{secucode}\")").as_bytes())
            .collect();
    let url = format!(
        "https://datacenter.eastmoney.com/securities/api/data/v1/get?reportName=RPT_HKF10_FN_MAININDICATOR&columns=REPORT_DATE,START_DATE,FISCAL_YEAR,DATE_TYPE_CODE,REPORT_TYPE,CURRENCY,IS_CNY_CODE,OPERATE_INCOME,GROSS_PROFIT,OPERATE_PROFIT,HOLDER_PROFIT,DILUTED_EPS,BASIC_EPS,NETCASH_OPERATE,TOTAL_MARKET_CAP,PE_TTM,PB_TTM&filter={filter}&pageNumber=1&pageSize=80&sortColumns=REPORT_DATE&sortTypes=-1"
    );
    let bytes = traced_get_bytes(
        "东方财富港股财报",
        &url,
        Some("https://emweb.securities.eastmoney.com/"),
    )
    .await
    .ok()?;
    let parsed = parse_eastmoney_hk_financials(symbol, &bytes).ok()?;
    (!parsed.annual.is_empty() || !parsed.quarterly.is_empty()).then_some(parsed)
}

// ---------------------------------------------------------------------------
// Long-history source 3: FinMind Open API (Taiwan stocks, 10+ years)
// ---------------------------------------------------------------------------

fn parse_finmind_tw_financials(symbol: &str, bytes: &[u8]) -> Result<StockFinancials, String> {
    use std::collections::BTreeMap;

    let body: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("FinMind 财报 JSON 解析失败: {e}"))?;
    if body["status"].as_i64() != Some(200) {
        return Err(format!("FinMind 财报返回异常状态: {}", body["msg"]));
    }
    let rows = body["data"]
        .as_array()
        .ok_or_else(|| "FinMind 财报缺少 data".to_string())?;

    let mut q_map: BTreeMap<String, FinancialPeriod> = BTreeMap::new();
    for item in rows {
        let Some(date) = item["date"].as_str().and_then(|s| s.trim().get(..10)) else {
            continue;
        };
        let Some(kind) = item["type"].as_str() else {
            continue;
        };
        let Some(val) = item["value"].as_f64().filter(|v| v.is_finite()) else {
            continue;
        };
        let row = q_map
            .entry(date.to_string())
            .or_insert_with(|| FinancialPeriod {
                as_of_date: date.to_string(),
                ..Default::default()
            });
        match kind {
            "Revenue" => row.revenue = Some(val),
            "GrossProfit" => row.gross_profit = Some(val),
            "OperatingIncome" => row.operating_income = Some(val),
            "IncomeAfterTaxes" => row.net_income = Some(val),
            "NetIncome" if row.net_income.is_none() => row.net_income = Some(val),
            "EPS" => row.diluted_eps = Some(val),
            _ => {}
        }
    }

    let quarterly_all: Vec<FinancialPeriod> = q_map
        .into_values()
        .filter(has_any_financial_value)
        .collect();

    // Sum complete 4-quarter calendar years into annual periods.
    let mut by_year: BTreeMap<String, Vec<&FinancialPeriod>> = BTreeMap::new();
    for q in &quarterly_all {
        if let Some(year) = q.as_of_date.get(..4) {
            by_year.entry(year.to_string()).or_default().push(q);
        }
    }
    let sum_field =
        |items: &[&FinancialPeriod], f: fn(&FinancialPeriod) -> Option<f64>| -> Option<f64> {
            let vals: Vec<f64> = items.iter().filter_map(|p| f(p)).collect();
            (vals.len() == items.len()).then(|| vals.into_iter().sum())
        };
    let mut annual_rows = Vec::new();
    for (year, qs) in by_year {
        if qs.len() == 4 {
            annual_rows.push(FinancialPeriod {
                as_of_date: format!("{year}-12-31"),
                revenue: sum_field(&qs, |p| p.revenue),
                gross_profit: sum_field(&qs, |p| p.gross_profit),
                operating_income: sum_field(&qs, |p| p.operating_income),
                net_income: sum_field(&qs, |p| p.net_income),
                diluted_eps: sum_field(&qs, |p| p.diluted_eps),
                operating_cash_flow: None,
                free_cash_flow: None,
            });
        }
    }

    Ok(StockFinancials {
        symbol: symbol.trim().to_string(),
        currency: "TWD".into(),
        market_cap_currency: None,
        latest_market_cap: None,
        latest_market_cap_date: None,
        peak_market_cap: None,
        peak_market_cap_date: None,
        trailing_pe: None,
        trailing_ps: None,
        trailing_pb: None,
        annual: trim_periods(annual_rows, FINANCIALS_MAX_ANNUAL),
        quarterly: trim_periods(quarterly_all, FINANCIALS_MAX_QUARTERLY),
        source: "FinMind".into(),
        fetched_at: chrono::Utc::now().timestamp(),
    })
}

async fn fetch_finmind_tw_financials(symbol: &str) -> Option<StockFinancials> {
    let upper = symbol.trim().to_ascii_uppercase();
    let code = upper
        .strip_suffix(".TW")
        .or_else(|| upper.strip_suffix(".TWO"))?;
    if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let url = format!(
        "https://api.finmindtrade.com/api/v4/data?dataset=TaiwanStockFinancialStatements&data_id={code}&start_date=2010-01-01"
    );
    let bytes = traced_get_bytes_opts("FinMind 台股财报", &url, None, None, Some(12))
        .await
        .ok()?;
    let parsed = parse_finmind_tw_financials(symbol, &bytes).ok()?;
    (!parsed.annual.is_empty() || !parsed.quarterly.is_empty()).then_some(parsed)
}

// ---------------------------------------------------------------------------
// Cross-source merger (deep history primary + Yahoo valuation/market-cap/recent fill)
// ---------------------------------------------------------------------------

fn merge_period_lists(
    mut primary: Vec<FinancialPeriod>,
    secondary: Vec<FinancialPeriod>,
    limit: usize,
) -> Vec<FinancialPeriod> {
    for sec in secondary {
        let sec_date = parse_iso_date(&sec.as_of_date);
        // Match fiscal dates within ±10 days (e.g. Apple's 52-week 2026-09-26 vs Yahoo's 2026-09-30).
        let matched = primary.iter_mut().find(|p| {
            if p.as_of_date == sec.as_of_date {
                return true;
            }
            match (parse_iso_date(&p.as_of_date), sec_date) {
                (Some(a), Some(b)) => (a - b).num_days().abs() <= 10,
                _ => false,
            }
        });
        if let Some(dst) = matched {
            if dst.revenue.is_none() {
                dst.revenue = sec.revenue;
            }
            if dst.gross_profit.is_none() {
                dst.gross_profit = sec.gross_profit;
            }
            if dst.operating_income.is_none() {
                dst.operating_income = sec.operating_income;
            }
            if dst.net_income.is_none() {
                dst.net_income = sec.net_income;
            }
            if dst.diluted_eps.is_none() {
                dst.diluted_eps = sec.diluted_eps;
            }
            if dst.operating_cash_flow.is_none() {
                dst.operating_cash_flow = sec.operating_cash_flow;
            }
            if dst.free_cash_flow.is_none() {
                dst.free_cash_flow = sec.free_cash_flow;
            }
        } else {
            primary.push(sec);
        }
    }
    trim_periods(primary, limit)
}

fn merge_financials(
    mut primary: StockFinancials,
    yahoo: StockFinancials,
    market: &str,
) -> StockFinancials {
    // For HK stocks, Eastmoney's CURRENCY field reflects trading currency (HKD) even when revenue is CNY.
    // Yahoo carries exact reporting currencyCode on every statement row, so adopt Yahoo's when Yahoo has rows.
    let yahoo_has_rows = !yahoo.annual.is_empty() || !yahoo.quarterly.is_empty();
    if market == "HK" && yahoo_has_rows && !yahoo.currency.is_empty() {
        primary.currency = yahoo.currency.clone();
    }

    let same_stmt_currency = primary.currency.eq_ignore_ascii_case(&yahoo.currency);
    if same_stmt_currency {
        primary.annual = merge_period_lists(primary.annual, yahoo.annual, FINANCIALS_MAX_ANNUAL);
        primary.quarterly =
            merge_period_lists(primary.quarterly, yahoo.quarterly, FINANCIALS_MAX_QUARTERLY);
    }

    // Prefer Yahoo's trading-currency market-cap snapshots & valuation ratios; keep higher peak cap if same currency.
    if yahoo.latest_market_cap.is_some() {
        let same_cap_cur = match (
            primary.market_cap_currency.as_deref(),
            yahoo.market_cap_currency.as_deref(),
        ) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => true,
        };
        let combined_peak = match (same_cap_cur, primary.peak_market_cap, yahoo.peak_market_cap) {
            (true, Some(p), Some(y)) if p > y => (Some(p), primary.peak_market_cap_date.clone()),
            (_, _, Some(y)) => (Some(y), yahoo.peak_market_cap_date.clone()),
            (true, Some(p), None) => (Some(p), primary.peak_market_cap_date.clone()),
            _ => (None, None),
        };
        primary.market_cap_currency = yahoo.market_cap_currency.or(primary.market_cap_currency);
        primary.latest_market_cap = yahoo.latest_market_cap;
        primary.latest_market_cap_date = yahoo.latest_market_cap_date;
        primary.peak_market_cap = combined_peak.0;
        primary.peak_market_cap_date = combined_peak.1;
    }
    if yahoo.trailing_pe.is_some() {
        primary.trailing_pe = yahoo.trailing_pe;
    }
    if yahoo.trailing_ps.is_some() {
        primary.trailing_ps = yahoo.trailing_ps;
    }
    if yahoo.trailing_pb.is_some() {
        primary.trailing_pb = yahoo.trailing_pb;
    }
    if yahoo_has_rows || yahoo.latest_market_cap.is_some() || yahoo.trailing_pe.is_some() {
        primary.source = format!("{} + Yahoo", primary.source);
    }
    primary.fetched_at = chrono::Utc::now().timestamp();
    primary
}

async fn fetch_yahoo_financials(sym: &str, now: i64) -> Result<StockFinancials, String> {
    let encoded_sym: String = url::form_urlencoded::byte_serialize(sym.as_bytes()).collect();
    let period2 = now + 86_400;
    let url = format!(
        "https://query1.finance.yahoo.com/ws/fundamentals-timeseries/v1/finance/timeseries/{encoded_sym}?type={YAHOO_FINANCIAL_TYPES}&period1=1262304000&period2={period2}"
    );
    let bytes = traced_get_bytes("Yahoo 财报", &url, None).await?;
    parse_yahoo_financials(sym, &bytes)
}

/// Fetch long-history company financials & historical market-cap snapshots (24 h persistent cache).
///
/// Queries the market-specific deep-history source (SEC EDGAR for US, Eastmoney for CN/HK,
/// FinMind for TW) concurrently with Yahoo Finance (for PE/PS/PB, market-cap snapshots, and
/// universal fallback).
pub async fn fetch_financials(
    cached: HashMap<String, StockFinancials>,
    symbol: &str,
) -> Result<(Option<StockFinancials>, StockFinancials), AppError> {
    let sym = symbol.trim();
    if sym.is_empty() {
        return Err(AppError::InvalidInput("symbol is required".into()));
    }
    let key = sym.to_ascii_uppercase();
    if is_non_equity_symbol(&key) {
        return Ok((None, empty_financials(sym)));
    }

    let now = chrono::Utc::now().timestamp();
    let market = detect_market(sym);
    let has_deep_source = matches!(market, "US" | "CN" | "HK" | "TW");
    let stale = cached.get(&key).cloned();
    if let Some(ref hit) = stale {
        let ttl = if has_deep_source && hit.source == "Yahoo Finance" {
            FINANCIALS_FALLBACK_TTL_SECS
        } else {
            FINANCIALS_TTL_SECS
        };
        if now - hit.fetched_at < ttl {
            return Ok((None, hit.clone()));
        }
    }

    let fail_ttl = Duration::from_secs(300);
    let recently_failed = financials_fail()
        .lock()
        .ok()
        .and_then(|m| m.get(&key).copied())
        .is_some_and(|at| at.elapsed() < fail_ttl);
    if recently_failed {
        if let Some(hit) = stale {
            return Ok((None, hit));
        }
        return Err(AppError::Message("财报数据暂不可用，请稍后重试".into()));
    }

    let primary_fut = async {
        match market {
            "CN" => fetch_eastmoney_cn_financials(sym).await,
            "HK" => fetch_eastmoney_hk_financials(sym).await,
            "TW" => fetch_finmind_tw_financials(sym).await,
            "US" => fetch_sec_financials(sym).await,
            _ => None,
        }
    };
    let (primary_res, yahoo_res) = tokio::join!(primary_fut, fetch_yahoo_financials(sym, now));

    let combined = match (primary_res, yahoo_res) {
        (Some(primary), Ok(yahoo)) => Ok(merge_financials(primary, yahoo, market)),
        (Some(primary), Err(_)) => Ok(primary),
        (None, Ok(yahoo)) => Ok(yahoo),
        (None, Err(err)) => Err(err),
    };

    match combined {
        Ok(parsed) => {
            if let Ok(mut fail_map) = financials_fail().lock() {
                fail_map.remove(&key);
            }
            Ok((Some(parsed.clone()), parsed))
        }
        Err(err) => {
            if let Ok(mut fail_map) = financials_fail().lock() {
                fail_map.insert(key, std::time::Instant::now());
            }
            if let Some(hit) = stale {
                Ok((None, hit))
            } else {
                Err(AppError::Message(err))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Intraday points for list-view trend charts (pools over 24 stocks fetch batch
// quotes, which carry none). Loaded a few at a time for the rows on screen.
// ---------------------------------------------------------------------------

/// Points per symbol and per-call limits; results (and failures, briefly) are cached.
const SPARKLINE_POINTS: usize = 48;
const SPARKLINES_MAX_PER_CALL: usize = 10;
const SPARKLINES_CONCURRENCY: usize = 2;
const SPARKLINE_TTL: Duration = Duration::from_secs(300);
const SPARKLINE_FAIL_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SparklinesResponse {
    pub items: HashMap<String, Vec<ChartPoint>>,
    /// Symbols fetched over the network in this call, and how many succeeded.
    pub attempted: usize,
    pub fetched: usize,
}

type SparklineCache = HashMap<String, (std::time::Instant, Option<Vec<ChartPoint>>)>;

fn sparkline_cache() -> &'static std::sync::Mutex<SparklineCache> {
    static CACHE: OnceLock<std::sync::Mutex<SparklineCache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

async fn fetch_one_sparkline(symbol: &str) -> Option<Vec<ChartPoint>> {
    let market = detect_market(symbol);
    if matches!(market, "CN" | "HK") && tencent_available(TencentService::Minute) {
        if let Some(code) = to_tencent_code(symbol) {
            match fetch_tencent_quote(symbol, "", &code, SPARKLINE_POINTS).await {
                Ok(q) => {
                    tencent_report(TencentService::Minute, true);
                    if q.sparkline.len() >= 2 {
                        return Some(q.sparkline);
                    }
                }
                Err(e) if e.contains("无报价数据") => {}
                Err(_) => tencent_report(TencentService::Minute, false),
            }
        }
    }
    let (_, points) = fetch_yahoo_chart_raw(symbol, "1d", false).await.ok()?;
    (points.len() >= 2).then(|| downsample_points(points, SPARKLINE_POINTS))
}

/// Intraday points for up to [`SPARKLINES_MAX_PER_CALL`] symbols not cached yet;
/// cached ones (including recent failures, as empty) are returned without a request.
pub async fn fetch_sparklines(symbols: Vec<String>) -> SparklinesResponse {
    let now = std::time::Instant::now();
    let mut items = HashMap::new();
    let mut missing = Vec::new();
    {
        let cache = sparkline_cache().lock().ok();
        for sym in symbols {
            let sym = sym.trim().to_string();
            if sym.is_empty() || items.contains_key(&sym) || missing.contains(&sym) {
                continue;
            }
            match cache.as_ref().and_then(|c| c.get(&sym)) {
                Some((at, Some(points))) if now.duration_since(*at) < SPARKLINE_TTL => {
                    items.insert(sym, points.clone());
                }
                Some((at, None)) if now.duration_since(*at) < SPARKLINE_FAIL_TTL => {}
                _ => missing.push(sym),
            }
        }
    }
    missing.truncate(SPARKLINES_MAX_PER_CALL);
    let attempted = missing.len();
    let fetched: Vec<(String, Option<Vec<ChartPoint>>)> = stream::iter(missing)
        .map(|sym| async move {
            let points = fetch_one_sparkline(&sym).await;
            (sym, points)
        })
        .buffer_unordered(SPARKLINES_CONCURRENCY)
        .collect()
        .await;
    let mut ok = 0;
    if let Ok(mut cache) = sparkline_cache().lock() {
        if cache.len() > 4000 {
            cache.clear();
        }
        for (sym, points) in fetched {
            if let Some(p) = &points {
                ok += 1;
                items.insert(sym.clone(), p.clone());
            }
            cache.insert(sym, (std::time::Instant::now(), points));
        }
    }
    SparklinesResponse {
        items,
        attempted,
        fetched: ok,
    }
}

// ---------------------------------------------------------------------------
// Multi-market search (Tencent SmartBox UTF-8 escapes + Yahoo Finance Search)
// ---------------------------------------------------------------------------

fn parse_tencent_smartbox(raw: &str) -> Vec<SearchHit> {
    let Some(start) = raw.find('"') else {
        return Vec::new();
    };
    let Some(end) = raw.rfind('"') else {
        return Vec::new();
    };
    if end <= start {
        return Vec::new();
    }
    let quoted = &raw[start..=end];
    let Ok(decoded) = serde_json::from_str::<String>(quoted) else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for entry in decoded.split('^') {
        let parts: Vec<&str> = entry.split('~').collect();
        if parts.len() < 4 {
            continue;
        }
        let mkt = parts[0];
        let code = parts[1].trim();
        let name = parts[2].trim();
        if code.is_empty() || name.is_empty() {
            continue;
        }
        let (symbol, market, exchange) = match mkt {
            "sh" => (format!("{code}.SS"), "CN", "SSE"),
            "sz" => (format!("{code}.SZ"), "CN", "SZSE"),
            "bj" => (format!("{code}.BJ"), "CN", "BSE"),
            "hk" => {
                let trimmed = code.trim_start_matches('0');
                let padded = if trimmed.len() < 4 {
                    format!("{:0>4}", trimmed)
                } else {
                    trimmed.to_string()
                };
                (format!("{padded}.HK"), "HK", "HKEX")
            }
            "us" => (us_symbol_from_tencent(code), "US", "US"),
            _ => continue,
        };
        hits.push(SearchHit {
            symbol,
            name: name.to_string(),
            market: market.to_string(),
            exchange: exchange.to_string(),
        });
    }
    hits
}

/// Tencent US codes carry an exchange suffix (`aapl.oq`, `brk.b.n`); drop only that
/// suffix so share classes survive, in the dash form Yahoo uses (`BRK-B`).
fn us_symbol_from_tencent(code: &str) -> String {
    const EXCHANGE_SUFFIXES: [&str; 5] = ["oq", "n", "am", "ps", "op"];
    let lower = code.to_ascii_lowercase();
    let base = match lower.rsplit_once('.') {
        Some((head, tail)) if EXCHANGE_SUFFIXES.contains(&tail) => head,
        _ => lower.as_str(),
    };
    base.replace('.', "-").to_ascii_uppercase()
}

// ---------------------------------------------------------------------------
// Exchange rates (ExchangeRate-API open endpoint: free, no key, updated daily)
// ---------------------------------------------------------------------------

/// Currencies the Markets page converts to USD to compare market cap and traded value.
const FX_CURRENCIES: [&str; 5] = ["CNY", "HKD", "TWD", "JPY", "EUR"];
/// The source updates once a day; refetch at most every six hours.
const FX_CACHE_SECS: u64 = 6 * 3600;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FxRates {
    /// Units of each currency per 1 USD.
    pub rates: HashMap<String, f64>,
    /// When the source last updated the rates (Unix seconds).
    pub updated_at: Option<i64>,
    pub source: String,
}

/// Pick the currencies we use from an ExchangeRate-API `latest/USD` response.
fn parse_fx_rates(body: &Value) -> Result<FxRates, String> {
    if body["result"].as_str() != Some("success") {
        return Err(format!(
            "汇率接口返回 {}",
            body["error-type"].as_str().unwrap_or("未知错误")
        ));
    }
    let rates: HashMap<String, f64> = FX_CURRENCIES
        .iter()
        .filter_map(|code| {
            body["rates"][*code]
                .as_f64()
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(|v| (code.to_string(), v))
        })
        .collect();
    if rates.is_empty() {
        return Err("汇率接口没有返回可用汇率".into());
    }
    Ok(FxRates {
        rates,
        updated_at: body["time_last_update_unix"].as_i64(),
        source: "ExchangeRate-API".into(),
    })
}

/// Current USD exchange rates, cached in memory for [`FX_CACHE_SECS`].
pub async fn fetch_fx_rates() -> Result<FxRates, AppError> {
    static CACHE: std::sync::Mutex<Option<(std::time::Instant, FxRates)>> =
        std::sync::Mutex::new(None);
    if let Some((at, rates)) = CACHE.lock().ok().and_then(|c| c.clone()) {
        if at.elapsed() < Duration::from_secs(FX_CACHE_SECS) {
            return Ok(rates);
        }
    }
    let bytes = traced_get_bytes(
        "ExchangeRate-API",
        "https://open.er-api.com/v6/latest/USD",
        None,
    )
    .await
    .map_err(AppError::Message)?;
    let body: Value = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Message(format!("汇率 JSON 解析失败: {e}")))?;
    let rates = parse_fx_rates(&body).map_err(AppError::Message)?;
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((std::time::Instant::now(), rates.clone()));
    }
    Ok(rates)
}

pub async fn search_symbols(query: &str) -> Vec<SearchHit> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let encoded: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
    let tencent_url = format!("https://smartbox.gtimg.cn/s3/?v=2&t=all&c=1&q={encoded}");
    let yahoo_url = format!(
        "https://query1.finance.yahoo.com/v1/finance/search?q={encoded}&quotesCount=10&newsCount=0"
    );

    let (t_res, y_res) = tokio::join!(
        async {
            let bytes = traced_get_bytes("腾讯搜索", &tencent_url, None)
                .await
                .ok()?;
            String::from_utf8(bytes).ok()
        },
        async {
            let bytes = traced_get_bytes("Yahoo 搜索", &yahoo_url, None)
                .await
                .ok()?;
            serde_json::from_slice::<Value>(&bytes).ok()
        }
    );

    let mut out = Vec::new();
    let mut seen = HashSet::new();

    // Direct 4-digit Taiwan stock shortcut (e.g. `2330` -> `2330.TW`)
    if q.len() == 4 && q.chars().all(|c| c.is_ascii_digit()) {
        let tw = format!("{q}.TW");
        if seen.insert(tw.clone()) {
            out.push(SearchHit {
                symbol: tw,
                name: format!("台股上市 {q}"),
                market: "TW".into(),
                exchange: "TWSE".into(),
            });
        }
    }

    if let Some(raw) = t_res {
        for hit in parse_tencent_smartbox(&raw) {
            if seen.insert(hit.symbol.clone()) {
                out.push(hit);
            }
        }
    }

    if let Some(json) = y_res {
        if let Some(arr) = json["quotes"].as_array() {
            for item in arr {
                let Some(sym) = item["symbol"].as_str().filter(|s| !s.is_empty()) else {
                    continue;
                };
                if !seen.insert(sym.to_string()) {
                    continue;
                }
                let name = item["shortname"]
                    .as_str()
                    .or_else(|| item["longname"].as_str())
                    .unwrap_or(sym)
                    .to_string();
                let exchange = item["exchDisp"]
                    .as_str()
                    .or_else(|| item["exchange"].as_str())
                    .unwrap_or("")
                    .to_string();
                let market = detect_market(sym).to_string();
                out.push(SearchHit {
                    symbol: sym.to_string(),
                    name,
                    market,
                    exchange,
                });
            }
        }
    }

    out.truncate(14);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_routing_detects_markets_and_channels() {
        assert_eq!(detect_market("NVDA"), "US");
        assert_eq!(detect_market("2330.TW"), "TW");
        assert_eq!(detect_market("6488.TWO"), "TW");
        assert_eq!(detect_market("^TWII"), "TW");
        assert_eq!(detect_market("600519.SS"), "CN");
        assert_eq!(detect_market("300750.SZ"), "CN");
        assert_eq!(detect_market("0700.HK"), "HK");
        assert_eq!(detect_market("^HSI"), "HK");

        assert_eq!(to_tencent_code("600519.SS").as_deref(), Some("sh600519"));
        assert_eq!(to_tencent_code("0700.HK").as_deref(), Some("hk00700"));
        assert_eq!(to_tencent_code("^HSI").as_deref(), Some("hkHSI"));
        assert_eq!(to_twse_channel("2330.TW").as_deref(), Some("tse_2330.tw"));
        assert_eq!(to_twse_channel("6488.TWO").as_deref(), Some("otc_6488.tw"));
        assert_eq!(to_twse_channel("^TWII").as_deref(), Some("tse_t00.tw"));
        assert_eq!(to_tencent_batch_code("NVDA").as_deref(), Some("usNVDA"));
        assert_eq!(to_tencent_batch_code("BRK-B").as_deref(), Some("usBRK.B"));
        assert_eq!(to_tencent_batch_code("^GSPC"), None);
    }

    #[test]
    fn gbk_tilde_splitter_preserves_double_byte_characters_with_0x7e() {
        // 0x81 0x7E is a valid GBK double-byte character whose second byte equals '~' (0x7E).
        let raw: Vec<u8> = vec![b'1', b'~', 0x81, 0x7E, b'~', b'1', b'2', b'3'];
        let parts = split_gbk_tilde(&raw);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], b"1");
        assert_eq!(parts[1], &[0x81, 0x7E]);
        assert_eq!(parts[2], b"123");
    }

    #[test]
    fn tencent_batch_line_extracts_price_change_and_market_cap() {
        let mut fields = vec!["".to_string(); 50];
        fields[0] = "200".into();
        fields[1] = "Nvidia".into();
        fields[2] = "NVDA.OQ".into();
        fields[3] = "125.53".into();
        fields[4] = "145.98".into();
        fields[5] = "143.52".into();
        fields[6] = "633418432".into();
        fields[30] = "2026-11-20 16:00:04".into();
        fields[31] = "-20.45".into();
        fields[32] = "-14.01".into();
        fields[33] = "143.73".into();
        fields[34] = "124.50".into();
        fields[45] = "30311.72910".into();
        let line = format!("v_usNVDA=\"{}\";", fields.join("~"));

        let mut map = HashMap::new();
        map.insert(
            "usNVDA".to_string(),
            WatchItem::simple("NVDA", "英伟达", "US"),
        );
        let q = parse_tencent_batch_line(line.as_bytes(), &map).expect("quote parsed");
        assert_eq!(q.symbol, "NVDA");
        assert_eq!(q.name, "英伟达");
        assert!((q.price.unwrap() - 125.53).abs() < 1e-6);
        assert!((q.change_percent.unwrap() - (-14.01)).abs() < 1e-6);
        assert!((q.market_cap.unwrap() - 3_031_172_910_000.0).abs() < 1e5);
    }

    #[tokio::test]
    async fn cached_sparklines_are_served_without_a_request() {
        let pts = vec![
            ChartPoint {
                ts: 1,
                price: 10.0,
                open: None,
                high: None,
                low: None,
                volume: None,
            },
            ChartPoint {
                ts: 2,
                price: 11.0,
                open: None,
                high: None,
                low: None,
                volume: None,
            },
        ];
        {
            let mut cache = sparkline_cache().lock().unwrap();
            let now = std::time::Instant::now();
            cache.insert("TEST-OK".into(), (now, Some(pts.clone())));
            cache.insert("TEST-FAILED".into(), (now, None));
        }
        let res = fetch_sparklines(vec!["TEST-OK".into(), "TEST-FAILED".into()]).await;
        assert_eq!(res.attempted, 0, "both come from the cache");
        assert_eq!(res.items.get("TEST-OK").map(Vec::len), Some(2));
        assert!(
            !res.items.contains_key("TEST-FAILED"),
            "recent failure is not retried"
        );
    }

    #[test]
    fn twse_daily_fill_only_outside_the_session() {
        use chrono::TimeZone;
        // 2026-11-24 is a Tuesday; times are UTC (Taipei is UTC+8).
        let at = |h, m| chrono::Utc.with_ymd_and_hms(2026, 11, 24, h, m, 0).unwrap();
        assert!(
            !twse_session_open(at(0, 59)),
            "08:59 Taipei: before the open"
        );
        assert!(twse_session_open(at(1, 0)), "09:00 Taipei: open");
        assert!(twse_session_open(at(5, 29)), "13:29 Taipei: open");
        assert!(!twse_session_open(at(5, 30)), "13:30 Taipei: closed");
        let saturday = chrono::Utc.with_ymd_and_hms(2026, 11, 28, 2, 0, 0).unwrap();
        assert!(!twse_session_open(saturday), "weekend");
    }

    #[test]
    fn a_blocked_tencent_service_does_not_pause_the_other() {
        tencent_report(TencentService::Minute, false);
        assert!(!tencent_available(TencentService::Minute));
        assert!(
            tencent_available(TencentService::Batch),
            "batch quotes keep working while the intraday service is blocked"
        );
        tencent_report(TencentService::Minute, true);
        assert!(tencent_available(TencentService::Minute));
    }

    #[test]
    fn tencent_backoff_grows_and_caps() {
        assert_eq!(backoff_delay(1).as_secs(), 60);
        assert_eq!(backoff_delay(2).as_secs(), 120);
        assert_eq!(backoff_delay(3).as_secs(), 240);
        assert_eq!(backoff_delay(5).as_secs(), 900);
        assert_eq!(backoff_delay(40).as_secs(), 900);
        assert!(looks_blocked(b"  <!DOCTYPE html><html>"));
        assert!(!looks_blocked(b"v_usBABA=\"200~"));
    }

    #[test]
    fn failed_quotes_fall_back_to_the_last_good_one() {
        let item = WatchItem::simple("BABA", "阿里巴巴", "US");
        let mut good = error_quote(&item, "");
        good.error = None;
        good.price = Some(86.0);
        good.source = "腾讯批量实时".into();
        let mut cache = HashMap::new();
        let first = with_last_good(vec![good.clone()], &mut cache);
        assert_eq!(first[0].source, "腾讯批量实时");
        let failed = with_last_good(vec![error_quote(&item, "行情源暂不可用")], &mut cache);
        assert_eq!(failed[0].price, Some(86.0));
        assert!(failed[0].error.is_none());
        assert_eq!(failed[0].source, "腾讯批量实时（缓存）");
        // A later quote without a market cap (Yahoo) reuses the last one, scaled by price.
        good.market_cap = Some(2_000e9);
        with_last_good(vec![good.clone()], &mut cache);
        let mut yahoo = good.clone();
        yahoo.market_cap = None;
        yahoo.price = Some(86.0 * 1.1);
        let filled = with_last_good(vec![yahoo], &mut cache);
        assert!((filled[0].market_cap.unwrap() - 2_200e9).abs() < 1.0);
        // A symbol never fetched successfully keeps its error.
        let other = WatchItem {
            symbol: "XYZ".into(),
            ..item
        };
        let none = with_last_good(vec![error_quote(&other, "x")], &mut cache);
        assert!(none[0].error.is_some());
    }

    #[test]
    fn fx_rates_keep_known_currencies_and_reject_failures() {
        let ok = serde_json::json!({
            "result": "success",
            "time_last_update_unix": 1_795_000_000,
            "rates": {"USD": 1, "CNY": 6.68, "HKD": 7.84, "TWD": 32.97, "GBP": 0.79, "JPY": 0}
        });
        let fx = parse_fx_rates(&ok).unwrap();
        assert_eq!(fx.rates.get("CNY"), Some(&6.68));
        assert_eq!(fx.rates.get("TWD"), Some(&32.97));
        assert!(
            !fx.rates.contains_key("GBP"),
            "only currencies the page uses"
        );
        assert!(!fx.rates.contains_key("JPY"), "zero is not a rate");
        assert_eq!(fx.updated_at, Some(1_795_000_000));
        let failed = serde_json::json!({"result": "error", "error-type": "quota-reached"});
        assert!(parse_fx_rates(&failed)
            .unwrap_err()
            .contains("quota-reached"));
    }

    #[test]
    fn tencent_turnover_reads_exact_amounts_per_market() {
        // Live samples: Moutai (CN), Tencent (HK), NVIDIA (US).
        assert_eq!(
            tencent_turnover("1439.00/56976/8223838967", "822384", "CN"),
            Some(8_223_838_967.0)
        );
        assert_eq!(tencent_turnover("", "822384", "CN"), Some(8_223_840_000.0));
        assert_eq!(
            tencent_turnover("344.000", "11960782218.610", "HK"),
            Some(11_960_782_218.61)
        );
        assert_eq!(
            tencent_turnover("USD", "108334123344", "US"),
            Some(108_334_123_344.0)
        );
        assert_eq!(tencent_turnover("", "0", "US"), None);
    }

    #[test]
    fn tencent_us_codes_keep_share_classes() {
        assert_eq!(us_symbol_from_tencent("aapl.oq"), "AAPL");
        assert_eq!(us_symbol_from_tencent("brk.b.n"), "BRK-B");
        assert_eq!(us_symbol_from_tencent("BRK.B"), "BRK-B");
        assert_eq!(us_symbol_from_tencent("tsla"), "TSLA");
    }

    #[test]
    fn tencent_smartbox_parser_decodes_unicode_names() {
        let sample = r#"v_hint="sh~600519~\u8d35\u5dde\u8305\u53f0~gzmt~GP-A^hk~00700~\u817e\u8baf\u63a7\u80a1~txkg~GP""#;
        let hits = parse_tencent_smartbox(sample);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].symbol, "600519.SS");
        assert_eq!(hits[0].name, "贵州茅台");
        assert_eq!(hits[0].market, "CN");
        assert_eq!(hits[1].symbol, "0700.HK");
        assert_eq!(hits[1].name, "腾讯控股");
        assert_eq!(hits[1].market, "HK");
    }

    #[test]
    fn tencent_minute_parser_extracts_intraday_points() {
        let arr = vec![
            Value::String("0930 1475.35 1063 156829705.00".into()),
            Value::String("0931 1455.50 4938 726902333.00".into()),
        ];
        let pts = parse_tencent_minute_points("20261123", &arr);
        assert_eq!(pts.len(), 2);
        assert!((pts[0].price - 1475.35).abs() < 1e-6);
        assert!((pts[1].price - 1455.50).abs() < 1e-6);
        assert!(pts[1].ts > pts[0].ts);
    }

    #[test]
    fn markets_config_supports_legacy_json_and_pool_edits_roundtrip() {
        let legacy = r#"{"indices":[],"watchlist":[{"symbol":"NVDA","name":"英伟达","market":"US"}],"colorScheme":"cn","refreshSec":15}"#;
        let parsed: MarketsConfig = serde_json::from_str(legacy).expect("legacy config parses");
        assert!(parsed.pool_edits.is_empty());
        assert!(parsed.custom_pools.is_empty());

        let mut cfg = parsed;
        cfg.pool_edits.insert(
            "ai".into(),
            PoolEdit {
                added: vec![WatchItem {
                    symbol: "CRWV".into(),
                    name: "CoreWeave".into(),
                    market: "US".into(),
                    sector: Some("算力芯片".into()),
                    cap_hint: None,
                }],
                removed: vec!["SMCI".into()],
            },
        );
        cfg.custom_pools.push(CustomPool {
            id: "custom_robot".into(),
            name: "机器人观察池".into(),
            description: "核心人形机器人标的".into(),
            items: vec![WatchItem {
                symbol: "TSLA".into(),
                name: "特斯拉".into(),
                market: "US".into(),
                sector: Some("整机".into()),
                cap_hint: None,
            }],
        });
        let encoded = serde_json::to_string(&cfg).expect("serializes");
        let roundtrip: MarketsConfig = serde_json::from_str(&encoded).expect("deserializes");
        assert_eq!(roundtrip.pool_edits["ai"].added[0].symbol, "CRWV");
        assert_eq!(
            roundtrip.pool_edits["ai"].added[0].sector.as_deref(),
            Some("算力芯片")
        );
        assert_eq!(roundtrip.pool_edits["ai"].removed, vec!["SMCI"]);
        assert_eq!(roundtrip.custom_pools.len(), 1);
        assert_eq!(roundtrip.custom_pools[0].name, "机器人观察池");
    }

    #[test]
    fn twse_day_all_parses_roc_date_change_and_turnover() {
        let sample = r#"[
            {"Date":"1151123","Code":"2330","Name":"台積電","TradeVolume":"25000000","TradeValue":"50750000000","OpeningPrice":"2010.00","HighestPrice":"2040.00","LowestPrice":"2005.00","ClosingPrice":"2030.00","Change":"+25.0000","Transaction":"12345"},
            {"Date":"1151123","Code":"2317","Name":"鴻海","TradeVolume":"18000000","TradeValue":"3708000000","OpeningPrice":"208.00","HighestPrice":"209.00","LowestPrice":"205.00","ClosingPrice":"206.00","Change":"-2.5000","Transaction":"9876"},
            {"Date":"1151123","Code":"2881","Name":"富邦金","TradeVolume":"10000000","TradeValue":"1305000000","OpeningPrice":"130.50","HighestPrice":"131.00","LowestPrice":"130.00","ClosingPrice":"130.50","Change":"X0.0000","Transaction":"5432"}
        ]"#
        .as_bytes();
        let parsed = parse_twse_day_all(sample);
        assert_eq!(parsed.len(), 3);

        let tsmc = &parsed["2330.TW"];
        assert_eq!(tsmc.price, Some(2030.0));
        assert_eq!(tsmc.prev_close, Some(2005.0));
        assert_eq!(tsmc.volume, Some(25_000_000));
        assert_eq!(tsmc.turnover, Some(50_750_000_000.0));
        assert!(tsmc.from_daily);
        assert!(tsmc.quote_time.is_some());

        let honhai = &parsed["2317.TW"];
        assert_eq!(honhai.price, Some(206.0));
        assert_eq!(honhai.prev_close, Some(208.5));

        let fubon = &parsed["2881.TW"];
        assert_eq!(fubon.price, Some(130.5));
        assert_eq!(fubon.prev_close, Some(130.5));
    }

    #[test]
    fn computes_all_time_high_and_low_from_monthly_candles_and_tencent_json() {
        let pts = vec![
            ChartPoint {
                ts: 1_500_000_000,
                price: 12.0,
                open: Some(10.0),
                high: Some(15.0),
                low: Some(8.5),
                volume: Some(100),
            },
            ChartPoint {
                ts: 1_700_000_000,
                price: 210.0,
                open: Some(180.0),
                high: Some(243.37),
                low: Some(175.0),
                volume: Some(500),
            },
        ];
        let ext = compute_extremes_from_points(&pts, Some(240.0), Some(100.0))
            .expect("should compute extremes");
        assert_eq!(ext.ath, 243.37);
        assert_eq!(ext.ath_ts, Some(1_700_000_000));
        assert_eq!(ext.atl, 8.5);
        assert_eq!(ext.atl_ts, Some(1_500_000_000));

        let tencent_json = r#"{"data":{"sh600519":{"qfqmonth":[["2015-01-30","100.0","110.0","120.0","95.5","1000"],["2021-02-26","2000.0","2100.0","2608.59","1950.0","2000"]]}}}"#.as_bytes();
        let t_ext =
            parse_tencent_monthly_extremes(tencent_json, "sh600519").expect("tencent monthly");
        assert_eq!(t_ext.ath, 2608.59);
        assert_eq!(t_ext.atl, 95.5);
    }

    #[test]
    fn parses_yahoo_financials_preserving_separate_statement_and_market_cap_currencies() {
        let sample = serde_json::json!({
            "timeseries": {
                "result": [
                    {
                        "meta": {"symbol": ["0700.HK"], "type": ["annualTotalRevenue"]},
                        "annualTotalRevenue": [
                            null,
                            {"asOfDate": "2024-12-31", "currencyCode": "CNY", "reportedValue": {"raw": 6.60e11}},
                            {"asOfDate": "2025-12-31", "currencyCode": "CNY", "reportedValue": {"raw": 7.40e11}}
                        ]
                    },
                    {
                        "meta": {"symbol": ["0700.HK"], "type": ["annualNetIncome"]},
                        "annualNetIncome": [
                            {"asOfDate": "2024-12-31", "currencyCode": "CNY", "reportedValue": {"raw": 1.94e11}},
                            {"asOfDate": "2025-12-31", "currencyCode": "CNY", "reportedValue": {"raw": 2.25e11}}
                        ]
                    },
                    {
                        "meta": {"symbol": ["0700.HK"], "type": ["annualFreeCashFlow"]},
                        "annualFreeCashFlow": [
                            {"asOfDate": "2024-12-31", "currencyCode": "CNY", "reportedValue": {"raw": -1.2e10}},
                            {"asOfDate": "2025-12-31", "currencyCode": "CNY", "reportedValue": {"raw": 1.8e11}}
                        ]
                    },
                    {
                        "meta": {"symbol": ["0700.HK"], "type": ["quarterlyTotalRevenue"]},
                        "quarterlyTotalRevenue": [
                            {"asOfDate": "2025-09-30", "currencyCode": "CNY", "reportedValue": {"raw": 1.85e11}},
                            {"asOfDate": "2025-12-31", "currencyCode": "CNY", "reportedValue": {"raw": 1.95e11}}
                        ]
                    },
                    {
                        "meta": {"symbol": ["0700.HK"], "type": ["quarterlyMarketCap"]},
                        "quarterlyMarketCap": [
                            {"asOfDate": "2025-09-30", "currencyCode": "HKD", "reportedValue": {"raw": 6.01e12}},
                            {"asOfDate": "2025-12-31", "currencyCode": "HKD", "reportedValue": {"raw": 5.41e12}}
                        ]
                    },
                    {
                        "meta": {"symbol": ["0700.HK"], "type": ["trailingPeRatio"]},
                        "trailingPeRatio": [
                            {"asOfDate": "2025-09-30", "reportedValue": {"raw": 22.5}},
                            {"asOfDate": "2025-12-31", "reportedValue": {"raw": 19.8}}
                        ]
                    }
                ],
                "error": null
            }
        });
        let bytes = serde_json::to_vec(&sample).unwrap();
        let fin = parse_yahoo_financials("0700.HK", &bytes).expect("parse financials");
        assert_eq!(fin.currency, "CNY");
        assert_eq!(fin.market_cap_currency.as_deref(), Some("HKD"));
        assert_eq!(fin.latest_market_cap, Some(5.41e12));
        assert_eq!(fin.latest_market_cap_date.as_deref(), Some("2025-12-31"));
        assert_eq!(fin.peak_market_cap, Some(6.01e12));
        assert_eq!(fin.peak_market_cap_date.as_deref(), Some("2025-09-30"));
        assert_eq!(fin.trailing_pe, Some(19.8));
        assert_eq!(fin.annual.len(), 2);
        assert_eq!(fin.annual[0].free_cash_flow, Some(-1.2e10));
        assert_eq!(fin.annual[1].revenue, Some(7.40e11));
        assert_eq!(fin.quarterly.len(), 2);

        assert!(is_non_equity_symbol("^GSPC"));
        assert!(is_non_equity_symbol("000300.SS"));
        assert!(is_non_equity_symbol("BTC-USD"));
        assert!(!is_non_equity_symbol("NVDA"));
        assert!(!is_non_equity_symbol("2330.TW"));
        assert!(!is_non_equity_symbol("0700.HK"));
    }

    #[test]
    fn parses_sec_edgar_multi_tag_history_q4_derivation_and_fuzzy_yahoo_merge() {
        let sec = serde_json::json!({
            "facts": {
                "us-gaap": {
                    "SalesRevenueNet": {
                        "units": {
                            "USD": [
                                {"start": "2015-09-27", "end": "2016-09-24", "val": 215.6e9, "form": "10-K", "filed": "2016-10-26"}
                            ]
                        }
                    },
                    "RevenueFromContractWithCustomerExcludingAssessedTax": {
                        "units": {
                            "USD": [
                                {"start": "2025-09-28", "end": "2025-12-27", "val": 140.0e9, "form": "10-Q", "filed": "2026-01-30"},
                                {"start": "2025-09-28", "end": "2026-03-28", "val": 250.0e9, "form": "10-Q", "filed": "2026-05-01"},
                                {"start": "2025-12-28", "end": "2026-03-28", "val": 110.0e9, "form": "10-Q", "filed": "2026-05-01"},
                                {"start": "2025-09-28", "end": "2026-06-27", "val": 360.0e9, "form": "10-Q", "filed": "2026-07-31"},
                                {"start": "2026-03-29", "end": "2026-06-27", "val": 110.0e9, "form": "10-Q", "filed": "2026-07-31"},
                                {"start": "2025-09-28", "end": "2026-09-26", "val": 475.0e9, "form": "10-K", "filed": "2026-11-03"}
                            ]
                        }
                    },
                    "NetCashProvidedByUsedInOperatingActivities": {
                        "units": {
                            "USD": [
                                {"start": "2025-09-28", "end": "2025-12-27", "val": 40.0e9, "form": "10-Q", "filed": "2026-01-30"},
                                {"start": "2025-09-28", "end": "2026-03-28", "val": 70.0e9, "form": "10-Q", "filed": "2026-05-01"},
                                {"start": "2025-09-28", "end": "2026-06-27", "val": 95.0e9, "form": "10-Q", "filed": "2026-07-31"},
                                {"start": "2025-09-28", "end": "2026-09-26", "val": 130.0e9, "form": "10-K", "filed": "2026-11-03"}
                            ]
                        }
                    },
                    "PaymentsToAcquirePropertyPlantAndEquipment": {
                        "units": {
                            "USD": [
                                {"start": "2025-09-28", "end": "2026-09-26", "val": 10.0e9, "form": "10-K", "filed": "2026-11-03"}
                            ]
                        }
                    }
                }
            }
        });
        let parsed =
            parse_sec_companyfacts("AAPL", &serde_json::to_vec(&sec).unwrap()).expect("sec parse");
        assert_eq!(parsed.currency, "USD");
        assert_eq!(parsed.annual.len(), 2);
        assert_eq!(parsed.annual[0].as_of_date, "2016-09-24");
        assert_eq!(parsed.annual[0].revenue, Some(215.6e9));
        assert_eq!(parsed.annual[1].as_of_date, "2026-09-26");
        assert_eq!(parsed.annual[1].free_cash_flow, Some(120.0e9));
        // Q1, Q2, Q3, and derived Q4 (475B - 360B = 115B) plus derived quarterly OCF (35B in Q4)
        assert_eq!(parsed.quarterly.len(), 4);
        assert_eq!(parsed.quarterly[3].as_of_date, "2026-09-26");
        assert_eq!(parsed.quarterly[3].revenue, Some(115.0e9));
        assert_eq!(parsed.quarterly[1].operating_cash_flow, Some(30.0e9));
        assert_eq!(parsed.quarterly[3].operating_cash_flow, Some(35.0e9));

        // Merging Yahoo's normalized 2026-09-30 into SEC's 2026-09-26 must not duplicate bars.
        let yahoo = StockFinancials {
            symbol: "AAPL".into(),
            currency: "USD".into(),
            market_cap_currency: Some("USD".into()),
            latest_market_cap: Some(3.8e12),
            latest_market_cap_date: Some("2026-11-23".into()),
            peak_market_cap: Some(4.1e12),
            peak_market_cap_date: Some("2026-10-15".into()),
            trailing_pe: Some(33.2),
            trailing_ps: Some(8.4),
            trailing_pb: None,
            annual: vec![FinancialPeriod {
                as_of_date: "2026-09-30".into(),
                revenue: Some(475.0e9),
                gross_profit: Some(220.0e9),
                ..Default::default()
            }],
            quarterly: vec![],
            source: "Yahoo Finance".into(),
            fetched_at: 1_795_000_000,
        };
        let merged = merge_financials(parsed, yahoo, "US");
        assert_eq!(merged.annual.len(), 2, "±10 day fiscal match deduplicates");
        assert_eq!(merged.annual[1].gross_profit, Some(220.0e9));
        assert_eq!(merged.peak_market_cap, Some(4.1e12));
        assert_eq!(merged.trailing_pe, Some(33.2));
        assert_eq!(merged.source, "SEC EDGAR + Yahoo");
    }

    #[test]
    fn parses_eastmoney_cn_hk_and_finmind_tw_financials() {
        let cn = serde_json::json!({
            "result": {
                "data": [
                    {"REPORT_DATE": "2025-12-31 00:00:00", "REPORT_TYPE": "年报", "TOTALOPERATEREVE": 170.0e9, "PARENTNETPROFIT": 82.0e9, "XSMLL": 90.0, "EPSXS": 65.0, "MGJYXJJE": 50.0, "TOTAL_SHARE": 1.25e9},
                    {"REPORT_DATE": "2025-09-30 00:00:00", "REPORT_TYPE": "三季报", "TOTALOPERATEREVE": 130.0e9, "PARENTNETPROFIT": 64.0e9, "XSMLL": 90.0, "EPSXS": 51.0, "MGJYXJJE": 30.0, "TOTAL_SHARE": 1.25e9},
                    {"REPORT_DATE": "2025-06-30 00:00:00", "REPORT_TYPE": "中报", "TOTALOPERATEREVE": 90.0e9, "PARENTNETPROFIT": 45.0e9, "XSMLL": 90.0, "EPSXS": 36.0, "MGJYXJJE": 10.0, "TOTAL_SHARE": 1.25e9},
                    {"REPORT_DATE": "2025-03-31 00:00:00", "REPORT_TYPE": "一季报", "TOTALOPERATEREVE": 50.0e9, "PARENTNETPROFIT": 25.0e9, "XSMLL": 90.0, "EPSXS": 20.0, "MGJYXJJE": 8.0, "TOTAL_SHARE": 1.25e9}
                ]
            }
        });
        let cn_fin = parse_eastmoney_cn_financials("600519.SS", &serde_json::to_vec(&cn).unwrap())
            .expect("cn parse");
        assert_eq!(cn_fin.annual.len(), 1);
        assert_eq!(cn_fin.annual[0].revenue, Some(170.0e9));
        assert_eq!(cn_fin.quarterly.len(), 4);
        assert_eq!(cn_fin.quarterly[0].revenue, Some(50.0e9));
        assert_eq!(cn_fin.quarterly[1].revenue, Some(40.0e9));
        assert_eq!(cn_fin.quarterly[2].revenue, Some(40.0e9));
        assert_eq!(cn_fin.quarterly[3].revenue, Some(40.0e9));
        assert_eq!(cn_fin.quarterly[3].net_income, Some(18.0e9));

        let tw = serde_json::json!({
            "status": 200,
            "data": [
                {"date": "2025-03-31", "type": "Revenue", "value": 800.0e9},
                {"date": "2025-03-31", "type": "IncomeAfterTaxes", "value": 360.0e9},
                {"date": "2025-06-30", "type": "Revenue", "value": 900.0e9},
                {"date": "2025-06-30", "type": "IncomeAfterTaxes", "value": 400.0e9},
                {"date": "2025-09-30", "type": "Revenue", "value": 1000.0e9},
                {"date": "2025-09-30", "type": "IncomeAfterTaxes", "value": 450.0e9},
                {"date": "2025-12-31", "type": "Revenue", "value": 1100.0e9},
                {"date": "2025-12-31", "type": "IncomeAfterTaxes", "value": 500.0e9}
            ]
        });
        let tw_fin = parse_finmind_tw_financials("2330.TW", &serde_json::to_vec(&tw).unwrap())
            .expect("tw parse");
        assert_eq!(tw_fin.quarterly.len(), 4);
        assert_eq!(tw_fin.annual.len(), 1);
        assert_eq!(tw_fin.annual[0].as_of_date, "2025-12-31");
        assert_eq!(tw_fin.annual[0].revenue, Some(3800.0e9));
        assert_eq!(tw_fin.annual[0].net_income, Some(1710.0e9));
    }
}
