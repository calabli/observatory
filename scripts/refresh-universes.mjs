// Check the built-in S&P 500 and CSI 300 pools against their current constituents:
//   npm run universes:refresh            # show what was added and removed
//   npm run universes:refresh -- --write # apply it to src/markets/universes.json
// Sources (free, no key): Wikipedia's "List of S&P 500 companies" (GICS sectors) and
// Eastmoney's CSI 300 board list. Stocks already in a pool keep their curated Chinese
// name and sector; new ones get the source's name and a mapped sector, so review the
// added rows before committing. The Taiwan and Hong Kong pools are hand-picked by
// market cap rather than an index list, so they are not touched.
import fs from "node:fs";

const FILE = "src/markets/universes.json";
const write = process.argv.includes("--write");

const GICS = {
  Industrials: "工业",
  "Health Care": "医疗健康",
  "Information Technology": "信息技术",
  Utilities: "公用事业",
  Financials: "金融",
  Materials: "原材料",
  "Consumer Discretionary": "可选消费",
  "Real Estate": "房地产",
  "Communication Services": "通信服务",
  "Consumer Staples": "必选消费",
  Energy: "能源",
};

// Eastmoney industry names -> the CSI 300 pool's groups, first match wins.
const CSI_GROUPS = [
  [/银行|证券|保险|多元金融|信托|期货/, "银行与金融"],
  [/半导体|电子|元件|光学|消费电子|面板/, "电子与半导体"],
  [
    /通信|计算机|软件|IT服务|数字媒体|广告营销|传媒|游戏|影视|互联网/,
    "通信与软件",
  ],
  [/白酒|饮料|食品|乳品|调味|养殖|农业|农产品|饲料/, "白酒与食品"],
  [/光伏|电池|电力设备|电源设备|风电|电网|储能/, "新能源与电气"],
  [
    /家电|家居|零售|旅游|酒店|商贸|免税|纺织|服装|美容|教育|文娱用品/,
    "家电与消费",
  ],
  [/医药|医疗|生物|中药|制药/, "医药生物"],
  [/煤炭|石油|油气|电力|燃气|公用|水务/, "能源与公用"],
  [
    /有色|钢铁|普钢|特钢|化工|化学|石化|炼化|农化|能源金属|材料|金属|塑料|橡胶|玻璃|水泥/,
    "有色与化工材料",
  ],
  [
    /建筑|工程|基础建设|房屋建设|建材|房地产|铁路|公路|港口|航运|航空机场|物流|交运|运输/,
    "基建与交运",
  ],
  [
    /汽车|乘用车|商用车|机械|航空装备|航天|航海|军工|船舶|轨交|自动化|设备|零部件/,
    "汽车与高端制造",
  ],
];
const csiGroup = (industry) =>
  CSI_GROUPS.find(([re]) => re.test(industry))?.[1] ?? "汽车与高端制造";

/** GET JSON, retrying a few times: these free endpoints fail intermittently. */
async function get(url) {
  let last;
  for (let attempt = 0; attempt < 4; attempt++) {
    if (attempt) await new Promise((r) => setTimeout(r, 1500 * attempt));
    try {
      const res = await fetch(url, {
        headers: { "User-Agent": "observatory-universe-refresh" },
        redirect: "follow",
      });
      if (res.ok) return await res.json();
      last = new Error(`${url} returned HTTP ${res.status}`);
    } catch (e) {
      last = e;
    }
  }
  throw last;
}

async function sp500() {
  const url =
    "https://en.wikipedia.org/w/api.php?action=parse&page=List_of_S%26P_500_companies&prop=wikitext&section=1&format=json&formatversion=2";
  const text = (await get(url)).parse.wikitext;
  const rows = [];
  for (const block of text.split("\n|-")) {
    // Cells are usually one per line ("|| x"), but some rows put two on one line.
    const cells = block
      .replace(/\|\|/g, "\n||")
      .split("\n")
      .filter((l) => l.startsWith("|") && !l.startsWith("|}"))
      .map((l) => l.replace(/^\|+\s*/, "").trim())
      .filter(Boolean);
    // {{NyseSymbol|MMM}}, {{NasdaqSymbol|AAPL}}, {{BZX link|CBOE}} ...
    const symbol = cells[0]?.match(/\{\{[^|}]*\|\s*([A-Z.]+)\s*\}\}/)?.[1];
    const link = cells[1]?.match(/\[\[(?:[^|\]]*\|)?([^\]]+)\]\]/)?.[1];
    const sector = GICS[cells[2]?.trim()];
    if (symbol && link && sector) {
      // Yahoo uses a dash for share classes (BRK-B); the app converts for Tencent.
      rows.push([symbol.replace(".", "-"), link, "US", sector]);
    }
  }
  if (rows.length < 490)
    throw new Error(`Only ${rows.length} S&P 500 rows parsed`);
  return rows;
}

async function csi300() {
  const rows = [];
  for (let page = 1; page <= 5; page++) {
    const url = `https://push2delay.eastmoney.com/api/qt/clist/get?pn=${page}&pz=100&fs=b:BK0500&fields=f12,f13,f14,f100`;
    const data = (await get(url)).data;
    const items = Object.values(data?.diff ?? {});
    for (const it of items) {
      const code = String(it.f12);
      const suffix = it.f13 === 1 ? "SS" : /^[48]/.test(code) ? "BJ" : "SZ";
      rows.push([`${code}.${suffix}`, it.f14, "CN", csiGroup(it.f100 ?? "")]);
    }
    if (rows.length >= (data?.total ?? 0) || items.length === 0) break;
  }
  if (rows.length < 290)
    throw new Error(`Only ${rows.length} CSI 300 rows parsed`);
  return rows;
}

function merge(name, current, fresh) {
  const now = new Map(current.map((r) => [r[0], r]));
  const next = new Set(fresh.map((r) => r[0]));
  const removed = current.filter((r) => !next.has(r[0]));
  const added = fresh.filter((r) => !now.has(r[0]));
  console.log(
    `\n${name}: ${current.length} -> ${fresh.length} (${added.length} added, ${removed.length} removed)`,
  );
  for (const r of added) console.log(`  + ${r[0]}  ${r[1]}  [${r[3]}]`);
  for (const r of removed) console.log(`  - ${r[0]}  ${r[1]}`);
  // Keep the curated rows in their order, then append the new ones.
  return [...current.filter((r) => next.has(r[0])), ...added];
}

const universes = JSON.parse(fs.readFileSync(FILE, "utf8"));
/** Refresh one pool; if its source is down, keep the current list and say so. */
async function refresh(name, current, load) {
  try {
    return merge(name, current, await load());
  } catch (e) {
    console.log(`\n${name}: skipped, source unavailable (${e.message})`);
    return current;
  }
}

const updated = {
  ...universes,
  sp500: await refresh("S&P 500", universes.sp500, sp500),
  csi300: await refresh("CSI 300", universes.csi300, csi300),
};

if (!write) {
  console.log("\nDry run. Re-run with --write to update", FILE);
} else {
  // One row per line, the same layout as the hand-maintained file.
  const body = Object.entries(updated)
    .map(
      ([key, rows]) =>
        `  ${JSON.stringify(key)}: [\n${rows.map((r) => `    ${JSON.stringify(r)}`).join(",\n")}\n  ]`,
    )
    .join(",\n");
  fs.writeFileSync(FILE, `{\n${body}\n}\n`);
  console.log(`\nUpdated ${FILE}. Review the added rows' names and sectors.`);
}
