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

// US-listed Chinese companies (中概股), grouped by sector. Checked against Tencent
// quotes on 2026-11-24; delisted or stale names (Zeekr, Qudian, Dada) are left out.
const CHINA_ADR_ITEMS: PresetItem[] = [
  // 电商零售 (5)
  { symbol: "BABA", name: "阿里巴巴", market: "US", sector: "电商零售" },
  { symbol: "PDD", name: "拼多多", market: "US", sector: "电商零售" },
  { symbol: "JD", name: "京东", market: "US", sector: "电商零售" },
  { symbol: "VIPS", name: "唯品会", market: "US", sector: "电商零售" },
  { symbol: "MNSO", name: "名创优品", market: "US", sector: "电商零售" },

  // 互联网与内容 (12)
  { symbol: "BIDU", name: "百度", market: "US", sector: "互联网与内容" },
  { symbol: "NTES", name: "网易", market: "US", sector: "互联网与内容" },
  { symbol: "BILI", name: "哔哩哔哩", market: "US", sector: "互联网与内容" },
  { symbol: "TME", name: "腾讯音乐", market: "US", sector: "互联网与内容" },
  { symbol: "IQ", name: "爱奇艺", market: "US", sector: "互联网与内容" },
  { symbol: "WB", name: "微博", market: "US", sector: "互联网与内容" },
  { symbol: "ZH", name: "知乎", market: "US", sector: "互联网与内容" },
  { symbol: "HUYA", name: "虎牙", market: "US", sector: "互联网与内容" },
  { symbol: "DOYU", name: "斗鱼", market: "US", sector: "互联网与内容" },
  { symbol: "JOYY", name: "欢聚", market: "US", sector: "互联网与内容" },
  { symbol: "MOMO", name: "挚文集团", market: "US", sector: "互联网与内容" },
  { symbol: "BZ", name: "BOSS直聘", market: "US", sector: "互联网与内容" },

  // 智能汽车与出行 (7)
  { symbol: "NIO", name: "蔚来", market: "US", sector: "智能汽车与出行" },
  { symbol: "XPEV", name: "小鹏汽车", market: "US", sector: "智能汽车与出行" },
  { symbol: "LI", name: "理想汽车", market: "US", sector: "智能汽车与出行" },
  { symbol: "HSAI", name: "禾赛", market: "US", sector: "智能汽车与出行" },
  { symbol: "PONY", name: "小马智行", market: "US", sector: "智能汽车与出行" },
  { symbol: "WRD", name: "文远知行", market: "US", sector: "智能汽车与出行" },
  { symbol: "YMM", name: "满帮", market: "US", sector: "智能汽车与出行" },

  // 金融科技 (6)
  { symbol: "FUTU", name: "富途控股", market: "US", sector: "金融科技" },
  { symbol: "TIGR", name: "老虎证券", market: "US", sector: "金融科技" },
  { symbol: "LU", name: "陆金所", market: "US", sector: "金融科技" },
  { symbol: "QFIN", name: "奇富科技", market: "US", sector: "金融科技" },
  { symbol: "FINV", name: "信也科技", market: "US", sector: "金融科技" },
  { symbol: "LX", name: "乐信", market: "US", sector: "金融科技" },

  // 消费与本地生活 (8)
  { symbol: "TCOM", name: "携程", market: "US", sector: "消费与本地生活" },
  { symbol: "YUMC", name: "百胜中国", market: "US", sector: "消费与本地生活" },
  { symbol: "HTHT", name: "华住", market: "US", sector: "消费与本地生活" },
  { symbol: "ATAT", name: "亚朵", market: "US", sector: "消费与本地生活" },
  { symbol: "BEKE", name: "贝壳", market: "US", sector: "消费与本地生活" },
  { symbol: "ZTO", name: "中通快递", market: "US", sector: "消费与本地生活" },
  { symbol: "LKNCY", name: "瑞幸咖啡", market: "US", sector: "消费与本地生活" },
  { symbol: "CHA", name: "霸王茶姬", market: "US", sector: "消费与本地生活" },

  // 教育 (3)
  { symbol: "TAL", name: "好未来", market: "US", sector: "教育" },
  { symbol: "EDU", name: "新东方", market: "US", sector: "教育" },
  { symbol: "GOTU", name: "高途", market: "US", sector: "教育" },

  // 医药生物 (4)
  { symbol: "ONC", name: "百济神州", market: "US", sector: "医药生物" },
  { symbol: "LEGN", name: "传奇生物", market: "US", sector: "医药生物" },
  { symbol: "ZLAB", name: "再鼎医药", market: "US", sector: "医药生物" },
  { symbol: "HCM", name: "和黄医药", market: "US", sector: "医药生物" },

  // 云与数据中心 (5)
  { symbol: "KC", name: "金山云", market: "US", sector: "云与数据中心" },
  { symbol: "GDS", name: "万国数据", market: "US", sector: "云与数据中心" },
  { symbol: "VNET", name: "世纪互联", market: "US", sector: "云与数据中心" },
  { symbol: "API", name: "声网", market: "US", sector: "云与数据中心" },
  { symbol: "TUYA", name: "涂鸦智能", market: "US", sector: "云与数据中心" },

  // 光伏新能源 (3)
  { symbol: "JKS", name: "晶科能源", market: "US", sector: "光伏新能源" },
  { symbol: "DQ", name: "大全新能源", market: "US", sector: "光伏新能源" },
  { symbol: "CSIQ", name: "阿特斯", market: "US", sector: "光伏新能源" },
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
    id: "cn_adr",
    label: `中概股 (${CHINA_ADR_ITEMS.length})`,
    description: `美股上市的中国公司 ${CHINA_ADR_ITEMS.length} 家：电商、互联网、智能汽车、金融科技、消费、教育、医药、云与新能源（腾讯批量实时）`,
    items: CHINA_ADR_ITEMS,
  },
  {
    id: "ai",
    label: `AI 全球产业链 (${AI_GLOBAL_ITEMS.length})`,
    description:
      "横跨美 / 台 / A / 港四地的 AI 全产业链龙头：算力芯片、代工设备、存储光模块、服务器、大模型与应用",
    items: AI_GLOBAL_ITEMS,
  },
];
