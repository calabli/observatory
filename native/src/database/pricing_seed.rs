//! Migrated directly from CC Switch schema.rs; see migration-manifest.json.
use super::*;
impl Database {
    fn seed_model_pricing(conn: &Connection) -> Result<(), AppError> {
        let pricing_data = [
            // Claude Fable 5.1 / Mythos 5.1（2026-09-01 发布；同 Fable 5 价，
            // 但缓存读为 0.025x = $0.25，非 Fable 5 的 $1）
            (
                "claude-fable-5-1",
                "Claude Fable 5.1",
                "10",
                "50",
                "0.25",
                "12.50",
            ),
            (
                "claude-mythos-5-1",
                "Claude Mythos 5.1",
                "10",
                "50",
                "0.25",
                "12.50",
            ),
            // Claude Fable 5（Opus 之上的新档）
            (
                "claude-fable-5",
                "Claude Fable 5",
                "10",
                "50",
                "1.00",
                "12.50",
            ),
            (
                "claude-mythos-5",
                "Claude Mythos 5",
                "10",
                "50",
                "1.00",
                "12.50",
            ),
            // Claude Opus 5.5（2026-09-23 发布；缓存读为 0.05x = $0.20，非常规 0.1x 的
            // $0.40，也非 Opus 5 的 $0.50；fast mode $8/$40 不入表）
            ("claude-opus-5-5", "Claude Opus 5.5", "4", "20", "0.20", "5"),
            // Claude Opus 5（与 Opus 4.8 同价位；fast mode $10/$50 不入表）
            ("claude-opus-5", "Claude Opus 5", "5", "25", "0.50", "6.25"),
            // Claude 4.8 系列
            (
                "claude-opus-4-8",
                "Claude Opus 4.8",
                "5",
                "25",
                "0.50",
                "6.25",
            ),
            // Claude Sonnet 5.5（2026-09-28 发布；同 Sonnet 5 价，但缓存读为
            // 0.05x = $0.10，非 Sonnet 5 的 $0.20）
            (
                "claude-sonnet-5-5",
                "Claude Sonnet 5.5",
                "2",
                "10",
                "0.10",
                "2.50",
            ),
            // Claude Haiku 5.5（2026-10-07 发布；按提示词长度分档，表内只存
            // 10 万 token 以内的标准档，超过时 $0.50/$2.50 的高档不入表）
            (
                "claude-haiku-5-5",
                "Claude Haiku 5.5",
                "0.10",
                "0.50",
                "0.01",
                "0.125",
            ),
            // Claude Sonnet 5（官方定价页 2026-09 确认：$2/$10 介绍价转为正式价，
            // 原定 09-01 涨至 $3/$15 取消）
            (
                "claude-sonnet-5",
                "Claude Sonnet 5",
                "2",
                "10",
                "0.20",
                "2.50",
            ),
            // Claude 4.7 系列
            (
                "claude-opus-4-7",
                "Claude Opus 4.7",
                "5",
                "25",
                "0.50",
                "6.25",
            ),
            // Claude 4.6 系列（裸 id 行覆盖无日期后缀的日志变体，与 dated 行同价）
            (
                "claude-opus-4-6",
                "Claude Opus 4.6",
                "5",
                "25",
                "0.50",
                "6.25",
            ),
            (
                "claude-sonnet-4-6",
                "Claude Sonnet 4.6",
                "3",
                "15",
                "0.30",
                "3.75",
            ),
            (
                "claude-opus-4-6-20260206",
                "Claude Opus 4.6",
                "5",
                "25",
                "0.50",
                "6.25",
            ),
            (
                "claude-sonnet-4-6-20260217",
                "Claude Sonnet 4.6",
                "3",
                "15",
                "0.30",
                "3.75",
            ),
            // Claude 4.5 系列
            (
                "claude-opus-4-5-20251101",
                "Claude Opus 4.5",
                "5",
                "25",
                "0.50",
                "6.25",
            ),
            (
                "claude-sonnet-4-5-20250929",
                "Claude Sonnet 4.5",
                "3",
                "15",
                "0.30",
                "3.75",
            ),
            (
                "claude-haiku-4-5-20251001",
                "Claude Haiku 4.5",
                "1",
                "5",
                "0.10",
                "1.25",
            ),
            // Claude 4 系列 (Legacy Models)
            (
                "claude-opus-4-20250514",
                "Claude Opus 4",
                "15",
                "75",
                "1.50",
                "18.75",
            ),
            (
                "claude-opus-4-1-20250805",
                "Claude Opus 4.1",
                "15",
                "75",
                "1.50",
                "18.75",
            ),
            (
                "claude-sonnet-4-20250514",
                "Claude Sonnet 4",
                "3",
                "15",
                "0.30",
                "3.75",
            ),
            // Claude 3.5 系列
            (
                "claude-3-5-haiku-20241022",
                "Claude 3.5 Haiku",
                "0.80",
                "4",
                "0.08",
                "1",
            ),
            (
                "claude-3-5-sonnet-20241022",
                "Claude 3.5 Sonnet",
                "3",
                "15",
                "0.30",
                "3.75",
            ),
            // GPT-6 系列（Astra 2026-09-04 发布，1.05M 窗口；Sol / Luna 2026-09-22 发布）
            // 2026-09-23 核对官方价页 + 模型页 + models.dev：录入 Standard 短上下文价，
            // cache read 0.1×、cache write 1.25× 输入价。>272K 长上下文档（输入与缓存 2×、输出 1.5×）、
            // Batch/Flex、Fast mode、区域加价本表无法表达，与 gpt-5.5 同样忽略。
            // effort 档 low/medium/high/xhigh 由查价剥后缀回落到本行；max 不在剥离列表
            //（会与 *-max 真 id 撞名），不另加后缀行。
            ("gpt-6-astra", "GPT-6 Astra", "10", "50", "1", "12.5"),
            // GPT-6.1 Sol: Standard short-context pricing; cached input is 0.05× input.
            // https://developers.openai.com/api/docs/models/gpt-6.1-sol
            ("gpt-6.1-sol", "GPT-6.1 Sol", "2", "10", "0.10", "2.50"),
            ("gpt-6-sol", "GPT-6 Sol", "2", "10", "0.20", "2.50"),
            ("gpt-6-luna", "GPT-6 Luna", "0.10", "0.50", "0.01", "0.125"),
            // GPT-5.6 系列（Sol / Terra / Luna，2026-06 发布）
            // 5.6 家族起 cache write 收 1.25× 输入价（此前 GPT 模型写缓存免费，勿回填旧系列）
            // 2026-09-06 审计：Sol 改促销价 4/20/0.40/5（OpenAI 价页原文"至少持续到 2026-11-21"），
            // 挂牌价 5/30/0.50/6.25。录促销价、不进豁免表：促销结束 models.dev 更新后审计会自动报出。
            ("gpt-5.6-sol", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            // 2026-07-30 OpenAI 降价：luna -80%、terra -20%，sol 不变（Fast mode 2× 价不入表）
            ("gpt-5.6-terra", "GPT-5.6 Terra", "2", "12", "0.20", "2.50"),
            (
                "gpt-5.6-luna",
                "GPT-5.6 Luna",
                "0.20",
                "1.20",
                "0.02",
                "0.25",
            ),
            // GPT-5.6 Cyber（Daybreak 计划的网安模型，需 Trusted Access；2026-09-23 核对官方价页）。
            // 别名 gpt-daybreak-red-latest / gpt-daybreak-blue-latest 当前分别指向 gpt-5.6-cyber /
            // gpt-5.6-sol，官方明说别名改指向时价格随之改变，故别名不入表。
            (
                "gpt-5.6-cyber",
                "GPT-5.6 Cyber",
                "12.50",
                "75",
                "1.25",
                "15.625",
            ),
            // 裸名 gpt-5.6 是 sol 的官方别名；effort 后缀对齐 gpt-5.5 系列的记账形态。
            // 查价先精确匹配 id 再剥 effort 后缀，这些行必须与 sol 同步改价，否则旧价会压过基础行。
            ("gpt-5.6", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-low", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-medium", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-high", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-xhigh", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-minimal", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            // GPT-5.5 系列
            ("gpt-5.5", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-low", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-medium", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-high", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-xhigh", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-minimal", "GPT-5.5", "5", "30", "0.50", "0"),
            // GPT-5.4 系列
            ("gpt-5.4", "GPT-5.4", "2.50", "15", "0.25", "0"),
            ("gpt-5.4-mini", "GPT-5.4 Mini", "0.75", "4.50", "0.075", "0"),
            ("gpt-5.4-nano", "GPT-5.4 Nano", "0.20", "1.25", "0.02", "0"),
            // GPT-5.2 系列
            ("gpt-5.2", "GPT-5.2", "1.75", "14", "0.175", "0"),
            ("gpt-5.2-low", "GPT-5.2", "1.75", "14", "0.175", "0"),
            ("gpt-5.2-medium", "GPT-5.2", "1.75", "14", "0.175", "0"),
            ("gpt-5.2-high", "GPT-5.2", "1.75", "14", "0.175", "0"),
            ("gpt-5.2-xhigh", "GPT-5.2", "1.75", "14", "0.175", "0"),
            ("gpt-5.2-codex", "GPT-5.2 Codex", "1.75", "14", "0.175", "0"),
            (
                "gpt-5.2-codex-low",
                "GPT-5.2 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.2-codex-medium",
                "GPT-5.2 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.2-codex-high",
                "GPT-5.2 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.2-codex-xhigh",
                "GPT-5.2 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            // GPT-5.3 Codex 系列
            ("gpt-5.3-codex", "GPT-5.3 Codex", "1.75", "14", "0.175", "0"),
            (
                "gpt-5.3-codex-spark",
                "GPT-5.3 Codex Spark",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.3-codex-low",
                "GPT-5.3 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.3-codex-medium",
                "GPT-5.3 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.3-codex-high",
                "GPT-5.3 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            (
                "gpt-5.3-codex-xhigh",
                "GPT-5.3 Codex",
                "1.75",
                "14",
                "0.175",
                "0",
            ),
            // GPT-5.1 系列
            ("gpt-5.1", "GPT-5.1", "1.25", "10", "0.125", "0"),
            ("gpt-5.1-low", "GPT-5.1", "1.25", "10", "0.125", "0"),
            ("gpt-5.1-medium", "GPT-5.1", "1.25", "10", "0.125", "0"),
            ("gpt-5.1-high", "GPT-5.1", "1.25", "10", "0.125", "0"),
            ("gpt-5.1-minimal", "GPT-5.1", "1.25", "10", "0.125", "0"),
            ("gpt-5.1-codex", "GPT-5.1 Codex", "1.25", "10", "0.125", "0"),
            (
                "gpt-5.1-codex-mini",
                "GPT-5.1 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5.1-codex-max",
                "GPT-5.1 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5.1-codex-max-high",
                "GPT-5.1 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5.1-codex-max-xhigh",
                "GPT-5.1 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            // GPT-5 系列
            ("gpt-5", "GPT-5", "1.25", "10", "0.125", "0"),
            ("gpt-5-low", "GPT-5", "1.25", "10", "0.125", "0"),
            ("gpt-5-medium", "GPT-5", "1.25", "10", "0.125", "0"),
            ("gpt-5-high", "GPT-5", "1.25", "10", "0.125", "0"),
            ("gpt-5-minimal", "GPT-5", "1.25", "10", "0.125", "0"),
            ("gpt-5-codex", "GPT-5 Codex", "1.25", "10", "0.125", "0"),
            ("gpt-5-codex-low", "GPT-5 Codex", "1.25", "10", "0.125", "0"),
            (
                "gpt-5-codex-medium",
                "GPT-5 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5-codex-high",
                "GPT-5 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5-codex-mini",
                "GPT-5 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5-codex-mini-medium",
                "GPT-5 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gpt-5-codex-mini-high",
                "GPT-5 Codex",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            // OpenAI Reasoning 系列
            ("o3", "OpenAI o3", "2", "8", "0.50", "0"),
            ("o4-mini", "OpenAI o4-mini", "1.10", "4.40", "0.275", "0"),
            // GPT-4.1 系列
            ("gpt-4.1", "GPT-4.1", "2", "8", "0.50", "0"),
            ("gpt-4.1-mini", "GPT-4.1 Mini", "0.40", "1.60", "0.10", "0"),
            ("gpt-4.1-nano", "GPT-4.1 Nano", "0.10", "0.40", "0.025", "0"),
            // OpenAI 文本模型补全（2026-09-23）：各模型页的标准 token 价。
            // 来源：https://developers.openai.com/api/docs/models/<model_id>
            // 已按 /api/docs/deprecations 排除弃用模型；不含工具调用费及多模态价格。
            // Pro 系列未提供缓存折扣，与 o3-pro 一样将未支持的缓存价格记为 0。
            // 有意不收：chat-latest 是滚动别名（改指向即改价）；gpt-rosalind-research 官方
            // 2026-10-05 才开始计费且仅限 Trusted Access，提前入表会给免费期用量记账。
            ("gpt-5.5-pro", "GPT-5.5 Pro", "30", "180", "0", "0"),
            ("gpt-5.4-pro", "GPT-5.4 Pro", "30", "180", "0", "0"),
            ("gpt-5.2-pro", "GPT-5.2 Pro", "21", "168", "0", "0"),
            ("gpt-4o", "GPT-4o", "2.50", "10", "1.25", "0"),
            ("gpt-4o-mini", "GPT-4o Mini", "0.15", "0.60", "0.075", "0"),
            // Gemini 4 系列
            // 来源：https://ai.google.dev/gemini-api/docs/pricing#gemini-4-argon
            // 录的是标准档介绍价（2026-10-15 抓取）：输入 2.00 / 输出 10.00 / 上下文缓存读 0.10
            // （缓存仅支持隐式，不支持显式写入，cache_creation 记 0，与其他 Gemini 条目口径一致）。
            // ⚠️ 介绍价 2026-11-30 到期，2026-12-01 起恢复挂牌价 4.00/20.00/0.20。
            // 到期后需走 seed + repair 双写改回；刻意不进 audit-ignore.json，
            // 由 /jason-update-model 审计的 A 段在到期后报出提醒，口径同 3.6/3.7/3.8 Flash。
            // 免费层级不可用；批量档（输入 1.00→2.00 / 输出 5.00→10.00）与 Flex 档不入表，
            // 仓库现有口径仅录标准档。
            (
                "gemini-4-argon",
                "Gemini 4 Argon",
                "2.00",
                "10.00",
                "0.10",
                "0",
            ),
            // Gemini 3.8 系列（2026-09-02 发布，1M 窗口）
            // 介绍价 0.75/3.75/0.075 至 2026-12-31，2027-01-01 起挂牌价 1.50/7.50/0.15；口径同 3.7 Flash，勿加豁免。
            (
                "gemini-3.8-flash",
                "Gemini 3.8 Flash",
                "0.75",
                "3.75",
                "0.075",
                "0",
            ),
            // Gemini 3.7 系列
            // 录的是介绍价（官方公告 + ai.google.dev 价表 + models.dev 三源一致）。
            // ⚠️ 介绍价 2026-12-31 到期，2027-01-01 起恢复挂牌价 1.50/7.50/0.15（3.6/3.8 Flash 同此规则）。
            // 到期后需走 seed + repair 双写改回；届时 models.dev 会先更新，
            // /jason-update-model 审计的 A 段会自动报出这一行作为提醒——
            // 因此这一行刻意不进 audit-ignore.json，勿加豁免（会屏蔽掉该提醒）。
            (
                "gemini-3.7-flash",
                "Gemini 3.7 Flash",
                "0.75",
                "3.75",
                "0.075",
                "0",
            ),
            // Gemini 3.6 系列
            // 2026-09-06 审计：Google 价页已把 3.6 Flash 也改成介绍价 0.75/3.75/0.075（至 2026-12-31），
            // 2027-01-01 起恢复挂牌价 1.50/7.50/0.15。与 3.7/3.8 Flash 同口径，刻意不进 audit-ignore.json。
            (
                "gemini-3.6-flash",
                "Gemini 3.6 Flash",
                "0.75",
                "3.75",
                "0.075",
                "0",
            ),
            // Gemini 3.5 系列
            (
                "gemini-3.5-flash",
                "Gemini 3.5 Flash",
                "1.50",
                "9.00",
                "0.15",
                "0",
            ),
            (
                "gemini-3.5-flash-lite",
                "Gemini 3.5 Flash Lite",
                "0.30",
                "2.50",
                "0.03",
                "0",
            ),
            // Gemini 3.1 系列
            (
                "gemini-3.1-pro-preview",
                "Gemini 3.1 Pro Preview",
                "2",
                "12",
                "0.20",
                "0",
            ),
            (
                "gemini-3.1-flash-lite",
                "Gemini 3.1 Flash Lite",
                "0.25",
                "1.50",
                "0.025",
                "0",
            ),
            (
                "gemini-3.1-flash-lite-preview",
                "Gemini 3.1 Flash Lite Preview",
                "0.25",
                "1.50",
                "0.025",
                "0",
            ),
            // Gemini 3 系列
            (
                "gemini-3-pro-preview",
                "Gemini 3 Pro Preview",
                "2",
                "12",
                "0.2",
                "0",
            ),
            (
                "gemini-3-flash-preview",
                "Gemini 3 Flash Preview",
                "0.5",
                "3",
                "0.05",
                "0",
            ),
            // Gemini 2.5 系列
            (
                "gemini-2.5-pro",
                "Gemini 2.5 Pro",
                "1.25",
                "10",
                "0.125",
                "0",
            ),
            (
                "gemini-2.5-flash",
                "Gemini 2.5 Flash",
                "0.3",
                "2.5",
                "0.03",
                "0",
            ),
            (
                "gemini-2.5-flash-lite",
                "Gemini 2.5 Flash Lite",
                "0.10",
                "0.40",
                "0.01",
                "0",
            ),
            // Gemini 2.0 系列
            (
                "gemini-2.0-flash",
                "Gemini 2.0 Flash",
                "0.10",
                "0.40",
                "0.025",
                "0",
            ),
            // StepFun 系列：CNY 按 1 USD ≈ 7.14 CNY 折算，保留两位小数。
            // Step 5 Preview 官方输入 / 输出 / 缓存读取：7 / 20 / 0.70 元（缓存读取 2026-10-14
            // 起由 0.35 元上调；此前已按量调用过的老用户仍按 0.35 元计，表里取现行挂牌价）。
            (
                "step-5-preview",
                "Step 5 Preview",
                "0.98",
                "2.80",
                "0.10",
                "0",
            ),
            (
                "step-3.7-flash",
                "Step 3.7 Flash",
                "0.19",
                "1.13",
                "0.04",
                "0",
            ),
            (
                "step-3.5-flash",
                "Step 3.5 Flash",
                "0.10",
                "0.30",
                "0.02",
                "0",
            ),
            (
                "step-3.5-flash-2603",
                "Step 3.5 Flash 2603",
                "0.10",
                "0.30",
                "0.02",
                "0",
            ),
            // ====== 国产模型 (USD/1M tokens) ======
            // Doubao (字节跳动)
            // Seed 2.1 系列（2026-06 火山引擎官方 list 价，CNY 按 ~7.14 折算）：
            //   pro   输入 6 元 / 输出 30 元 / 命中 1.2 元
            //   turbo 输入 3 元 / 输出 15 元 / 命中 0.6 元
            // 「缓存存储 0.017 元/M/小时」是按时长计费的存储费，与本表 cache_creation（按 token 写入价）口径不同，置 0。
            (
                "doubao-seed-2-1-pro",
                "Doubao Seed 2.1 Pro",
                "0.84",
                "4.2",
                "0.17",
                "0",
            ),
            (
                "doubao-seed-2-1-turbo",
                "Doubao Seed 2.1 Turbo",
                "0.42",
                "2.1",
                "0.08",
                "0",
            ),
            (
                "doubao-seed-code",
                "Doubao Seed Code",
                "0.17",
                "1.11",
                "0.02",
                "0",
            ),
            (
                "doubao-seed-2-0-pro",
                "Doubao Seed 2.0 Pro",
                "0.47",
                "2.37",
                "0.09",
                "0",
            ),
            (
                "doubao-seed-2-0-code",
                "Doubao Seed 2.0 Code",
                "0.47",
                "2.37",
                "0.09",
                "0",
            ),
            (
                "doubao-seed-2-0-code-preview-latest",
                "Doubao Seed 2.0 Code Preview",
                "0.47",
                "2.37",
                "0.09",
                "0",
            ),
            (
                "doubao-seed-2-0-lite",
                "Doubao Seed 2.0 Lite",
                "0.08",
                "0.50",
                "0.017",
                "0",
            ),
            (
                "doubao-seed-2-0-mini",
                "Doubao Seed 2.0 Mini",
                "0.03",
                "0.31",
                "0.0056",
                "0",
            ),
            // DeepSeek 系列
            (
                "deepseek-v3.2",
                "DeepSeek V3.2",
                "0.28",
                "0.42",
                "0.028",
                "0",
            ),
            (
                "deepseek-v3.1",
                "DeepSeek V3.1",
                "0.55",
                "1.67",
                "0.055",
                "0",
            ),
            ("deepseek-v3", "DeepSeek V3", "0.28", "1.11", "0.028", "0"),
            // ── DeepSeek V4 系列：2026-08-16 16:00 UTC 起改为峰谷双档计价 ──
            // 官方价页（api-docs.deepseek.com/quick_start/pricing，中英一致）直接挂 USD，
            // 不再需要 CNY 折算。高峰时段 = 北京时间 9:00-12:00 与 14:00-18:00
            // （= UTC 01:00-04:00、06:00-10:00），共 7h/天；其余 17h 为空闲档。
            //
            // 🔴 本表每模型仅一行、无时段维度，**统一录高峰档**（Jason 2026-08-18 拍板）：
            //   ① 官方措辞是「空闲价为高峰价的一半」，高峰档才是基准挂牌价；
            //   ② 高峰时段正是中文用户的工作时间，是 AI 编程主力时段。
            //   代价=夜间/凌晨用量高估一倍。勿按「阶梯取低档」惯例改成空闲档。
            //
            // input=缓存未命中价，cache_read=缓存命中价；DeepSeek 不单收 cache write → 0。
            //
            // ── 2026-09-11：V4 Flash 退役，三个 id 全部由 DeepSeek-V4.1-Flash 承接 ──
            // 官方价页原文：legacy names `deepseek-v4-flash` / `deepseek-v4-flash-vision-exp`
            // 仍被接受，但「the corresponding models have been retired」，请求由 V4.1-Flash 服务
            // 并按 Flash 价计费 → 三者同价。V4.1 Flash 高峰档 0.3/1.2/0.006（空闲档 0.15/0.6/0.003
            // 恰为一半；models.dev 录的正是空闲档，故审计 A 段会长期报这几行，属预期）。
            // deepseek-flash 是官方当前唯一推荐名，必须单列：查价前缀兜底是 LIKE '{id}-%'，
            // 只命中更长的行，短 id 匹配不到 deepseek-v4-flash，缺行即静默按 0 计费。
            //
            // 🔴 deepseek-chat / deepseek-reasoner 停在 V4 Flash 高峰档不动（2026-09-11 复核）：
            // 官方文档站已全站搜不到这两个 id、models.dev 第一方条目也已删除 —— 无权威源可证
            // 「跟随 V4.1 Flash 降价」或「已下线」任一方向，按无源不动原则保留旧值。
            (
                "deepseek-chat",
                "DeepSeek Chat",
                "0.44",
                "1.32",
                "0.014",
                "0",
            ),
            (
                "deepseek-reasoner",
                "DeepSeek Reasoner",
                "0.44",
                "1.32",
                "0.014",
                "0",
            ),
            (
                "deepseek-flash",
                "DeepSeek V4.1 Flash",
                "0.3",
                "1.2",
                "0.006",
                "0",
            ),
            // 第三方上游（OpenCode Go、OpenRouter 的 deepseek/ 命名空间、Hermes 预设等）用带版本号的
            // deepseek-v4.1-flash。点号版本既剥不成 deepseek-flash，前缀兜底也只命中更长的行，
            // 不单列会显示「未定价」并按 0 计费
            (
                "deepseek-v4.1-flash",
                "DeepSeek V4.1 Flash",
                "0.3",
                "1.2",
                "0.006",
                "0",
            ),
            (
                "deepseek-v4-flash",
                "DeepSeek V4 Flash",
                "0.3",
                "1.2",
                "0.006",
                "0",
            ),
            // 部分上游（如阿里百炼）回传 4 位 MMDD 日期变体。查价的
            // strip_model_date_suffix 只剥 ISO / 8 位 YYYYMMDD / 6 位 YYMMDD，
            // 剥不到裸 id，前缀兜底也只匹配更长的行 —— 不补别名会静默按 0 计费
            (
                "deepseek-v4-flash-0731",
                "DeepSeek V4 Flash",
                "0.3",
                "1.2",
                "0.006",
                "0",
            ),
            // 旧视觉实验名，官方定价页明示「仍被接受、由 V4.1-Flash 承接并按 Flash 价计费」。
            // 官方安装脚本 ≤1.2.0 写过这个 id，存量供应商仍在用；前缀兜底匹配不到更短的
            // deepseek-v4-flash，不单列会静默按 0 计费
            (
                "deepseek-v4-flash-vision-exp",
                "DeepSeek V4 Flash Vision Exp",
                "0.3",
                "1.2",
                "0.006",
                "0",
            ),
            // V4 Pro 高峰档 1.32/3.96/0.044（CNY 9/27/0.3）。官方 2026-09-12 撤回了「09-14 起
            // 路由到 V4.1 Flash」的公告，定价页注(2)：9 月 14 日之后继续提供 V4 Pro API，
            // 计费方式保持不变（2026-09-15 复核）。
            // 🔴 勿按未生效的厂商公告提前改价：v3.20.3 曾因此发出 0.3/1.2/0.006 错价。
            (
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "1.32",
                "3.96",
                "0.044",
                "0",
            ),
            // Kimi (月之暗面)
            (
                "kimi-k2-thinking",
                "Kimi K2 Thinking",
                "0.55",
                "2.20",
                "0.10",
                "0",
            ),
            ("kimi-k2-0905", "Kimi K2", "0.55", "2.20", "0.10", "0"),
            (
                "kimi-k2-turbo",
                "Kimi K2 Turbo",
                "1.11",
                "8.06",
                "0.14",
                "0",
            ),
            ("kimi-k2.5", "Kimi K2.5", "0.60", "3.00", "0.10", "0"),
            ("kimi-k2.6", "Kimi K2.6", "0.95", "4.00", "0.16", "0"),
            (
                "kimi-k2.7-code",
                "Kimi K2.7 Code",
                "0.95",
                "4.00",
                "0.19",
                "0",
            ),
            // HighSpeed 加速档=本体 2 倍价（Kimi 官方一贯模式，同 K2 Turbo）
            (
                "kimi-k2.7-code-highspeed",
                "Kimi K2.7 Code HighSpeed",
                "1.90",
                "8.00",
                "0.38",
                "0",
            ),
            // 2026-10-19 官方价页：K3 缓存写入按 TTL 计费，默认 5 分钟档 $3（1 小时档 $6，取默认档）
            ("kimi-k3", "Kimi K3", "3.00", "15.00", "0.30", "3.00"),
            // Kimi For Coding 套餐里 K3 的裸名（无 kimi- 前缀），同标准 list 价
            ("k3", "Kimi K3", "3.00", "15.00", "0.30", "3.00"),
            // 腾讯混元 (Tencent Hunyuan)（官方 CNY 1/4/0.25 按 1 USD ≈ 7.14 折算；Hy3 阶梯计价取最低档）
            ("hunyuan-hy3", "Hunyuan Hy3", "0.14", "0.56", "0.035", "0"),
            ("hy3", "Hunyuan Hy3", "0.14", "0.56", "0.035", "0"),
            // Hy4 preview：官方广州地域 CNY 6/18/0.3（1823/130055，2026-09-11 版）按 7.14 折算，无阶梯
            (
                "hy4-preview",
                "Hunyuan Hy4 Preview",
                "0.84",
                "2.52",
                "0.042",
                "0",
            ),
            // MiniMax 系列
            // 2026-09-06 审计：官方按量价页（platform.minimax.io/docs/guides/pricing-paygo）
            // M2 / M2.1 / M2.5 均为 0.3/1.2/0.03/0.375，models.dev 一致；旧值 0.27/0.95 与 0.15 为早期误录。
            (
                "minimax-m2.1",
                "MiniMax M2.1",
                "0.30",
                "1.20",
                "0.03",
                "0.375",
            ),
            (
                "minimax-m2.1-lightning",
                "MiniMax M2.1 Lightning",
                "0.27",
                "2.33",
                "0.03",
                "0",
            ),
            ("minimax-m2", "MiniMax M2", "0.30", "1.20", "0.03", "0.375"),
            (
                "minimax-m2.5",
                "MiniMax M2.5",
                "0.30",
                "1.20",
                "0.03",
                "0.375",
            ),
            (
                "minimax-m2.5-lightning",
                "MiniMax M2.5 Lightning",
                "0.30",
                "2.40",
                "0.03",
                "0",
            ),
            (
                "minimax-m2.7",
                "MiniMax M2.7",
                "0.30",
                "1.20",
                "0.06",
                "0.375",
            ),
            (
                "minimax-m2.7-highspeed",
                "MiniMax M2.7 Highspeed",
                "0.60",
                "2.40",
                "0.06",
                "0.375",
            ),
            ("minimax-m3", "MiniMax M3", "0.30", "1.20", "0.06", "0"),
            (
                "minimax-m3.1-flash",
                "MiniMax M3.1 Flash",
                "0.15",
                "0.60",
                "0.015",
                "0",
            ),
            // GLM (智谱)
            ("glm-4.7", "GLM-4.7", "0.6", "2.2", "0.11", "0"),
            ("glm-4.6", "GLM-4.6", "0.6", "2.2", "0.11", "0"),
            ("glm-5", "GLM-5", "1", "3.2", "0.2", "0"),
            ("glm-5.1", "GLM-5.1", "1.4", "4.4", "0.26", "0"),
            ("glm-5.2", "GLM-5.2", "1.4", "4.4", "0.26", "0"),
            ("glm-5.3", "GLM-5.3", "1.4", "4.4", "0.26", "0"),
            (
                "glm-5.3-flash",
                "GLM-5.3-Flash",
                "0.15",
                "0.50",
                "0.03",
                "0",
            ),
            (
                "glm-5.3-flashx",
                "GLM-5.3-FlashX",
                "0.37",
                "1.25",
                "0.075",
                "0",
            ),
            ("glm-5-turbo", "GLM-5-Turbo", "1.2", "4", "0.24", "0"),
            ("glm-5v-turbo", "GLM-5V-Turbo", "1.2", "4", "0.24", "0"),
            // MiMo (小米)
            (
                "mimo-v2-flash",
                "MiMo V2 Flash",
                "0.09",
                "0.29",
                "0.009",
                "0",
            ),
            ("mimo-v2-pro", "MiMo V2 Pro", "0.435", "0.87", "0.0036", "0"),
            ("mimo-v2.5", "MiMo V2.5", "0.14", "0.28", "0.0028", "0"),
            (
                "mimo-v2.5-pro",
                "MiMo V2.5 Pro",
                "0.435",
                "0.87",
                "0.0036",
                "0",
            ),
            // Qwen 系列 (阿里巴巴)
            ("qwen3.8-max", "Qwen3.8 Max", "2", "6", "0.25", "2.50"),
            // 2026-09-06：阿里国际站价页 0.15/0.47 全区间（0<Token≤1M）平价、无阶梯；
            // 缓存两列官方只注明"非常规比例"未给数字，取 models.dev（与 qwen3.8-max 同口径）
            (
                "qwen3.8-flash",
                "Qwen3.8 Flash",
                "0.15",
                "0.47",
                "0.016",
                "0.20",
            ),
            // 2026-09-15：开放权重两款，取阿里国际站（新加坡）模型页单价，无阶梯；缓存两列为
            // 隐式缓存命中价与显式缓存创建价，与 qwen3.8-max 同口径
            (
                "qwen3.8-2.4t-a95b",
                "Qwen3.8 2.4T A95B",
                "2",
                "6",
                "0.25",
                "2.50",
            ),
            ("qwen3.8-27b", "Qwen3.8 27B", "0.50", "3", "0.10", "0.625"),
            ("qwen3.7-max", "Qwen3.7 Max", "2.50", "7.50", "0.25", "0"),
            ("qwen3.7-plus", "Qwen3.7 Plus", "0.40", "1.60", "0.08", "0"),
            // 2026-10-19：阿里国际站（新加坡）价页。qwen3.7-flash 阶梯计价取最低档（≤32K），
            // 缓存两列按 qwen3.8 同口径（隐式命中 20%、显式创建 125%）；qwen3.6-27b 无阶梯、
            // 价页未列缓存价；qwen3.8-omni-flash 取文本价，价页只列缓存命中价
            (
                "qwen3.7-flash",
                "Qwen3.7 Flash",
                "0.03",
                "0.13",
                "0.006",
                "0.0375",
            ),
            ("qwen3.6-27b", "Qwen3.6 27B", "0.60", "3.60", "0", "0"),
            (
                "qwen3.8-omni-flash",
                "Qwen3.8 Omni Flash",
                "0.15",
                "0.47",
                "0.016",
                "0",
            ),
            (
                "qwen3.6-plus",
                "Qwen3.6 Plus",
                "0.325",
                "1.95",
                "0.065",
                "0",
            ),
            (
                "qwen3.6-flash",
                "Qwen3.6 Flash",
                "0.1875",
                "1.125",
                "0.0375",
                "0",
            ),
            ("qwen3.5-plus", "Qwen3.5 Plus", "0.26", "1.56", "0.052", "0"),
            ("qwen3-max", "Qwen3 Max", "0.78", "3.90", "0", "0"),
            (
                "qwen3-235b-a22b",
                "Qwen3 235B-A22B",
                "0.70",
                "8.40",
                "0",
                "0",
            ),
            (
                "qwen3-coder-plus",
                "Qwen3 Coder Plus",
                "0.65",
                "3.25",
                "0.13",
                "0",
            ),
            (
                "qwen3-coder-480b",
                "Qwen3 Coder 480B",
                "0.65",
                "3.25",
                "0",
                "0",
            ),
            (
                "qwen3-coder-480b-a35b-instruct",
                "Qwen3 Coder 480B-A35B Instruct",
                "0.65",
                "3.25",
                "0",
                "0",
            ),
            (
                "qwen3-coder-flash",
                "Qwen3 Coder Flash",
                "0.195",
                "0.975",
                "0.039",
                "0",
            ),
            (
                "qwen3-coder-next",
                "Qwen3 Coder Next",
                "0.12",
                "0.75",
                "0",
                "0",
            ),
            ("qwq-plus", "QwQ Plus", "0.80", "2.40", "0", "0"),
            ("qwq-32b", "QwQ 32B", "0.20", "0.60", "0", "0"),
            ("qwen3-32b", "Qwen3 32B", "0.16", "0.64", "0", "0"),
            // Grok 系列 (xAI)
            // 4.5/4.6/4.7 均为分档计价：prompt ≥200K 时单价翻倍（4/12，cached 亦翻倍）。
            // 本表无档位列，统一取基础档（<200K），与其它分档厂商口径一致
            ("grok-4.7", "Grok 4.7", "2", "6", "0.50", "0"),
            ("grok-4.6", "Grok 4.6", "2", "6", "0.50", "0"),
            ("grok-4.5", "Grok 4.5", "2", "6", "0.30", "0"),
            // Grok CLI 官方 OAuth 态 modelUsage 上报的内部别名。定价由
            // costUsdTicks（1 tick = 1e-10 USD）双轮实测反推：input/output 与
            // grok-4.5 同为 2/6，cache read 同为 0.30
            ("grok-4.5-build", "Grok 4.5 Build", "2", "6", "0.30", "0"),
            ("grok-4.3", "Grok 4.3", "1.25", "2.50", "0.20", "0"),
            (
                "grok-4.20-0309-reasoning",
                "Grok 4.20 Reasoning",
                "1.25",
                "2.50",
                "0.20",
                "0",
            ),
            (
                "grok-4.20-0309-non-reasoning",
                "Grok 4.20",
                "1.25",
                "2.50",
                "0.20",
                "0",
            ),
            (
                "grok-4-1-fast-reasoning",
                "Grok 4.1 Fast Reasoning",
                "0.20",
                "0.50",
                "0.05",
                "0",
            ),
            (
                "grok-4-1-fast-non-reasoning",
                "Grok 4.1 Fast",
                "0.20",
                "0.50",
                "0.05",
                "0",
            ),
            ("grok-4", "Grok 4", "3", "15", "0.75", "0"),
            (
                "grok-code-fast-1",
                "Grok Build 0.1 (Code Fast Alias)",
                "1",
                "2",
                "0.20",
                "0",
            ),
            ("grok-build-0.1", "Grok Build 0.1", "1", "2", "0.20", "0"),
            ("grok-3", "Grok 3", "3", "15", "0.75", "0"),
            ("grok-3-mini", "Grok 3 Mini", "0.25", "0.50", "0.075", "0"),
            // Mistral 系列
            (
                "mistral-medium-3.5",
                "Mistral Medium 3.5",
                "1.50",
                "7.50",
                "0",
                "0",
            ),
            (
                "mistral-small-4",
                "Mistral Small 4",
                "0.10",
                "0.30",
                "0.01",
                "0",
            ),
            (
                "devstral-small-2-2512",
                "Devstral Small 2",
                "0.10",
                "0.30",
                "0.01",
                "0",
            ),
            (
                "magistral-small",
                "Magistral Small",
                "0.50",
                "1.50",
                "0",
                "0",
            ),
            ("codestral-2508", "Codestral", "0.30", "0.90", "0.03", "0"),
            (
                "devstral-small-1.1",
                "Devstral Small 1.1",
                "0.07",
                "0.28",
                "0.01",
                "0",
            ),
            ("devstral-2-2512", "Devstral 2", "0.40", "2", "0.04", "0"),
            (
                "devstral-medium",
                "Devstral Medium",
                "0.40",
                "2",
                "0.04",
                "0",
            ),
            (
                "mistral-large-3-2512",
                "Mistral Large 3",
                "0.50",
                "1.50",
                "0.05",
                "0",
            ),
            (
                "mistral-medium-3.1",
                "Mistral Medium 3.1",
                "0.40",
                "2",
                "0.04",
                "0",
            ),
            (
                "mistral-small-3.2-24b",
                "Mistral Small 3.2",
                "0.075",
                "0.20",
                "0.01",
                "0",
            ),
            ("magistral-medium", "Magistral Medium", "2", "5", "0", "0"),
            // Cohere 系列
            ("command-a", "Cohere Command A", "2.50", "10", "0", "0"),
            (
                "command-r-plus",
                "Cohere Command R+",
                "2.50",
                "10",
                "0",
                "0",
            ),
            ("command-r", "Cohere Command R", "0.15", "0.60", "0", "0"),
            // OpenAI 补充
            ("o3-pro", "OpenAI o3-pro", "20", "80", "0", "0"),
            ("o3-mini", "OpenAI o3-mini", "1.10", "4.40", "0.55", "0"),
            ("o1", "OpenAI o1", "15", "60", "7.50", "0"),
            ("o1-mini", "OpenAI o1-mini", "0.55", "2.20", "0.55", "0"),
            ("codex-mini", "Codex Mini", "0.75", "3", "0.025", "0"),
            ("gpt-5-mini", "GPT-5 Mini", "0.25", "2", "0.025", "0"),
            ("gpt-5-nano", "GPT-5 Nano", "0.05", "0.40", "0.005", "0"),
            // MiMo 2.6：2026-09-23 核对官方标准价格，单位 USD / 百万 tokens。
            (
                "mimo-v2.6-pro",
                "MiMo V2.6 Pro",
                "0.435",
                "0.87",
                "0.0036",
                "0",
            ),
            (
                "mimo-v2.6-flash",
                "MiMo V2.6 Flash",
                "0.14",
                "0.28",
                "0.0028",
                "0",
            ),
            (
                "mimo-v2.6-pro-ultraspeed",
                "MiMo V2.6 Pro UltraSpeed",
                "4.35",
                "8.7",
                "0.036",
                "0",
            ),
        ];

        let mut stmt = conn
            .prepare(
                "INSERT OR IGNORE INTO model_pricing (
                    model_id, display_name, input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(|e| AppError::Database(format!("准备模型定价语句失败: {e}")))?;
        for (model_id, display_name, input, output, cache_read, cache_creation) in pricing_data {
            stmt.execute(rusqlite::params![
                model_id,
                display_name,
                input,
                output,
                cache_read,
                cache_creation
            ])
            .map_err(|e| AppError::Database(format!("插入模型定价失败: {e}")))?;
        }

        log::info!("已插入 {} 条默认模型定价数据", pricing_data.len());
        Ok(())
    }

    fn repair_current_model_pricing(conn: &Connection) -> Result<(), AppError> {
        let pricing_fixes = [
            // 2026-10-19 models.dev 审计核价：Kimi K3 官方价页列有缓存写入（默认 5 分钟档 $3），
            // 早先按 0 录入；Step 5 Preview 缓存读取 2026-10-14 起由 0.35 元上调至 0.70 元
            (
                "kimi-k3", "Kimi K3", "3.00", "15.00", "0.30", "3.00", "3.00", "15.00", "0.30", "0",
            ),
            (
                "k3", "Kimi K3", "3.00", "15.00", "0.30", "3.00", "3.00", "15.00", "0.30", "0",
            ),
            (
                "step-5-preview",
                "Step 5 Preview",
                "0.98",
                "2.80",
                "0.10",
                "0",
                "0.98",
                "2.80",
                "0.05",
                "0",
            ),
            // 2026-09-02 官方定价页确认 Sonnet 5 $2/$10 介绍价转为正式价、原定 09-01 涨至
            // $3/$15 取消：早先按 list 价 seed 的行改回正式价（用户手改过的行不匹配旧值，不动）
            (
                "claude-sonnet-5",
                "Claude Sonnet 5",
                "2",
                "10",
                "0.20",
                "2.50",
                "3",
                "15",
                "0.30",
                "3.75",
            ),
            // 2026-08-13 models.dev 审计核价：grok-4.5 的 cached input 官方挂牌为 0.30
            // （docs.x.ai 现行价表），与 grok-4.5-build 的实测计费一致；早先按 0.50
            // 录入的行在此校正。注意 0.50 是 grok-4.6 的 cached 价，勿两者互串
            (
                "grok-4.5", "Grok 4.5", "2", "6", "0.30", "0", "2", "6", "0.50", "0",
            ),
            // 2026-07-30 OpenAI GPT-5.6 降价：luna -80%、terra -20%（sol 不变）。
            // 每档两条守卫：主守卫匹配 ≥v3.19（已跑过 07-12 cache_write 修正），
            // 0 态守卫匹配 <v3.19 直升用户（cache_write 仍为旧 seed 的 0）
            (
                "gpt-5.6-luna",
                "GPT-5.6 Luna",
                "0.20",
                "1.20",
                "0.02",
                "0.25",
                "1",
                "6",
                "0.10",
                "1.25",
            ),
            (
                "gpt-5.6-luna",
                "GPT-5.6 Luna",
                "0.20",
                "1.20",
                "0.02",
                "0.25",
                "1",
                "6",
                "0.10",
                "0",
            ),
            (
                "gpt-5.6-terra",
                "GPT-5.6 Terra",
                "2",
                "12",
                "0.20",
                "2.50",
                "2.50",
                "15",
                "0.25",
                "3.125",
            ),
            (
                "gpt-5.6-terra",
                "GPT-5.6 Terra",
                "2",
                "12",
                "0.20",
                "2.50",
                "2.50",
                "15",
                "0.25",
                "0",
            ),
            // 2026-07-31 models.dev 审计核价：DeepSeek V4 发布后 chat/reasoner 降为 V4 Flash
            // 别名价；MiniMax M3 官方 standard 档 0.3/1.2（旧值疑似录了加速档）
            (
                "deepseek-chat",
                "DeepSeek Chat",
                "0.14",
                "0.28",
                "0.0028",
                "0",
                "0.27",
                "1.10",
                "0.07",
                "0",
            ),
            (
                "deepseek-reasoner",
                "DeepSeek Reasoner",
                "0.14",
                "0.28",
                "0.0028",
                "0",
                "0.55",
                "2.19",
                "0.14",
                "0",
            ),
            (
                "minimax-m3",
                "MiniMax M3",
                "0.30",
                "1.20",
                "0.06",
                "0",
                "0.60",
                "2.40",
                "0.12",
                "0",
            ),
            // 2026-07-12 GPT-5.6 家族 cache write=1.25× 输入价（OpenAI 5.6 起的新规），
            // 修正早期 seed 的 0 值；只匹配未被用户改过的行
            (
                "gpt-5.6-sol",
                "GPT-5.6 Sol",
                "5",
                "30",
                "0.50",
                "6.25",
                "5",
                "30",
                "0.50",
                "0",
            ),
            (
                "gpt-5.6-terra",
                "GPT-5.6 Terra",
                "2.50",
                "15",
                "0.25",
                "3.125",
                "2.50",
                "15",
                "0.25",
                "0",
            ),
            (
                "gpt-5.6-luna",
                "GPT-5.6 Luna",
                "1",
                "6",
                "0.10",
                "1.25",
                "1",
                "6",
                "0.10",
                "0",
            ),
            // 2026-06-10 全量核价（厂商官方 list 价；CNY 按 ~7.14 折算）
            // GLM 4.6/4.7：旧值是中转/OpenRouter 折扣价，统一到 Z.ai 官方（与 glm-5/5.1 一致）
            (
                "glm-4.7", "GLM-4.7", "0.6", "2.2", "0.11", "0", "0.39", "1.75", "0.04", "0",
            ),
            (
                "glm-4.6", "GLM-4.6", "0.6", "2.2", "0.11", "0", "0.28", "1.11", "0.03", "0",
            ),
            // Grok 4.20：xAI 已降价 2/6 → 1.25/2.50
            (
                "grok-4.20-0309-reasoning",
                "Grok 4.20 Reasoning",
                "1.25",
                "2.50",
                "0.20",
                "0",
                "2",
                "6",
                "0.20",
                "0",
            ),
            (
                "grok-4.20-0309-non-reasoning",
                "Grok 4.20",
                "1.25",
                "2.50",
                "0.20",
                "0",
                "2",
                "6",
                "0.20",
                "0",
            ),
            // Kimi K2.5 官方 output 3.00
            (
                "kimi-k2.5",
                "Kimi K2.5",
                "0.60",
                "3.00",
                "0.10",
                "0",
                "0.60",
                "2.50",
                "0.10",
                "0",
            ),
            // MiniMax M2.5 input 0.15
            (
                "minimax-m2.5",
                "MiniMax M2.5",
                "0.15",
                "0.95",
                "0.03",
                "0",
                "0.12",
                "0.95",
                "0.03",
                "0",
            ),
            // Mistral Devstral 2 output 0.90 → 2（与同表 devstral-medium 一致）
            (
                "devstral-2-2512",
                "Devstral 2",
                "0.40",
                "2",
                "0.04",
                "0",
                "0.40",
                "0.90",
                "0.04",
                "0",
            ),
            // Doubao Seed 2.0：lite 旧价贵 3-4 倍 + 全系补 cache 命中价
            (
                "doubao-seed-2-0-lite",
                "Doubao Seed 2.0 Lite",
                "0.08",
                "0.50",
                "0.017",
                "0",
                "0.25",
                "2",
                "0",
                "0",
            ),
            (
                "doubao-seed-2-0-pro",
                "Doubao Seed 2.0 Pro",
                "0.47",
                "2.37",
                "0.09",
                "0",
                "0.47",
                "2.37",
                "0",
                "0",
            ),
            (
                "doubao-seed-2-0-code",
                "Doubao Seed 2.0 Code",
                "0.47",
                "2.37",
                "0.09",
                "0",
                "0.47",
                "2.37",
                "0",
                "0",
            ),
            (
                "doubao-seed-2-0-code-preview-latest",
                "Doubao Seed 2.0 Code Preview",
                "0.47",
                "2.37",
                "0.09",
                "0",
                "0.47",
                "2.37",
                "0",
                "0",
            ),
            (
                "doubao-seed-2-0-mini",
                "Doubao Seed 2.0 Mini",
                "0.03",
                "0.31",
                "0.0056",
                "0",
                "0.03",
                "0.31",
                "0",
                "0",
            ),
            // MiMo：5/27 永久降价，旧值是旧价
            (
                "mimo-v2-pro",
                "MiMo V2 Pro",
                "0.435",
                "0.87",
                "0.0036",
                "0",
                "1",
                "3",
                "0",
                "0",
            ),
            (
                "mimo-v2.5",
                "MiMo V2.5",
                "0.14",
                "0.29",
                "0.0028",
                "0",
                "0.09",
                "0.29",
                "0.009",
                "0",
            ),
            (
                "mimo-v2.5-pro",
                "MiMo V2.5 Pro",
                "0.435",
                "0.87",
                "0.0036",
                "0",
                "1",
                "3",
                "0",
                "0",
            ),
            // Qwen：官方"隐式缓存 = 输入 20%"补 cache 命中价
            (
                "qwen3.6-plus",
                "Qwen3.6 Plus",
                "0.325",
                "1.95",
                "0.065",
                "0",
                "0.325",
                "1.95",
                "0",
                "0",
            ),
            (
                "qwen3.5-plus",
                "Qwen3.5 Plus",
                "0.26",
                "1.56",
                "0.052",
                "0",
                "0.26",
                "1.56",
                "0",
                "0",
            ),
            (
                "qwen3-coder-plus",
                "Qwen3 Coder Plus",
                "0.65",
                "3.25",
                "0.13",
                "0",
                "0.65",
                "3.25",
                "0",
                "0",
            ),
            (
                "qwen3-coder-flash",
                "Qwen3 Coder Flash",
                "0.195",
                "0.975",
                "0.039",
                "0",
                "0.195",
                "0.975",
                "0",
                "0",
            ),
            (
                "deepseek-v4-flash",
                "DeepSeek V4 Flash",
                "0.14",
                "0.28",
                "0.0028",
                "0",
                "0.14",
                "0.28",
                "0.028",
                "0",
            ),
            (
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "0.435",
                "0.87",
                "0.003625",
                "0",
                "1.68",
                "3.36",
                "0.14",
                "0",
            ),
            (
                "glm-5", "GLM-5", "1", "3.2", "0.2", "0", "0.72", "2.30", "0", "0",
            ),
            (
                "glm-5.1", "GLM-5.1", "1.4", "4.4", "0.26", "0", "0.95", "3.15", "0", "0",
            ),
            (
                "grok-code-fast-1",
                "Grok Build 0.1 (Code Fast Alias)",
                "1",
                "2",
                "0.20",
                "0",
                "0.20",
                "1.50",
                "0.02",
                "0",
            ),
            // 2026-08-16 16:00 UTC DeepSeek V4 全系改峰谷双档计价（本表统一录高峰档，
            // 理由见 seed_model_pricing 里 DeepSeek V4 段的注释）。涨幅很大：
            // flash 0.14/0.28/0.0028 → 0.44/1.32/0.014；pro 0.435/0.87/0.003625 → 1.32/3.96/0.044。
            //
            // 🔴 这五条必须留在数组末尾：上面 2026-07-31 的 chat/reasoner 条目与
            // 2026-07 的 v4-flash(cache_read 0.028→0.0028) / v4-pro(1.68/3.36→0.435/0.87)
            // 条目会先把各种历史形态收敛到同一个旧值，这里才能单守卫命中。
            // 若把本组挪到它们之前，老库会停在中间价位不再前进。
            (
                "deepseek-chat",
                "DeepSeek Chat",
                "0.44",
                "1.32",
                "0.014",
                "0",
                "0.14",
                "0.28",
                "0.0028",
                "0",
            ),
            (
                "deepseek-reasoner",
                "DeepSeek Reasoner",
                "0.44",
                "1.32",
                "0.014",
                "0",
                "0.14",
                "0.28",
                "0.0028",
                "0",
            ),
            (
                "deepseek-v4-flash",
                "DeepSeek V4 Flash",
                "0.44",
                "1.32",
                "0.014",
                "0",
                "0.14",
                "0.28",
                "0.0028",
                "0",
            ),
            (
                "deepseek-v4-flash-0731",
                "DeepSeek V4 Flash",
                "0.44",
                "1.32",
                "0.014",
                "0",
                "0.14",
                "0.28",
                "0.0028",
                "0",
            ),
            (
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "1.32",
                "3.96",
                "0.044",
                "0",
                "0.435",
                "0.87",
                "0.003625",
                "0",
            ),
            // 2026-09-06 审计。以下条目须排在上方所有旧条目之后（链式守卫，顺序由
            // tests.rs::model_pricing_seed_repairs_known_outdated_builtin_prices 锁住）：
            // - gpt-5.6-sol：<v3.19 老库先经 07-12 条目把 cache_write 0 补成 6.25，再由本条降到促销价
            // - minimax-m2.5：先经 0.12→0.15 条目，再由本条到 0.30
            // Google 3.6 Flash 改介绍价 0.75/3.75/0.075（至 2026-12-31，挂牌 1.50/7.50/0.15）
            (
                "gemini-3.6-flash",
                "Gemini 3.6 Flash",
                "0.75",
                "3.75",
                "0.075",
                "0",
                "1.50",
                "7.50",
                "0.15",
                "0",
            ),
            // OpenAI GPT-5.6 Sol 促销价 4/20/0.40/5（至少到 2026-11-21）；裸名与 effort 后缀行同步
            (
                "gpt-5.6-sol",
                "GPT-5.6 Sol",
                "4",
                "20",
                "0.40",
                "5",
                "5",
                "30",
                "0.50",
                "6.25",
            ),
            (
                "gpt-5.6",
                "GPT-5.6 Sol",
                "4",
                "20",
                "0.40",
                "5",
                "5",
                "30",
                "0.50",
                "6.25",
            ),
            (
                "gpt-5.6-low",
                "GPT-5.6 Sol",
                "4",
                "20",
                "0.40",
                "5",
                "5",
                "30",
                "0.50",
                "6.25",
            ),
            (
                "gpt-5.6-medium",
                "GPT-5.6 Sol",
                "4",
                "20",
                "0.40",
                "5",
                "5",
                "30",
                "0.50",
                "6.25",
            ),
            (
                "gpt-5.6-high",
                "GPT-5.6 Sol",
                "4",
                "20",
                "0.40",
                "5",
                "5",
                "30",
                "0.50",
                "6.25",
            ),
            (
                "gpt-5.6-xhigh",
                "GPT-5.6 Sol",
                "4",
                "20",
                "0.40",
                "5",
                "5",
                "30",
                "0.50",
                "6.25",
            ),
            (
                "gpt-5.6-minimal",
                "GPT-5.6 Sol",
                "4",
                "20",
                "0.40",
                "5",
                "5",
                "30",
                "0.50",
                "6.25",
            ),
            // MiniMax 官方按量价：M2 / M2.1 / M2.5 = 0.3/1.2/0.03/0.375
            (
                "minimax-m2",
                "MiniMax M2",
                "0.30",
                "1.20",
                "0.03",
                "0.375",
                "0.27",
                "0.95",
                "0.03",
                "0",
            ),
            (
                "minimax-m2.1",
                "MiniMax M2.1",
                "0.30",
                "1.20",
                "0.03",
                "0.375",
                "0.27",
                "0.95",
                "0.03",
                "0",
            ),
            (
                "minimax-m2.5",
                "MiniMax M2.5",
                "0.30",
                "1.20",
                "0.03",
                "0.375",
                "0.15",
                "0.95",
                "0.03",
                "0",
            ),
            // 2026-09-11 审计：DeepSeek V4 Flash 退役，打到 deepseek-v4-flash / -0731 的
            // 请求已由 V4.1-Flash 承接并按 Flash 价计费（官方定价页 quick_start/pricing），
            // 高峰档 0.44/1.32/0.014 → 0.3/1.2/0.006。
            //
            // 🔴 必须排在上方 2026-08-16 峰谷调价五条之后：老库要先被那一组推到
            // 0.44/1.32/0.014，本组的守卫才能命中；挪到其前老库会停在 0.44 不再前进。
            // deepseek-chat / deepseek-reasoner 刻意不在本组 —— 官方已全面下架、无权威源
            // 可证其跟随降价，见 seed_model_pricing 的 DeepSeek V4 段注释。
            (
                "deepseek-v4-flash",
                "DeepSeek V4 Flash",
                "0.3",
                "1.2",
                "0.006",
                "0",
                "0.44",
                "1.32",
                "0.014",
                "0",
            ),
            (
                "deepseek-v4-flash-0731",
                "DeepSeek V4 Flash",
                "0.3",
                "1.2",
                "0.006",
                "0",
                "0.44",
                "1.32",
                "0.014",
                "0",
            ),
            // 2026-09-15：撤销 09-11 提前执行的 V4 Pro → Flash 档回调（官方 09-12 撤回迁移公告，
            // V4 Pro 继续按原价计费）。v3.20.3 已把老库推到 0.3/1.2/0.006，本条修回高峰档。
            // 🔴 原 1.32→0.3 条目必须删除而非保留：两条并存会让每次启动都来回改写。
            (
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "1.32",
                "3.96",
                "0.044",
                "0",
                "0.3",
                "1.2",
                "0.006",
                "0",
            ),
            // 2026-09-23 核对标准价格，仅修正仍匹配旧内置值的记录。
            (
                "mimo-v2.5",
                "MiMo V2.5",
                "0.14",
                "0.28",
                "0.0028",
                "0",
                "0.14",
                "0.29",
                "0.0028",
                "0",
            ),
            (
                "o3-mini",
                "OpenAI o3-mini",
                "1.10",
                "4.40",
                "0.55",
                "0",
                "0.55",
                "2.20",
                "0.55",
                "0",
            ),
        ];

        for (
            model_id,
            display_name,
            input,
            output,
            cache_read,
            cache_creation,
            old_input,
            old_output,
            old_cache_read,
            old_cache_creation,
        ) in pricing_fixes
        {
            conn.execute(
                "UPDATE model_pricing SET
                    display_name = ?2,
                    input_cost_per_million = ?3,
                    output_cost_per_million = ?4,
                    cache_read_cost_per_million = ?5,
                    cache_creation_cost_per_million = ?6
                 WHERE model_id = ?1
                   AND input_cost_per_million = ?7
                   AND output_cost_per_million = ?8
                   AND cache_read_cost_per_million = ?9
                   AND cache_creation_cost_per_million = ?10",
                rusqlite::params![
                    model_id,
                    display_name,
                    input,
                    output,
                    cache_read,
                    cache_creation,
                    old_input,
                    old_output,
                    old_cache_read,
                    old_cache_creation
                ],
            )
            .map_err(|e| AppError::Database(format!("修复模型 {model_id} 定价失败: {e}")))?;
        }

        Ok(())
    }

    pub fn ensure_model_pricing_seeded(&self) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        Self::ensure_model_pricing_seeded_on_conn(&conn)
    }

    pub(crate) fn ensure_model_pricing_seeded_on_conn(conn: &Connection) -> Result<(), AppError> {
        // 每次启动都执行 INSERT OR IGNORE，增量追加新模型；仅修复仍等于旧内置值的定价。
        Self::seed_model_pricing(conn)?;
        Self::repair_current_model_pricing(conn)
    }

    pub(super) fn create_lan_usage_tables(conn: &Connection) -> Result<(), AppError> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS lan_usage_snapshots (
            device_id TEXT PRIMARY KEY, payload TEXT NOT NULL
        )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS lan_devices (
                device_id TEXT PRIMARY KEY,
                device_name TEXT NOT NULL DEFAULT '',
                host TEXT NOT NULL DEFAULT '',
                port INTEGER NOT NULL DEFAULT 17890,
                enabled INTEGER NOT NULL DEFAULT 1,
                auto_sync INTEGER NOT NULL DEFAULT 1,
                last_seen_at INTEGER,
                last_sync_at INTEGER,
                last_error TEXT,
                created_at INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS lan_usage_events (
                device_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                data_source TEXT NOT NULL DEFAULT '',
                app_type TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '',
                session_id TEXT,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                input_token_semantics INTEGER NOT NULL DEFAULT 0,
                input_cost_usd TEXT NOT NULL DEFAULT '0',
                output_cost_usd TEXT NOT NULL DEFAULT '0',
                cache_read_cost_usd TEXT NOT NULL DEFAULT '0',
                cache_creation_cost_usd TEXT NOT NULL DEFAULT '0',
                total_cost_usd TEXT NOT NULL DEFAULT '0',
                status_code INTEGER NOT NULL DEFAULT 0,
                latency_ms INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL DEFAULT 0,
                synced_at INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (device_id, request_id)
            )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_lan_usage_events_device_time
             ON lan_usage_events(device_id, created_at)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_lan_usage_events_time
             ON lan_usage_events(created_at)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS lan_usage_daily (
                device_id TEXT NOT NULL,
                date TEXT NOT NULL,
                app_type TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '',
                request_count INTEGER NOT NULL DEFAULT 0,
                success_count INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                input_token_semantics INTEGER NOT NULL DEFAULT 0,
                total_cost_usd TEXT NOT NULL DEFAULT '0',
                avg_latency_ms INTEGER NOT NULL DEFAULT 0,
                synced_at INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (device_id, date, app_type, model)
            )",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_lan_usage_daily_date
             ON lan_usage_daily(date)",
            [],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        Ok(())
    }
}
