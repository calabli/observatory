# 观测台 · Observatory

独立的 Tauri 2 + React + TypeScript + Rust + SQLite 桌面应用。AI 用量和本机网络概况已接入真实数据；设备性能、DNS 诊断与 Agent 调用栈目前尚未接入。

## 本机网络概况

显示本机网卡、IPv4/IPv6、状态、网关、网卡 DNS 和收发速率。进入网络概况后每 3 秒采样计数器，窗口不可见或离开页面时暂停；网卡配置每 30 秒更新，也可手动刷新。曲线只保留当前页面的最近 60 次有效速率采样，不持久化或自动共享。初次采样、网卡切换、计数器重置或间隔过长时不计算速率。

系统推测的推荐网卡可能因 VPN 或分流与期望路径不同，可手动选择。网卡状态不代表互联网可用；无法取得的字段显示未获取。国际出口 IPv4 在用户点击时请求 ipify，显示该请求路径的出口及检测时间，不与选中的网卡绑定，也不自动查询。地区信息、网关延迟、DNS 解析与应用连接将在后续阶段接入。系统采集网关信息时可能解析本地网关的邻居地址。跨平台网卡采集使用 MIT 许可的 netdev。

## 界面风格

采用 CC Switch 的黑白灰层级、橙色操作色与青色统计图，保留独立应用的顶部导航。AI 用量指标条可直接切换费用、请求、Token 和缓存命中率，趋势支持柱状和折线。主题值集中在 `src/theme/ccSwitchTheme.css`，来源为 CC Switch 的 `src/index.css` 与组件样式。

## 托盘与菜单栏

Windows 任务栏通知区域和 macOS 菜单栏提供观测台图标。菜单展示本机今日估算费用、请求数及 Token，每 30 秒读取本地统计；不会因此额外触发扫描。点击窗口关闭按钮会隐藏窗口并继续后台运行，可通过托盘菜单重新打开或退出。若系统托盘创建失败，窗口关闭仍会退出程序。

## 已实现的 AI 用量

- 本机与多设备汇总；时间、自定义日期、应用、供应商和模型筛选。
- 请求、费用、四类 Token、缓存命中率、趋势、调用活动、供应商与模型统计。
- 请求分页、状态码筛选、详情与当前页 CSV 导出；区分精确耗时和日志估算耗时。
- 独立持久化数据库、采集游标和长期去重记录；重复扫描不重复累计。
- 模型四类价格编辑与删除、缺失费用回填、models.dev 导入及自动同步配置。启动时检查，成功同步间隔至少六小时。
- LAN 设备发现、手动连接、按应用共享、快照替换、停用设备保留缓存、断开删除副本。远端费用不按本机价格重新计算。

### 数据来源

直接复用 CC Switch 的日志采集器：Claude Code、Codex、Gemini CLI、Grok Build、OpenCode、Pi、MiniMax Code、Antigravity、Kimi Code、DeepSeek Harness、ZCode。

Claude Desktop、Hermes、OpenClaw 目前通过 CC Switch 历史统计导入或对端共享接入，尚未提供独立日志采集器。采集开关停止后续扫描，不删除已有历史。

## 数据目录与历史迁移

默认目录为 `~/.observatory/`，包含 `observatory.db`、`model-pricing.json` 和实例锁。`OBSERVATORY_HOME` 可指定独立目录。

首次启动且本库没有统计记录时，如果发现 `~/.cc-switch/cc-switch.db`，会先执行只读历史迁移，再开始原生日志扫描。只复制统计明细、日汇总、游标和去重记录；供应商仅保留名称，配置置为空对象，不导入密钥或对话正文。

日汇总只在首次迁移时建立历史基线。后续重复导入只补入缺失明细，不再次叠加源库后来剪枝生成的日汇总。若目标已独立采集、源库又包含日汇总而无迁移基线，会拒绝歧义合并；请保留原库，使用新的 `OBSERVATORY_HOME` 先迁移再采集。历史迁移是一次性基线，不是对 CC Switch 数据库的实时镜像。

可通过 `CC_SWITCH_DB` 指定源数据库，或在“AI 用量 → 数据来源”中填写路径。其他设备的记录应通过 LAN 同步接入。

日志路径沿用相应工具约定，并支持原有环境变量，例如 `CODEX_HOME`、`DSH_HOME`、`ZCODE_DB`、`MINIMAX_DATA_DIR`、`MAVIS_DATA_DIR`、`KIMI_CODE_HOME`、`PI_CODING_AGENT_DIR`、`OPENCODE_DB`。

## 局域网共享

默认关闭。观测台默认使用 TCP/UDP 17891，避免与 CC Switch 默认 17890 冲突；扫描同时兼容两种端口。手动连接 CC Switch 时填写 `IP:17890`。

共享开启后，局域网中能够访问该端口的设备可以读取勾选应用的统计指标；当前协议没有配对认证。请仅在可信网络开启。不会导出 API 密钥、供应商配置或对话正文。

## 开发

需要 Node.js 20.19+ 或 22.12+、Rust stable，以及平台编译工具。Windows 需要 Visual Studio C++ Build Tools、Windows SDK 和 WebView2。

```sh
npm install
npm run native:build
npm run dev
```

浏览器开发入口为 `http://127.0.0.1:5188/`。Vite 只监听回环地址，通过同源会话令牌调用 Rust 子进程；生产桌面程序直接使用 Tauri IPC，不启动 Node 服务。

```sh
npm run desktop:dev
npm run desktop:build
```

`desktop:build` 生成嵌入前端资源的开发版 EXE；`desktop:release` 生成优化构建。浏览器开发服务和桌面程序共用数据目录，同一时间使用其中一个。Windows 上修改 Rust 后，请先停止正在使用对应引擎的开发服务，再重新构建。

```sh
npm run typecheck
npm test
npm run build
npm run native:test
```

原始统计代码中的应用级单测保留在源码中，独立核心使用 `native/tests/usage_integration.rs` 验证迁移契约：导入只读与去重、剪枝基线、缓存语义、DSH 拼接压缩帧、ZCode 耗时与长期去重、HTTP 双实例快照、分页和价格回填。

## 来源与许可

迁移清单见 `native/migration-manifest.json`。保留 CC Switch 的 MIT 许可证和原始源码注释：`licenses/CC-Switch-MIT.txt`。迁移适配主要涉及数据目录、最小数据库、事件通知、Tokio 运行时和应用启动入口。

ZCode 图标来自官方桌面安装包，来源说明位于 `src/assets/zcode.SOURCE.md`。

Windows 安装器模板基于 Tauri CLI 2.10.1 的 NSIS 模板（MIT 或 Apache-2.0 许可）；调整默认安装目录为 `Observatory`，保留已有安装路径的升级识别。
