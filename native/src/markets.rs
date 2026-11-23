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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchItem {
    pub symbol: String,
    pub name: String,
    /// `"US"` | `"TW"` | `"CN"` | `"HK"` | `"GLOBAL"`
    pub market: String,
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
}

impl Default for MarketsConfig {
    fn default() -> Self {
        Self {
            indices: vec![
                WatchItem {
                    symbol: "^GSPC".into(),
                    name: "标普500".into(),
                    market: "US".into(),
                },
                WatchItem {
                    symbol: "^IXIC".into(),
                    name: "纳斯达克".into(),
                    market: "US".into(),
                },
                WatchItem {
                    symbol: "^DJI".into(),
                    name: "道琼斯".into(),
                    market: "US".into(),
                },
                WatchItem {
                    symbol: "000001.SS".into(),
                    name: "上证指数".into(),
                    market: "CN".into(),
                },
                WatchItem {
                    symbol: "000300.SS".into(),
                    name: "沪深300".into(),
                    market: "CN".into(),
                },
                WatchItem {
                    symbol: "^HSI".into(),
                    name: "恒生指数".into(),
                    market: "HK".into(),
                },
                WatchItem {
                    symbol: "^TWII".into(),
                    name: "台湾加权".into(),
                    market: "TW".into(),
                },
            ],
            watchlist: vec![
                WatchItem {
                    symbol: "NVDA".into(),
                    name: "英伟达".into(),
                    market: "US".into(),
                },
                WatchItem {
                    symbol: "AAPL".into(),
                    name: "苹果".into(),
                    market: "US".into(),
                },
                WatchItem {
                    symbol: "MSFT".into(),
                    name: "微软".into(),
                    market: "US".into(),
                },
                WatchItem {
                    symbol: "TSLA".into(),
                    name: "特斯拉".into(),
                    market: "US".into(),
                },
                WatchItem {
                    symbol: "GOOGL".into(),
                    name: "谷歌".into(),
                    market: "US".into(),
                },
                WatchItem {
                    symbol: "AMZN".into(),
                    name: "亚马逊".into(),
                    market: "US".into(),
                },
                WatchItem {
                    symbol: "2330.TW".into(),
                    name: "台积电".into(),
                    market: "TW".into(),
                },
                WatchItem {
                    symbol: "2317.TW".into(),
                    name: "鸿海".into(),
                    market: "TW".into(),
                },
                WatchItem {
                    symbol: "2454.TW".into(),
                    name: "联发科".into(),
                    market: "TW".into(),
                },
                WatchItem {
                    symbol: "600519.SS".into(),
                    name: "贵州茅台".into(),
                    market: "CN".into(),
                },
                WatchItem {
                    symbol: "300750.SZ".into(),
                    name: "宁德时代".into(),
                    market: "CN".into(),
                },
                WatchItem {
                    symbol: "002594.SZ".into(),
                    name: "比亚迪".into(),
                    market: "CN".into(),
                },
                WatchItem {
                    symbol: "0700.HK".into(),
                    name: "腾讯控股".into(),
                    market: "HK".into(),
                },
                WatchItem {
                    symbol: "9988.HK".into(),
                    name: "阿里巴巴".into(),
                    market: "HK".into(),
                },
                WatchItem {
                    symbol: "3690.HK".into(),
                    name: "美团".into(),
                    market: "HK".into(),
                },
            ],
            color_scheme: "cn".into(),
            refresh_sec: 15,
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
    db.set_setting(WATCHLIST_SETTINGS_KEY, &raw)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartPoint {
    pub ts: i64,
    pub price: f64,
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

/// Batch-fetch up to hundreds of CN / HK / US real-time quotes + market caps via `qt.gtimg.cn`.
async fn fetch_tencent_batch(items: &[WatchItem]) -> HashMap<String, StockQuote> {
    let mut code_to_item: HashMap<String, WatchItem> = HashMap::new();
    let mut codes: Vec<String> = Vec::new();
    for item in items {
        let Some(code) = to_tencent_batch_code(&item.symbol) else {
            continue;
        };
        if !code_to_item.contains_key(&code) {
            codes.push(code.clone());
            code_to_item.insert(code, item.clone());
        }
    }
    if codes.is_empty() {
        return HashMap::new();
    }

    let chunks: Vec<Vec<String>> = codes.chunks(55).map(|c| c.to_vec()).collect();
    let map_ref = &code_to_item;
    let results: Vec<Vec<StockQuote>> = stream::iter(chunks)
        .map(|chunk| async move {
            let url = format!("https://qt.gtimg.cn/q={}", chunk.join(","));
            let Ok(resp) = http_client().get(&url).send().await else {
                return Vec::new();
            };
            if !resp.status().is_success() {
                return Vec::new();
            }
            let Ok(bytes) = resp.bytes().await else {
                return Vec::new();
            };
            let mut quotes = Vec::new();
            for line in bytes.split(|&b| b == b'\n') {
                if line.is_empty() {
                    continue;
                }
                if let Some(q) = parse_tencent_batch_line(line, map_ref) {
                    quotes.push(q);
                }
            }
            quotes
        })
        .buffer_unordered(8)
        .collect()
        .await;

    let mut out = HashMap::new();
    for batch in results {
        for q in batch {
            out.insert(q.symbol.clone(), q);
        }
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
    for item in arr {
        let Some(line) = item.as_str() else { continue };
        let mut parts = line.split_whitespace();
        let Some(hhmm) = parts.next() else { continue };
        let Some(price) = parts.next().and_then(parse_f64_opt) else {
            continue;
        };
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
        pts.push(ChartPoint { ts, price });
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
    let resp = http_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("腾讯行情请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("腾讯行情 HTTP {}", resp.status()));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("腾讯行情 JSON 解析失败: {e}"))?;
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
    let change = parse_f64_opt(get_str(31)).or_else(|| match (price, prev_close) {
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

struct TwseSnapshot {
    name: Option<String>,
    price: Option<f64>,
    prev_close: Option<f64>,
    open: Option<f64>,
    high: Option<f64>,
    low: Option<f64>,
    volume: Option<u64>,
    quote_time: Option<i64>,
    exchange: String,
}

async fn fetch_twse_batch(channels: &[(String, String)]) -> HashMap<String, TwseSnapshot> {
    if channels.is_empty() {
        return HashMap::new();
    }
    let chunks: Vec<Vec<(String, String)>> = channels.chunks(25).map(|c| c.to_vec()).collect();
    let batches: Vec<HashMap<String, TwseSnapshot>> = stream::iter(chunks)
        .map(|chunk| async move { fetch_twse_batch_chunk(&chunk).await })
        .buffer_unordered(3)
        .collect()
        .await;
    let mut out = HashMap::new();
    for map in batches {
        out.extend(map);
    }
    out
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
    let Ok(resp) = http_client()
        .get(&url)
        .header("Referer", "https://mis.twse.com.tw/stock/fibest.jsp")
        .send()
        .await
    else {
        return out;
    };
    if !resp.status().is_success() {
        return out;
    }
    let Ok(body) = resp.json::<Value>().await else {
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
                quote_time,
                exchange,
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
        "1y" => ("1y", "1d"),
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
    let encoded: String = url::form_urlencoded::byte_serialize(symbol.as_bytes()).collect();
    let url = format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{encoded}?interval={interval}&range={r}&includePrePost={include_pre_post}"
    );
    let resp = http_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Yahoo 行情请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Yahoo 行情 HTTP {}", resp.status()));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Yahoo JSON 解析失败: {e}"))?;
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
    let closes = result["indicators"]["quote"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|q| q["close"].as_array());

    let mut points = Vec::new();
    if let (Some(ts_arr), Some(close_arr)) = (timestamps, closes) {
        for (ts_v, c_v) in ts_arr.iter().zip(close_arr.iter()) {
            if let (Some(ts), Some(price)) = (ts_v.as_i64(), c_v.as_f64()) {
                if price.is_finite() && price > 0.0 {
                    points.push(ChartPoint { ts, price });
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

pub async fn fetch_quotes(items: Vec<WatchItem>, skip_sparkline: bool) -> Vec<StockQuote> {
    let use_fast_batch = skip_sparkline || items.len() > 24;

    // 1. Batch-fetch Taiwan (TWSE MIS, chunks of 25) and CN/HK/US (Tencent qt.gtimg.cn, chunks of 45)
    //    concurrently. For heatmaps / large 50-90 stock pools this finishes in ~200-350ms total!
    let tw_channels: Vec<(String, String)> = items
        .iter()
        .filter_map(|it| to_twse_channel(&it.symbol).map(|ch| (it.symbol.clone(), ch)))
        .collect();
    let (twse_map, tencent_batch_map) = if use_fast_batch {
        tokio::join!(fetch_twse_batch(&tw_channels), fetch_tencent_batch(&items))
    } else {
        (fetch_twse_batch(&tw_channels).await, HashMap::new())
    };

    // 2. Resolve each item: return fast-batch hit directly when available; otherwise fall back to
    //    per-symbol routed fetcher (full sparklines + US pre/post market + Yahoo fallback).
    //    For mega-pools (>60 symbols), cap per-symbol fallback calls to avoid a Yahoo request storm.
    let is_mega_pool = use_fast_batch && items.len() > 60;
    let mut fallback_budget: usize = if is_mega_pool { 8 } else { usize::MAX };
    let mut tasks = Vec::with_capacity(items.len());
    for (idx, item) in items.into_iter().enumerate() {
        let batch_hit = tencent_batch_map.get(&item.symbol).cloned();
        let twse_hit = twse_map.get(&item.symbol).map(|s| {
            (
                s.name.clone(),
                s.price,
                s.prev_close,
                s.open,
                s.high,
                s.low,
                s.volume,
                s.quote_time,
                s.exchange.clone(),
            )
        });
        let has_fast_hit = batch_hit.is_some() || twse_hit.as_ref().is_some_and(|t| t.1.is_some());
        let allow_network_fallback = if has_fast_hit {
            true
        } else if fallback_budget > 0 {
            fallback_budget -= 1;
            true
        } else {
            false
        };
        tasks.push((idx, item, batch_hit, twse_hit, allow_network_fallback));
    }

    let indexed: Vec<(usize, StockQuote)> = stream::iter(tasks)
        .map(
            |(idx, item, batch_hit, twse_hit, allow_network_fallback)| async move {
                if let Some(q) = batch_hit {
                    return (idx, q);
                }
                if !allow_network_fallback {
                    return (idx, error_quote(&item, "批量行情暂未返回"));
                }
                let quote = fetch_single_routed(&item, twse_hit, use_fast_batch).await;
                (idx, quote)
            },
        )
        .buffer_unordered(10)
        .collect()
        .await;

    let mut ordered = indexed;
    ordered.sort_by_key(|(idx, _)| *idx);
    ordered.into_iter().map(|(_, q)| q).collect()
}

type TwseTuple = (
    Option<String>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<u64>,
    Option<i64>,
    String,
);

async fn fetch_single_routed(
    item: &WatchItem,
    twse_hit: Option<TwseTuple>,
    skip_sparkline: bool,
) -> StockQuote {
    let market = if item.market.is_empty() {
        detect_market(&item.symbol)
    } else {
        item.market.as_str()
    };

    // Route A: A-shares & Hong Kong -> Tencent UTF-8 real-time (1-3s lag), fallback Yahoo.
    if matches!(market, "CN" | "HK") {
        if let Some(code) = to_tencent_code(&item.symbol) {
            if let Ok(q) = fetch_tencent_quote(&item.symbol, &item.name, &code, 48).await {
                return q;
            }
        }
        if let Ok(q) = fetch_yahoo_quote(&item.symbol, &item.name, 48).await {
            return q;
        }
        return error_quote(item, "行情源暂不可用");
    }

    // Route B: Taiwan -> Official TWSE/TPEx MIS real-time (merged with Yahoo sparkline when requested).
    if market == "TW" {
        let has_twse_price = twse_hit.as_ref().is_some_and(|t| t.1.is_some());
        let yahoo_res = if skip_sparkline && has_twse_price {
            Err(String::new())
        } else {
            fetch_yahoo_quote(&item.symbol, &item.name, 48).await
        };
        if let Some((tw_name, tw_price, tw_prev, tw_open, tw_high, tw_low, tw_vol, tw_ts, tw_ex)) =
            twse_hit
        {
            let mut base = yahoo_res.unwrap_or_else(|_| StockQuote {
                symbol: item.symbol.clone(),
                name: tw_name
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
                exchange: tw_ex.clone(),
                price: None,
                prev_close: None,
                change: None,
                change_percent: None,
                open: None,
                high: None,
                low: None,
                volume: None,
                market_cap: None,
                extended_price: None,
                extended_change_percent: None,
                extended_label: None,
                market_state: "已休市".into(),
                source: "TWSE 官方实时".into(),
                quote_time: None,
                sparkline: Vec::new(),
                error: None,
            });
            if let Some(p) = tw_price {
                base.price = Some(p);
                if let Some(pc) = tw_prev.or(base.prev_close) {
                    base.prev_close = Some(pc);
                    let ch = p - pc;
                    base.change = Some(ch);
                    if pc > 0.0 {
                        base.change_percent = Some((ch / pc) * 100.0);
                    }
                }
                base.source = "TWSE 官方实时".into();
            }
            if tw_open.is_some() {
                base.open = tw_open;
            }
            if tw_high.is_some() {
                base.high = tw_high;
            }
            if tw_low.is_some() {
                base.low = tw_low;
            }
            if tw_vol.is_some() {
                base.volume = tw_vol;
            }
            if tw_ts.is_some() {
                base.quote_time = tw_ts;
            }
            base.exchange = tw_ex;
            if item.name.is_empty() {
                if let Some(n) = tw_name {
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
// High-resolution single-symbol chart (1d / 5d / 1mo / 3mo / 1y)
// ---------------------------------------------------------------------------

pub async fn fetch_chart(symbol: &str, range: &str) -> Result<StockChartDetail, AppError> {
    let sym = symbol.trim();
    if sym.is_empty() {
        return Err(AppError::InvalidInput("symbol is required".into()));
    }
    let market = detect_market(sym);
    if range == "1d" && matches!(market, "CN" | "HK") {
        if let Some(code) = to_tencent_code(sym) {
            if let Ok(q) = fetch_tencent_quote(sym, "", &code, 260).await {
                if !q.sparkline.is_empty() {
                    return Ok(StockChartDetail {
                        symbol: sym.to_string(),
                        range: range.to_string(),
                        currency: q.currency,
                        prev_close: q.prev_close,
                        source: "腾讯分时".into(),
                        points: q.sparkline,
                    });
                }
            }
        }
    }

    let (meta, points) = fetch_yahoo_chart_raw(sym, range, false)
        .await
        .map_err(AppError::Message)?;
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
        points: downsample_points(points, 240),
    })
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
            "us" => {
                let clean = code.split('.').next().unwrap_or(code).to_ascii_uppercase();
                (clean, "US", "US")
            }
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
            http_client()
                .get(&tencent_url)
                .send()
                .await
                .ok()?
                .text()
                .await
                .ok()
        },
        async {
            http_client()
                .get(&yahoo_url)
                .send()
                .await
                .ok()?
                .json::<Value>()
                .await
                .ok()
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
            WatchItem {
                symbol: "NVDA".into(),
                name: "英伟达".into(),
                market: "US".into(),
            },
        );
        let q = parse_tencent_batch_line(line.as_bytes(), &map).expect("quote parsed");
        assert_eq!(q.symbol, "NVDA");
        assert_eq!(q.name, "英伟达");
        assert!((q.price.unwrap() - 125.53).abs() < 1e-6);
        assert!((q.change_percent.unwrap() - (-14.01)).abs() < 1e-6);
        assert!((q.market_cap.unwrap() - 3_031_172_910_000.0).abs() < 1e5);
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
}
