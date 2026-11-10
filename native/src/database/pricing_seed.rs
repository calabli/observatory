//! Migrated directly from CC Switch schema.rs; see migration-manifest.json.
use super::*;
impl Database {
    fn seed_model_pricing(conn: &Connection) -> Result<(), AppError> {
        let pricing_data = [
            // Claude Fable 5.1 / Mythos 5.1 (released 2026-09-01; the same price as Fable 5,
            // but the cache read is 0.025x = $0.25 instead of the $1 of Fable 5)
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
            // Claude Fable 5 (the new tier above Opus)
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
            // Claude Opus 5.5 (released 2026-09-23; the cache read is 0.05x = $0.20, neither the usual 0.1x
            // $0.40 nor the $0.50 of Opus 5; the fast mode $8/$40 is not in the table)
            ("claude-opus-5-5", "Claude Opus 5.5", "4", "20", "0.20", "5"),
            // Claude Opus 5 (the same price level as Opus 4.8; the fast mode $10/$50 is not in the table)
            ("claude-opus-5", "Claude Opus 5", "5", "25", "0.50", "6.25"),
            // The Claude 4.8 series
            (
                "claude-opus-4-8",
                "Claude Opus 4.8",
                "5",
                "25",
                "0.50",
                "6.25",
            ),
            // Claude Sonnet 5.5 (released 2026-09-28; the same price as Sonnet 5, but the cache read is
            // 0.05x = $0.10 instead of the $0.20 of Sonnet 5)
            (
                "claude-sonnet-5-5",
                "Claude Sonnet 5.5",
                "2",
                "10",
                "0.10",
                "2.50",
            ),
            // Claude Haiku 5.5 (released 2026-10-07; it is tiered by the prompt length and the table
            // only holds the standard tier up to 100k tokens, the $0.50/$2.50 tier above that is not in the table)
            (
                "claude-haiku-5-5",
                "Claude Haiku 5.5",
                "0.10",
                "0.50",
                "0.01",
                "0.125",
            ),
            // Claude Sonnet 5 (confirmed on the official pricing page in 2026-09: the $2/$10 introductory
            // price became the regular price, the planned rise to $3/$15 on 09-01 was cancelled)
            (
                "claude-sonnet-5",
                "Claude Sonnet 5",
                "2",
                "10",
                "0.20",
                "2.50",
            ),
            // The Claude 4.7 series
            (
                "claude-opus-4-7",
                "Claude Opus 4.7",
                "5",
                "25",
                "0.50",
                "6.25",
            ),
            // The Claude 4.6 series (the bare id row covers the log variants without a date suffix and has the same price as the dated rows)
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
            // The Claude 4.5 series
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
            // The Claude 4 series (legacy models)
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
            // The Claude 3.5 series
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
            // The GPT-6 series (Astra released 2026-09-04 with a 1.05M window; Sol / Luna released 2026-09-22)
            // Checked against the official price page + the model page + models.dev on 2026-09-23: the Standard
            // short context prices are recorded, with a cache read of 0.1x and a cache write of 1.25x the input
            // price. The >272K long context tiers (input and cache 2x, output 1.5x), the
            // Batch/Flex, the Fast mode and the regional surcharges cannot be expressed in this table and
            // are ignored just like for gpt-5.5.
            // The effort tiers low/medium/high/xhigh fall back to this row after the price lookup strips
            // the suffix; max is not in the strip list
            // (it would collide with the real *-max ids), so no extra suffix row is added.
            ("gpt-6-astra", "GPT-6 Astra", "10", "50", "1", "12.5"),
            // GPT-6.1 Sol: Standard short-context pricing; cached input is 0.05× input.
            // https://developers.openai.com/api/docs/models/gpt-6.1-sol
            ("gpt-6.1-sol", "GPT-6.1 Sol", "2", "10", "0.10", "2.50"),
            ("gpt-6-sol", "GPT-6 Sol", "2", "10", "0.20", "2.50"),
            ("gpt-6-luna", "GPT-6 Luna", "0.10", "0.50", "0.01", "0.125"),
            // The GPT-5.6 series (Sol / Terra / Luna, released 2026-06)
            // From the 5.6 family on a cache write costs 1.25x the input price (writing the cache was free for
            // the GPT models before, do not backfill the older series)
            // The 2026-09-06 audit: Sol switched to the promotional price 4/20/0.40/5 (the original text of the
            // OpenAI price page: "at least until 2026-11-21"),
            // the listed price is 5/30/0.50/6.25. The promotional price is recorded and does not go into the
            // exemption list: once the promotion is over and models.dev is updated the audit reports it
            // automatically.
            ("gpt-5.6-sol", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            // The 2026-07-30 OpenAI price cut: luna -80%, terra -20%, sol unchanged (the Fast mode 2x price is not in the table)
            ("gpt-5.6-terra", "GPT-5.6 Terra", "2", "12", "0.20", "2.50"),
            (
                "gpt-5.6-luna",
                "GPT-5.6 Luna",
                "0.20",
                "1.20",
                "0.02",
                "0.25",
            ),
            // GPT-5.6 Cyber (the cybersecurity model of the Daybreak program, it needs Trusted Access; the
            // official price page was checked on 2026-09-23).
            // The aliases gpt-daybreak-red-latest / gpt-daybreak-blue-latest currently point at gpt-5.6-cyber /
            // gpt-5.6-sol, and the official side states plainly that the price changes when an alias changes
            // its target, so the aliases are not in the table.
            (
                "gpt-5.6-cyber",
                "GPT-5.6 Cyber",
                "12.50",
                "75",
                "1.25",
                "15.625",
            ),
            // The bare name gpt-5.6 is the official alias of sol; the effort suffix matches the accounting
            // shape of the gpt-5.5 series.
            // The price lookup matches the id exactly first and strips the effort suffix afterwards, these
            // rows have to be repriced together with sol, otherwise the old price overrides the base row.
            ("gpt-5.6", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-low", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-medium", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-high", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-xhigh", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            ("gpt-5.6-minimal", "GPT-5.6 Sol", "4", "20", "0.40", "5"),
            // The GPT-5.5 series
            ("gpt-5.5", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-low", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-medium", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-high", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-xhigh", "GPT-5.5", "5", "30", "0.50", "0"),
            ("gpt-5.5-minimal", "GPT-5.5", "5", "30", "0.50", "0"),
            // The GPT-5.4 series
            ("gpt-5.4", "GPT-5.4", "2.50", "15", "0.25", "0"),
            ("gpt-5.4-mini", "GPT-5.4 Mini", "0.75", "4.50", "0.075", "0"),
            ("gpt-5.4-nano", "GPT-5.4 Nano", "0.20", "1.25", "0.02", "0"),
            // The GPT-5.2 series
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
            // The GPT-5.3 Codex series
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
            // The GPT-5.1 series
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
            // The GPT-5 series
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
            // The OpenAI Reasoning series
            ("o3", "OpenAI o3", "2", "8", "0.50", "0"),
            ("o4-mini", "OpenAI o4-mini", "1.10", "4.40", "0.275", "0"),
            // The GPT-4.1 series
            ("gpt-4.1", "GPT-4.1", "2", "8", "0.50", "0"),
            ("gpt-4.1-mini", "GPT-4.1 Mini", "0.40", "1.60", "0.10", "0"),
            ("gpt-4.1-nano", "GPT-4.1 Nano", "0.10", "0.40", "0.025", "0"),
            // The completion of the OpenAI text models (2026-09-23): the standard token prices of the
            // individual model pages.
            // Source: https://developers.openai.com/api/docs/models/<model_id>
            // The deprecated models were excluded according to /api/docs/deprecations; the tool call fees and
            // the multimodal prices are not included.
            // The Pro series offers no cache discount, so the unsupported cache prices are recorded as 0 just
            // like for o3-pro.
            // Intentionally not taken: chat-latest is a rolling alias (it changes the price when it changes
            // its target); gpt-rosalind-research only started to be billed on 2026-10-05 and only with
            // Trusted Access, taking it into the table early would bill the usage of the free period.
            ("gpt-5.5-pro", "GPT-5.5 Pro", "30", "180", "0", "0"),
            ("gpt-5.4-pro", "GPT-5.4 Pro", "30", "180", "0", "0"),
            ("gpt-5.2-pro", "GPT-5.2 Pro", "21", "168", "0", "0"),
            ("gpt-4o", "GPT-4o", "2.50", "10", "1.25", "0"),
            ("gpt-4o-mini", "GPT-4o Mini", "0.15", "0.60", "0.075", "0"),
            // The Gemini 4 series
            // Source: https://ai.google.dev/gemini-api/docs/pricing#gemini-4-argon
            // What is recorded is the introductory price of the standard tier (fetched on 2026-10-15): input 2.00
            // / output 10.00 / context cache read 0.10
            // (the cache only supports the implicit form and not an explicit write, cache_creation is recorded
            // as 0, which is the same definition as the other Gemini entries).
            // ⚠️ The introductory price expires on 2026-11-30 and the listed price 4.00/20.00/0.20 applies
            // from 2026-12-01.
            // After the expiry it has to be changed back with the seed + repair double write; it is
            // deliberately not put into audit-ignore.json,
            // the A section of the /jason-update-model audit reports the reminder after the expiry, the same
            // definition as for the 3.6/3.7/3.8 Flash.
            // The free tier is not available; the batch tier (input 1.00->2.00 / output 5.00->10.00) and the
            // Flex tier are not in the table,
            // the definition the repository uses today only records the standard tier.
            (
                "gemini-4-argon",
                "Gemini 4 Argon",
                "2.00",
                "10.00",
                "0.10",
                "0",
            ),
            // The Gemini 3.8 series (released 2026-09-02, 1M window)
            // The introductory price 0.75/3.75/0.075 runs until 2026-12-31 and the listed price 1.50/7.50/0.15 applies from 2027-01-01; the same definition as the 3.7 Flash, do not add an exemption.
            (
                "gemini-3.8-flash",
                "Gemini 3.8 Flash",
                "0.75",
                "3.75",
                "0.075",
                "0",
            ),
            // The Gemini 3.7 series
            // What is recorded is the introductory price (the official announcement + the ai.google.dev price
            // table + models.dev agree on all three sources).
            // ⚠️ The introductory price expires on 2026-12-31 and the listed price 1.50/7.50/0.15 applies from 2027-01-01 (the 3.6/3.8 Flash follow the same rule).
            // After the expiry it has to be changed back with the seed + repair double write; by then
            // models.dev will have been updated first and
            // the A section of the /jason-update-model audit reports this row as a reminder --
            // this row is therefore deliberately not put into audit-ignore.json, do not add an exemption
            // (it would suppress that reminder).
            (
                "gemini-3.7-flash",
                "Gemini 3.7 Flash",
                "0.75",
                "3.75",
                "0.075",
                "0",
            ),
            // The Gemini 3.6 series
            // The 2026-09-06 audit: the Google price page also changed the 3.6 Flash to the introductory price 0.75/3.75/0.075 (until 2026-12-31),
            // and the listed price 1.50/7.50/0.15 applies from 2027-01-01. The same definition as the 3.7/3.8 Flash, deliberately not put into audit-ignore.json.
            (
                "gemini-3.6-flash",
                "Gemini 3.6 Flash",
                "0.75",
                "3.75",
                "0.075",
                "0",
            ),
            // The Gemini 3.5 series
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
            // The Gemini 3.1 series
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
            // The Gemini 3 series
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
            // The Gemini 2.5 series
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
            // The Gemini 2.0 series
            (
                "gemini-2.0-flash",
                "Gemini 2.0 Flash",
                "0.10",
                "0.40",
                "0.025",
                "0",
            ),
            // The StepFun series: the CNY prices are converted at 1 USD ≈ 7.14 CNY and keep two decimals.
            // The official Step 5 Preview input / output / cache read: 7 / 20 / 0.70 CNY (the cache read was
            // raised from 0.35 CNY on 2026-10-14; the old users that already called it per usage are still
            // billed at 0.35 CNY, the table takes the current listed price).
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
            // ====== The domestic models (USD/1M tokens) ======
            // Doubao (ByteDance)
            // The Seed 2.1 series (the official list price of the Volcengine from 2026-06, the CNY prices
            // converted at ~7.14):
            //   pro   input 6 CNY / output 30 CNY / hit 1.2 CNY
            //   turbo input 3 CNY / output 15 CNY / hit 0.6 CNY
            // The "cache storage 0.017 CNY/M/hour" is a storage fee that is billed by time and has a
            // different definition than the cache_creation of this table (the price of a write per token),
            // so it is 0.
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
            // The DeepSeek series
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
            // ── The DeepSeek V4 series: peak/off peak dual tier pricing since 2026-08-16 16:00 UTC ──
            // The official price page (api-docs.deepseek.com/quick_start/pricing, the same in Chinese and
            // English) quotes USD directly,
            // so no CNY conversion is needed any more. The peak hours = 9:00-12:00 and 14:00-18:00 Beijing
            // time
            // (= UTC 01:00-04:00 and 06:00-10:00), 7h in total per day; the remaining 17h are the off-peak
            // tier.
            //
            // 🔴 This table has only one row per model and no time dimension, so it **uniformly records
            // the peak tier** (decided by Jason on 2026-08-18):
            //   ① The official wording is "the off-peak price is half of the peak price", so the peak tier
            //     is the base listed price;
            //   ② The peak hours are exactly the working hours of the Chinese users and the main time slot
            //     for AI programming.
            // The price=the usage at night and in the early morning is overestimated twofold. Do not
            // switch to the off-peak tier following the "pick the lower tier of the ladder" convention.
            //
            // input=the price of a cache miss, cache_read=the price of a cache hit; DeepSeek does not charge
            // a separate cache write -> 0.
            //
            // ── 2026-09-11: the V4 Flash is retired and all three ids are served by DeepSeek-V4.1-Flash ──
            // The original text of the official price page: the legacy names `deepseek-v4-flash` /
            // `deepseek-v4-flash-vision-exp`
            // are still accepted, but "the corresponding models have been retired", the request is served
            // by V4.1-Flash
            // and billed at the Flash price -> all three have the same price. The peak tier of V4.1 Flash
            // is 0.3/1.2/0.006 (the off-peak tier 0.15/0.6/0.003
            // is exactly half of it; models.dev records exactly the off-peak tier, so the A section of the
            // audit keeps reporting these rows, which is expected).
            // deepseek-flash is the only name the official side currently recommends and has to be listed
            // on its own: the prefix fallback of the price lookup is LIKE '{id}-%',
            // it only hits the longer rows and the short id does not match deepseek-v4-flash, a missing row
            // silently bills at 0.
            //
            // 🔴 deepseek-chat / deepseek-reasoner stay at the peak tier of the V4 Flash (rechecked on
            // 2026-09-11):
            // the official documentation site no longer has these two ids anywhere and the first party
            // entries of models.dev were deleted as well -- there is no authoritative source that proves
            // either "it follows the V4.1 Flash price cut" or "it is offline", so the old values are kept
            // following the principle of not changing anything without a source.
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
            // The third party upstreams (OpenCode Go, the deepseek/ namespace of OpenRouter, the Hermes
            // presets and the like) use the versioned
            // deepseek-v4.1-flash. The dotted version can neither be stripped down to deepseek-flash nor be
            // hit by the prefix fallback, which only matches the longer rows,
            // so without a separate row it shows up as "not priced" and is billed at 0
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
            // Some upstreams (Alibaba Bailian for example) return a 4 digit MMDD date variant. The
            // strip_model_date_suffix of the price
            // lookup only strips the ISO / the 8 digit YYYYMMDD / the 6 digit YYMMDD,
            // so the bare id cannot be stripped and the prefix fallback only matches the longer rows --
            // without an extra alias it silently bills at 0
            (
                "deepseek-v4-flash-0731",
                "DeepSeek V4 Flash",
                "0.3",
                "1.2",
                "0.006",
                "0",
            ),
            // The name of the old vision experiment, the official pricing page states plainly that it is
            // "still accepted, served by V4.1-Flash and billed at the Flash price".
            // The official install script up to 1.2.0 wrote this id and the existing providers still use
            // it; the prefix fallback does not match the shorter
            // deepseek-v4-flash, so without a separate row it silently bills at 0
            (
                "deepseek-v4-flash-vision-exp",
                "DeepSeek V4 Flash Vision Exp",
                "0.3",
                "1.2",
                "0.006",
                "0",
            ),
            // The peak tier of the V4 Pro is 1.32/3.96/0.044 (CNY 9/27/0.3). The official side withdrew the
            // "routing to V4.1 Flash from 09-14" announcement on 2026-09-12, note (2) of the pricing page:
            // the V4 Pro API is still offered after September 14 and the
            // billing stays the same (rechecked on 2026-09-15).
            // 🔴 Do not reprice early based on a vendor announcement that is not in effect: v3.20.3 once
            // released the wrong price 0.3/1.2/0.006 because of that.
            (
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "1.32",
                "3.96",
                "0.044",
                "0",
            ),
            // Kimi (Moonshot AI)
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
            // The HighSpeed acceleration tier = twice the price of the base model (the consistent Kimi model, the same as the K2 Turbo)
            (
                "kimi-k2.7-code-highspeed",
                "Kimi K2.7 Code HighSpeed",
                "1.90",
                "8.00",
                "0.38",
                "0",
            ),
            // The official price page of 2026-10-19: the cache write of the K3 is billed by TTL, the default 5 minute tier is $3 (the 1 hour tier is $6, the default tier is taken)
            ("kimi-k3", "Kimi K3", "3.00", "15.00", "0.30", "3.00"),
            // The bare name of the K3 in the Kimi For Coding package (without the kimi- prefix), the same as the standard list price
            ("k3", "Kimi K3", "3.00", "15.00", "0.30", "3.00"),
            // Tencent Hunyuan (the official CNY 1/4/0.25 converted at 1 USD ≈ 7.14; the Hy3 tiered pricing takes the lowest tier)
            ("hunyuan-hy3", "Hunyuan Hy3", "0.14", "0.56", "0.035", "0"),
            ("hy3", "Hunyuan Hy3", "0.14", "0.56", "0.035", "0"),
            // Hy4 preview: the official Guangzhou region CNY 6/18/0.3 (1823/130055, the version of 2026-09-11) converted at 7.14, no tiers
            (
                "hy4-preview",
                "Hunyuan Hy4 Preview",
                "0.84",
                "2.52",
                "0.042",
                "0",
            ),
            // The MiniMax series
            // The 2026-09-06 audit: the official pay-as-you-go price page
            // (platform.minimax.io/docs/guides/pricing-paygo)
            // M2 / M2.1 / M2.5 are all 0.3/1.2/0.03/0.375, models.dev agrees; the old values 0.27/0.95 and
            // 0.15 were entered by mistake early on.
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
            // GLM (Zhipu)
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
            // MiMo (Xiaomi)
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
            // The Qwen series (Alibaba)
            ("qwen3.8-max", "Qwen3.8 Max", "2", "6", "0.25", "2.50"),
            // 2026-09-06: the Alibaba international price page 0.15/0.47 over the whole range (0<Token≤1M),
            // a flat price without tiers;
            // for the two cache columns the official side only states "an unusual ratio" without a number,
            // so models.dev is taken (the same definition as qwen3.8-max)
            (
                "qwen3.8-flash",
                "Qwen3.8 Flash",
                "0.15",
                "0.47",
                "0.016",
                "0.20",
            ),
            // 2026-09-15: the two open weight models take the unit prices of the Alibaba international
            // (Singapore) model pages, without tiers; the two cache columns are
            // the implicit cache hit price and the explicit cache creation price, the same definition as
            // qwen3.8-max
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
            // 2026-10-19: the Alibaba international (Singapore) price page. The tiered pricing of
            // qwen3.7-flash takes the lowest tier (≤32K),
            // the two cache columns follow the same definition as qwen3.8 (an implicit hit of 20%, an
            // explicit creation of 125%); qwen3.6-27b has no tiers and
            // the price page lists no cache price; qwen3.8-omni-flash takes the text price, the price page
            // only lists the cache hit price
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
            // The Grok series (xAI)
            // 4.5/4.6/4.7 are all priced by tier: the unit price doubles when the prompt is ≥200K (4/12,
            // the cached price doubles as well).
            // This table has no tier column, so the base tier (<200K) is taken uniformly, the same
            // definition as the other vendors with tiers
            ("grok-4.7", "Grok 4.7", "2", "6", "0.50", "0"),
            ("grok-4.6", "Grok 4.6", "2", "6", "0.50", "0"),
            ("grok-4.5", "Grok 4.5", "2", "6", "0.30", "0"),
            // The internal alias that the Grok CLI reports in the modelUsage of the official OAuth state.
            // The pricing was derived from
            // costUsdTicks (1 tick = 1e-10 USD) with a measurement over two rounds: the input/output are
            // 2/6 like grok-4.5 and the cache read is 0.30 as well
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
            // The Mistral series
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
            // The Cohere series
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
            // The OpenAI additions
            ("o3-pro", "OpenAI o3-pro", "20", "80", "0", "0"),
            ("o3-mini", "OpenAI o3-mini", "1.10", "4.40", "0.55", "0"),
            ("o1", "OpenAI o1", "15", "60", "7.50", "0"),
            ("o1-mini", "OpenAI o1-mini", "0.55", "2.20", "0.55", "0"),
            ("codex-mini", "Codex Mini", "0.75", "3", "0.025", "0"),
            ("gpt-5-mini", "GPT-5 Mini", "0.25", "2", "0.025", "0"),
            ("gpt-5-nano", "GPT-5 Nano", "0.05", "0.40", "0.005", "0"),
            // MiMo 2.6: the official standard prices were checked on 2026-09-23, the unit is USD per million tokens.
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
            // The 2026-10-19 models.dev audit and price check: the official Kimi K3 price page lists a
            // cache write (the default 5 minute tier is $3),
            // it was entered as 0 early on; the cache read of the Step 5 Preview was raised from 0.35 CNY
            // to 0.70 CNY on 2026-10-14
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
            // The official pricing page of 2026-09-02 confirms that the $2/$10 introductory price of the
            // Sonnet 5 became the regular price and that the planned rise to
            // $3/$15 on 09-01 was cancelled: the rows that were seeded with the list price are changed
            // back to the regular price (the rows the user changed by hand do not match the old value and
            // stay untouched)
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
            // The 2026-08-13 models.dev audit and price check: the cached input of grok-4.5 is officially
            // listed at 0.30
            // (the current price table of docs.x.ai), which agrees with the measured billing of
            // grok-4.5-build; the rows that were
            // entered as 0.50 are corrected here. Note that 0.50 is the cached price of grok-4.6, do not
            // mix the two up
            (
                "grok-4.5", "Grok 4.5", "2", "6", "0.30", "0", "2", "6", "0.50", "0",
            ),
            // The 2026-07-30 OpenAI GPT-5.6 price cut: luna -80%, terra -20% (sol unchanged).
            // Two guards per tier: the main guard matches >=v3.19 (which already ran the 07-12 cache_write
            // correction),
            // the 0 state guard matches the <v3.19 users that came straight from the old version (their
            // cache_write is still the 0 of the old seed)
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
            // The 2026-07-31 models.dev audit and price check: after the release of the DeepSeek V4 the
            // chat/reasoner were lowered to the alias price
            // of the V4 Flash; the official standard tier of the MiniMax M3 is 0.3/1.2 (the old value
            // looks like it was the acceleration tier)
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
            // The 2026-07-12 GPT-5.6 family cache write = 1.25x the input price (the new rule from OpenAI
            // 5.6 on),
            // which corrects the 0 of the early seed; it only matches the rows the user has not changed
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
            // The 2026-06-10 full price check (the official list price of the vendors; the CNY prices
            // converted at ~7.14)
            // GLM 4.6/4.7: the old values were the discounted prices of a relay / of OpenRouter and are
            // unified to the official Z.ai ones (the same as glm-5/5.1)
            (
                "glm-4.7", "GLM-4.7", "0.6", "2.2", "0.11", "0", "0.39", "1.75", "0.04", "0",
            ),
            (
                "glm-4.6", "GLM-4.6", "0.6", "2.2", "0.11", "0", "0.28", "1.11", "0.03", "0",
            ),
            // Grok 4.20: xAI has cut the price 2/6 -> 1.25/2.50
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
            // The official output of the Kimi K2.5 is 3.00
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
            // Mistral Devstral 2 output 0.90 -> 2 (the same as the devstral-medium of this table)
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
            // Doubao Seed 2.0: the old price of lite was 3-4 times too high + the cache hit price was added for the whole family
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
            // MiMo: the permanent price cut on 5/27, the old value was the old price
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
            // Qwen: the official "implicit cache = 20% of the input" adds the cache hit price
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
            // On 2026-08-16 16:00 UTC the whole DeepSeek V4 family switched to the peak/off peak dual tier
            // pricing (this table uniformly records the peak tier,
            // the reason is in the comments of the DeepSeek V4 section of seed_model_pricing). The increase
            // is large:
            // flash 0.14/0.28/0.0028 -> 0.44/1.32/0.014; pro 0.435/0.87/0.003625 -> 1.32/3.96/0.044.
            //
            // 🔴 These five rows have to stay at the end of the array: the chat/reasoner entries of
            // 2026-07-31 above and the
            // v4-flash(cache_read 0.028->0.0028) / v4-pro(1.68/3.36->0.435/0.87) entries of 2026-07
            // first converge the various historical shapes onto the same old value, only then can a single
            // guard hit here.
            // If this group is moved in front of them, the old databases stop at the middle price and do
            // not move any further.
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
            // The 2026-09-06 audit. The entries below have to come after all the older entries above (the
            // chained guards, the order is locked by
            // tests.rs::model_pricing_seed_repairs_known_outdated_builtin_prices):
            // - gpt-5.6-sol: the old databases of <v3.19 first get the cache_write 0 raised to 6.25 by the
            //   07-12 entry and are then lowered to the promotional price by this entry
            // - minimax-m2.5: it goes through the 0.12->0.15 entry first and then to 0.30 through this entry
            // The Google 3.6 Flash switches to the introductory price 0.75/3.75/0.075 (until 2026-12-31, the
            // listed price is 1.50/7.50/0.15)
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
            // The OpenAI GPT-5.6 Sol promotional price 4/20/0.40/5 (at least until 2026-11-21); the bare name and the effort suffix rows are repriced together
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
            // The official MiniMax pay-as-you-go prices: M2 / M2.1 / M2.5 = 0.3/1.2/0.03/0.375
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
            // The 2026-09-11 audit: the DeepSeek V4 Flash is retired and the requests that hit
            // deepseek-v4-flash / -0731
            // are served by V4.1-Flash and billed at the Flash price (the official pricing page
            // quick_start/pricing),
            // the peak tier 0.44/1.32/0.014 -> 0.3/1.2/0.006.
            //
            // 🔴 It has to come after the five peak/off peak repricing rows of 2026-08-16 above: the old
            // databases first have to be pushed to
            // 0.44/1.32/0.014 by that group, only then can the guard of this group hit; moved in front of
            // it the old databases stop at 0.44 and do not move any further.
            // deepseek-chat / deepseek-reasoner are deliberately not in this group -- the official side
            // has taken them down completely and there is no authoritative source
            // that proves they follow the price cut, see the comments of the DeepSeek V4 section of
            // seed_model_pricing.
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
            // 2026-09-15: the V4 Pro -> Flash tier rollback of 09-11 that was executed too early is
            // withdrawn (the official side withdrew the migration announcement on 09-12,
            // the V4 Pro keeps being billed at the original price). v3.20.3 has already pushed the old
            // databases to 0.3/1.2/0.006, this entry restores the peak tier.
            // 🔴 The original 1.32->0.3 entry has to be deleted instead of kept: with both of them every
            // startup rewrites the values back and forth.
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
            // The standard prices were checked on 2026-09-23, only the records that still match the old built-in values are corrected.
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
        // An INSERT OR IGNORE runs on every startup and appends the new models incrementally; only the pricings that still equal the old built-in values are repaired.
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
