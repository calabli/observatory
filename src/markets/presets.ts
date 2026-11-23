import universes from "./universes.json";

export interface PresetItem {
  symbol: string;
  name: string;
  market: "US" | "TW" | "CN" | "HK" | "GLOBAL";
  sector: string;
  /** Optional approximate market cap in USD billions (used when feed omits marketCap, e.g. TWSE MIS). */
  capHint?: number;
}

export interface MarketPreset {
  id: string;
  label: string;
  description: string;
  items: PresetItem[];
}

type RawTuple = [
  string,
  string,
  "US" | "TW" | "CN" | "HK" | "GLOBAL",
  string,
  number?,
];

function unpack(rows: RawTuple[]): PresetItem[] {
  return rows.map(([symbol, name, market, sector, capHint]) =>
    capHint != null
      ? { symbol, name, market, sector, capHint }
      : { symbol, name, market, sector },
  );
}

const sp500Items = unpack(universes.sp500 as RawTuple[]);
const csi300Items = unpack(universes.csi300 as RawTuple[]);
const tw150Items = unpack(universes.tw150 as RawTuple[]);
const hk100Items = unpack(universes.hk100 as RawTuple[]);

const AI_GLOBAL_ITEMS: PresetItem[] = [
  // 算力 GPU 与 ASIC (8)
  { symbol: "NVDA", name: "英伟达", market: "US", sector: "算力芯片" },
  { symbol: "AVGO", name: "博通", market: "US", sector: "算力芯片" },
  { symbol: "AMD", name: "AMD", market: "US", sector: "算力芯片" },
  { symbol: "QCOM", name: "高通", market: "US", sector: "算力芯片" },
  { symbol: "ARM", name: "ARM", market: "US", sector: "算力芯片" },
  { symbol: "MRVL", name: "迈威尔", market: "US", sector: "算力芯片" },
  { symbol: "688256.SS", name: "寒武纪", market: "CN", sector: "算力芯片" },
  { symbol: "688041.SS", name: "海光信息", market: "CN", sector: "算力芯片" },

  // 晶圆代工与设备 (9)
  {
    symbol: "2330.TW",
    name: "台积电(台)",
    market: "TW",
    sector: "代工与设备",
    capHint: 880,
  },
  { symbol: "TSM", name: "台积电 ADR", market: "US", sector: "代工与设备" },
  { symbol: "ASML", name: "阿斯麦", market: "US", sector: "代工与设备" },
  { symbol: "AMAT", name: "应用材料", market: "US", sector: "代工与设备" },
  { symbol: "LRCX", name: "泛林集团", market: "US", sector: "代工与设备" },
  { symbol: "KLAC", name: "科磊", market: "US", sector: "代工与设备" },
  {
    symbol: "688981.SS",
    name: "中芯国际(A)",
    market: "CN",
    sector: "代工与设备",
  },
  { symbol: "002371.SZ", name: "北方华创", market: "CN", sector: "代工与设备" },
  {
    symbol: "0981.HK",
    name: "中芯国际(港)",
    market: "HK",
    sector: "代工与设备",
  },

  // 存储与光模块 (8)
  { symbol: "MU", name: "美光科技", market: "US", sector: "存储与光模块" },
  { symbol: "STX", name: "希捷", market: "US", sector: "存储与光模块" },
  { symbol: "WDC", name: "西部数据", market: "US", sector: "存储与光模块" },
  { symbol: "ALAB", name: "Astera Labs", market: "US", sector: "存储与光模块" },
  {
    symbol: "300308.SZ",
    name: "中际旭创",
    market: "CN",
    sector: "存储与光模块",
  },
  { symbol: "300502.SZ", name: "新易盛", market: "CN", sector: "存储与光模块" },
  {
    symbol: "300394.SZ",
    name: "天孚通信",
    market: "CN",
    sector: "存储与光模块",
  },
  {
    symbol: "603986.SS",
    name: "兆易创新",
    market: "CN",
    sector: "存储与光模块",
  },

  // AI 服务器与液冷 (9)
  { symbol: "DELL", name: "戴尔科技", market: "US", sector: "服务器与液冷" },
  { symbol: "VRT", name: "维谛技术", market: "US", sector: "服务器与液冷" },
  { symbol: "SMCI", name: "超微电脑", market: "US", sector: "服务器与液冷" },
  {
    symbol: "2317.TW",
    name: "鸿海",
    market: "TW",
    sector: "服务器与液冷",
    capHint: 84,
  },
  {
    symbol: "2382.TW",
    name: "广达",
    market: "TW",
    sector: "服务器与液冷",
    capHint: 36,
  },
  {
    symbol: "2308.TW",
    name: "台达电",
    market: "TW",
    sector: "服务器与液冷",
    capHint: 35,
  },
  {
    symbol: "6669.TW",
    name: "纬颖",
    market: "TW",
    sector: "服务器与液冷",
    capHint: 16,
  },
  {
    symbol: "3017.TW",
    name: "奇鋐",
    market: "TW",
    sector: "服务器与液冷",
    capHint: 9.0,
  },
  {
    symbol: "601138.SS",
    name: "工业富联",
    market: "CN",
    sector: "服务器与液冷",
  },

  // 大模型与云巨头 (9)
  { symbol: "AAPL", name: "苹果", market: "US", sector: "大模型与云" },
  { symbol: "MSFT", name: "微软", market: "US", sector: "大模型与云" },
  { symbol: "GOOGL", name: "谷歌", market: "US", sector: "大模型与云" },
  { symbol: "AMZN", name: "亚马逊", market: "US", sector: "大模型与云" },
  { symbol: "META", name: "Meta", market: "US", sector: "大模型与云" },
  { symbol: "ORCL", name: "甲骨文", market: "US", sector: "大模型与云" },
  { symbol: "0700.HK", name: "腾讯控股", market: "HK", sector: "大模型与云" },
  { symbol: "9988.HK", name: "阿里巴巴", market: "HK", sector: "大模型与云" },
  { symbol: "9888.HK", name: "百度集团", market: "HK", sector: "大模型与云" },

  // AI 应用与机器人 (6)
  { symbol: "TSLA", name: "特斯拉", market: "US", sector: "AI 应用" },
  { symbol: "PLTR", name: "Palantir", market: "US", sector: "AI 应用" },
  { symbol: "NOW", name: "ServiceNow", market: "US", sector: "AI 应用" },
  { symbol: "APP", name: "AppLovin", market: "US", sector: "AI 应用" },
  { symbol: "688111.SS", name: "金山办公", market: "CN", sector: "AI 应用" },
  { symbol: "002230.SZ", name: "科大讯飞", market: "CN", sector: "AI 应用" },
];

export const MARKET_PRESETS: MarketPreset[] = [
  {
    id: "us_sp500",
    label: `标普 500 (${sp500Items.length})`,
    description: `标普 500 全部 ${sp500Items.length} 家成分股，覆盖 11 大 GICS 官方行业，点击任意板块标题可全屏放大`,
    items: sp500Items,
  },
  {
    id: "cn_csi300",
    label: `沪深 300 (${csi300Items.length})`,
    description: `沪深 300 全部 ${csi300Items.length} 家成分股，覆盖主要行业板块（腾讯批量实时）`,
    items: csi300Items,
  },
  {
    id: "tw_top150",
    label: `台股 Top ${tw150Items.length}`,
    description: `台湾证交所上市市值前 ${tw150Items.length} 大企业，官方产业分类 + TWSE 实时批量行情`,
    items: tw150Items,
  },
  {
    id: "hk_core",
    label: `港股核心 (${hk100Items.length})`,
    description: `恒生指数 + 恒生科技 + 港股通前 ${hk100Items.length} 家权重蓝筹`,
    items: hk100Items,
  },
  {
    id: "ai",
    label: `AI 全球产业链 (${AI_GLOBAL_ITEMS.length})`,
    description:
      "横跨美 / 台 / A / 港四地的 AI 全产业链龙头：算力芯片、代工设备、存储光模块、服务器、大模型与应用",
    items: AI_GLOBAL_ITEMS,
  },
];
