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
    let started = std::time::Instant::now();
    let mut req = http_client().get(url);
    if let Some(r) = referer {
        req = req.header("Referer", r);
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
    let needs_twse_daily = tw_channels.iter().any(|(sym, _)| {
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
    if let Some(code) = to_tencent_batch_code(symbol) {
        let url = format!(
            "https://web.ifzq.gtimg.cn/appstock/app/fqkline/get?param={code},month,,,500,qfq"
        );
        if let Ok(bytes) = traced_get_bytes("腾讯历史月K", &url, None).await {
            if let Some(ext) = parse_tencent_monthly_extremes(&bytes, &code) {
                return Some(ext);
            }
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

    let limit = batch_limit.min(30);
    let to_fetch: Vec<String> = missing.iter().take(limit).cloned().collect();
    let remaining = missing.len().saturating_sub(to_fetch.len());

    let mut updates: HashMap<String, PriceExtremes> = HashMap::new();
    if !to_fetch.is_empty() {
        let fetched: Vec<(String, Option<PriceExtremes>)> = stream::iter(to_fetch)
            .map(|sym| async move {
                let res = fetch_single_extremes(&sym).await;
                (sym, res)
            })
            .buffer_unordered(4)
            .collect()
            .await;

        for (sym, opt) in fetched {
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
        },
    )
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
}
