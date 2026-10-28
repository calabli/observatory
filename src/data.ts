export type Page =
  | "overview"
  | "usage"
  | "agents"
  | "performance"
  | "network"
  | "devices";
export type DeviceId = "windows" | "mac";
export const deviceInfo = {
  windows: {
    name: "Windows 工作站",
    os: "Windows 11 Pro",
    cpu: "AMD Ryzen 7 7840HS",
    cores: "8 核 / 16 线程",
    cpuUsage: 18,
    memory: 12.6,
    memoryTotal: 32,
    disk: 246,
    diskTotal: 512,
    ip: "203.0.113.108",
    gateway: "203.0.113.1",
    adapter: "WLAN",
    down: "2.4",
    up: "368",
    uptime: "2 天 06 小时",
  },
  mac: {
    name: "MacBook Pro",
    os: "macOS",
    cpu: "Apple M3 Pro",
    cores: "11 核",
    cpuUsage: 12,
    memory: 8.2,
    memoryTotal: 18,
    disk: 184,
    diskTotal: 512,
    ip: "203.0.113.123",
    gateway: "203.0.113.1",
    adapter: "en0",
    down: "1.6",
    up: "214",
    uptime: "1 天 14 小时",
  },
} as const;
export const cpuSeries = [
  12, 16, 13, 18, 14, 24, 28, 19, 23, 18, 22, 41, 37, 26, 28, 22, 17, 23, 18,
  14, 19, 17, 21, 18,
];
export const memorySeries = [
  34, 34, 35, 36, 35, 36, 37, 37, 38, 37, 37, 38, 39, 40, 40, 39, 39, 40, 41,
  40, 39, 39, 40, 39,
];
export const downSeries = [
  0.1, 0.2, 0.1, 0.4, 0.9, 1.6, 1.2, 2.4, 3.7, 2.7, 2.9, 1.7, 1.1, 2.3, 3.2,
  2.2, 1.6, 2.6, 1.8, 1.3, 2.8, 2.2, 2.6, 2.4,
];
export const upSeries = [
  0.08, 0.09, 0.1, 0.12, 0.3, 0.45, 0.3, 0.56, 0.8, 0.5, 0.6, 0.4, 0.2, 0.48,
  0.7, 0.5, 0.4, 0.53, 0.4, 0.3, 0.6, 0.5, 0.4, 0.368,
];
export const usageSeries = [
  0, 0, 1, 0, 1, 0, 3, 6, 18, 42, 30, 34, 16, 10, 28, 19, 8, 10, 5, 4, 1, 0, 0,
  0,
];
export const sources = [
  "Claude Code",
  "Claude Desktop",
  "Codex",
  "Gemini CLI",
  "Grok Build",
  "OpenCode",
  "OpenClaw",
  "Hermes",
  "Pi",
  "MiniMax Code",
  "Kimi Code",
  "Antigravity",
  "DeepSeek Harness",
  "ZCode",
];
export const processes = [
  {
    name: "DeepSeek Harness",
    pid: 13420,
    cpu: 6.4,
    memory: 1824,
    network: "164 KB/s",
    role: "AI 应用",
  },
  {
    name: "Chrome",
    pid: 8824,
    cpu: 4.2,
    memory: 2486,
    network: "2.1 MB/s",
    role: "浏览器",
  },
  {
    name: "ZCode",
    pid: 16204,
    cpu: 3.1,
    memory: 864,
    network: "68 KB/s",
    role: "AI 应用",
  },
  {
    name: "Mihomo",
    pid: 5736,
    cpu: 1.8,
    memory: 148,
    network: "2.4 MB/s",
    role: "网络代理",
  },
  {
    name: "观测台",
    pid: 20132,
    cpu: 0.6,
    memory: 126,
    network: "4 KB/s",
    role: "观测工具",
  },
];
export const connections = [
  {
    process: "DeepSeek Harness",
    target: "api.deepseek.com",
    address: "203.0.113.10:443",
    protocol: "HTTPS",
    traffic: "164 KB/s",
    status: "已建立",
  },
  {
    process: "ZCode",
    target: "api.minimax.io",
    address: "203.0.113.25:443",
    protocol: "HTTPS",
    traffic: "68 KB/s",
    status: "已建立",
  },
  {
    process: "Chrome",
    target: "github.com",
    address: "198.51.100.42:443",
    protocol: "HTTPS",
    traffic: "2.1 MB/s",
    status: "已建立",
  },
  {
    process: "系统 DNS",
    target: "223.5.5.5",
    address: "223.5.5.5:53",
    protocol: "UDP",
    traffic: "1.2 KB/s",
    status: "活跃",
  },
];
export const spans = [
  {
    id: 0,
    name: "定位网络请求超时",
    type: "主 Agent",
    depth: 0,
    start: 0,
    duration: 8.24,
    tokens: "32.8K",
    cost: "$0.048",
    status: "已完成",
  },
  {
    id: 1,
    name: "检查网络接口",
    type: "工具调用",
    depth: 1,
    start: 0.18,
    duration: 0.72,
    tokens: "—",
    cost: "—",
    status: "已完成",
  },
  {
    id: 2,
    name: "诊断 DNS 解析",
    type: "子 Agent",
    depth: 1,
    start: 1.02,
    duration: 5.72,
    tokens: "18.4K",
    cost: "$0.026",
    status: "已完成",
  },
  {
    id: 3,
    name: "model.call",
    type: "模型调用",
    depth: 2,
    start: 1.28,
    duration: 2.62,
    tokens: "12.6K",
    cost: "$0.019",
    status: "已完成",
  },
  {
    id: 4,
    name: "dns.lookup",
    type: "工具调用",
    depth: 2,
    start: 4.08,
    duration: 0.54,
    tokens: "—",
    cost: "—",
    status: "已完成",
  },
  {
    id: 5,
    name: "生成诊断摘要",
    type: "模型调用",
    depth: 1,
    start: 6.84,
    duration: 1.4,
    tokens: "14.4K",
    cost: "$0.022",
    status: "已完成",
  },
];
