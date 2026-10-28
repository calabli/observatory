//! 把「本机 + 各远端设备」汇成一份总览。

use super::{storage, LanDeviceUsage, LanUsageOverview, LanUsageSummary};
use crate::database::Database;
use crate::error::AppError;

/// 本机在给定区间内的共享用量合计。
///
/// 直接复用 Dashboard 的统计口径（明细 + 按天汇总表），本机这边的历史因此和
/// 应用里看到的完全一致。
pub fn local_summary(
    db: &Database,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<LanUsageSummary, AppError> {
    let mut total = LanUsageSummary::default();
    for app_type in super::identity::share_settings(db).shared_app_types {
        let summary = db.get_usage_summary(start, end, Some(&app_type), None, None)?;
        // 本机的 UsageSummary 只给了成功率，成功条数按比例还原（够展示用）。
        let success_count = (summary.success_rate * summary.total_requests as f32).round() as u64;
        total = total.add(&LanUsageSummary {
            total_requests: summary.total_requests,
            total_cost: summary.total_cost.clone(),
            total_input_tokens: summary.total_input_tokens,
            total_output_tokens: summary.total_output_tokens,
            total_cache_creation_tokens: summary.total_cache_creation_tokens,
            total_cache_read_tokens: summary.total_cache_read_tokens,
            success_rate: summary.success_rate,
            real_total_tokens: summary.real_total_tokens,
            cache_hit_rate: summary.cache_hit_rate,
            success_count,
        });
    }
    Ok(total)
}

/// 局域网总览：本机 + 所有已登记远端设备 + 合计。
pub fn overview(
    db: &Database,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<LanUsageOverview, AppError> {
    let local = local_summary(db, start, end)?;
    let remote = storage::remote_summary(db, start, end)?;
    let combined = local.add(&remote);

    let mut devices = Vec::new();
    devices.push(LanDeviceUsage {
        device_id: super::identity::device_id(db)?,
        device_name: super::identity::device_name(db),
        is_local: true,
        summary: local.clone(),
    });
    devices.extend(storage::per_device_summaries(db, start, end)?);

    Ok(LanUsageOverview {
        local,
        remote,
        combined,
        devices,
    })
}
