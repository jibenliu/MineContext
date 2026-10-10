# 接口与前端

> 交付视图：控制面「长什么样、怎么鉴权、怎么报错」，以及渲染层「怎么通过本机 HTTP
> 与 SSE 拿到数据」。完整字段表与迁移细节见
> 控制面契约与前端迁移的设计推演原文（本机留档，不入库）。

## 1. 控制面

屏幕监控统计的 `recent_screenshots` 返回 blob 存储内的相对路径。缩略图通过 `screenMonitorAPI.readImageAsBase64` 调用已鉴权的 `/api/capture/screenshots/data`，使用响应中的 `data` 和 `mime` 渲染并预览；不要将路径转为 `file://` 或把鉴权 token 拼进图片 URL。图片读取失败或已被清理时显示重试入口。

- 只监听 **`127.0.0.1`**（端口由 daemon 自己挑，`--port 0`），
  **除 `/api/health` 外所有路径都要求 `X-MC-Token`**，没有 token 一律 401。
- daemon 起来后写 **`runtime.json`**（0600，含 `port` / `token` / `pid` / `version` /
  `started_at`）；渲染层与外壳都从它拿端口与 token，退出时删除（否则前端会读到过期端口）。
- 契约测试用的路由清单是**冻结**的（`fixtures/contract/compat-routes.json`），
  由 `mc-server` 的 `compat_routes_all_present` 保证「清单里的路径一条都不少」；
  渠道清单由 `cargo run -p xtask -- extract-used-ipc-channels` 生成并纳入门禁。

### 1.1 信封与错误

```jsonc
// 成功
{ "code": 0, "status": 200, "message": "success", "data": { }, "error_code": null, "remediation": null }

// 兼容面失败（**HTTP 200 + 非零 code**，调用方不处理非 200）
{ "code": 1, "status": 200, "message": "人话说明", "data": null,
  "error_code": "provider_timeout", "remediation": "去设置页填模型密钥", "detail": "…" }

// 扩展面失败（HTTP 4xx/5xx + 同一个信封）
```

- 每个错误都带 **错误码 + 组件 + 严重级别 + 是否可重试 + 补救建议**；
  错误码集合与 [`troubleshooting.md`](troubleshooting.md)
  一致（`check-docs.sh` 把关）。
- 「已登记但尚未实现」的路径返回**结构化的 `not_implemented`**，而不是 404 ——
  调用方能明确知道自己撞上了什么。

### 1.2 路由家族

| 前缀 | 用途 |
|---|---|
| `/api/health` | 启动握手（**唯一公开路径**） |
| `/api/diagnostics`、`/api/v1/diagnostics/export` | 不变量自检；可分享的诊断包（不含用户内容） |
| `/api/v1/vault/export`、`/api/v1/vault/import` | 笔记树 + `uploads/` 备份 zip（含用户内容；重装可恢复） |
| `/api/model_settings/*` | 模型配置读写与校验（密钥不回传，只回「有没有配」） |
| `/api/capture/*` | 权限、目标列表与选择、配置、立即截图、删除单张截图 |
| `/api/db/*` | 兼容面：活动、笔记树、待办、提示、热力图（**字段名不能改**） |
| `/api/v1/activities`、`/overrides` | 带来源与置信度的活动；用户改名/改分类（先落事件再重算） |
| `/api/v1/summaries*` | 总结列表、重生成；任意时段：`adhoc/preview`、`adhoc/jobs`、`jobs/{id}`、`jobs/{id}/cancel` |
| `/api/v1/threads`、`/api/v1/search` | 实体线索；搜索为本地关键词（响应带 `mode: "keyword"`，UI 显示「仅本地」；被拦截内容搜不到由服务端保证）。聊天路径可再叠向量。 |
| `/api/agent/chat/*` | 对话流（SSE 分帧）+ 会话/消息写入路径。模型断网/超时等可重试失败时降级为本地引用列表，`stream_complete.mode` 为 `"local"`，不假装在线生成。 |
| `/api/monitoring/recording-stats` | 录制统计（含最近错误与最近截图上限 5 条） |
| `/api/v1/stream` | 控制面事件流（SSE） |
| `/api/v1/jobs/backfill`、`/api/v1/jobs/{id}` | 作业队列：入队补偿推断、查状态（同范围幂等；未配置模型时如实跳过） |
| `/api/v1/links` | 链接上传：`POST {url, parent_id?}` 抓取公开网页正文，写入笔记树（`document_type=vaults`），进入既有检索/向量索引；拒绝非 http(s)、回环/私网与 `privacy.blocked_domains` |
| `/api/v1/files/import` | 文件上传：`POST {name, data, parent_id?}` 抽取文档/图片/代码/音视频元数据/会议字幕正文，写入笔记树并保留 `uploads/` 原文件；未知类型返回结构化错误 |
| `/api/v1/files/import-folder` | 本地目录导入：`POST {path, parent_id?, recursive?}` 扫描支持的文件并写入笔记树（Obsidian / Memory Bank 等） |
| `/api/v1/files/track`、`/api/v1/files/track/sync` | 文件跟踪：登记目录、列出、同步仅导入未见过的支持文件 |
| `/api/v1/rss` | RSS/Atom：`POST {url, parent_id?, limit?}` 抓取公开订阅源条目写入笔记树 |
| `/api/v1/research` | Deep Research（轻量）：`POST {topic, urls, parent_id?}` 抓取公开 URL 汇编一篇研究笔记 |
| `/api/v1/vault/export` | 导出笔记树与 `uploads/`：成功响应为 `application/zip`（`manifest.json` + `vaults.json` + `uploads/*`），需 token |
| `/api/v1/vault/import` | 导入同格式备份：`POST { data: "<base64 zip>" }`；重映射笔记 id / `parent_id`，并把正文里的 `file://…/uploads/<name>` 改写到本机 data_dir |

### 1.3 SSE

事件名沿用既有渠道名（`push:latest-activity`、`push:init-check-data`、
`activity:created`、`stage:closed`、`summary:progress` …），因此消费方零改动。

订阅**不能**用 `EventSource`：它不支持自定义请求头，而控制面除健康检查外都要求
`X-MC-Token` —— 用它只会得到一条被 401 掉的流，事件一个都不来，界面上仅表现为
「某些地方不刷新」。因此渲染层用 `fetch` + `ReadableStream` 自己读帧
（`adapters/sse-stream.ts`），并在建立连接时带 token。

连接建立后服务端依次发两帧：

1. `ready`：`{ port, version, token_ok }`（旧前端的握手）；
2. `push:init-check-data`：与 `GET /api/health` 同源的负载 —— 渲染层靠它判断
   「模型是否已配置」，从而决定进主界面还是进引导页。**少了这一帧，已经配好模型
   的用户每次启动都会被要求重新选一遍模型。**

负载约定按渠道不同（`adapters/server-push-api.ts` 是唯一解释处）：
`push:get-init-check-data` 的消费方自己 `JSON.parse`（所以原样透传字符串），
其余渠道解成值再交给消费方。

首次引导另有轻量清单（`adapters/first-run-checklist.ts`）：屏幕录制权限 → API Key →
开始录制 → 确认首张截图（或清除卡住的等待态）。完成或跳过写入
`settings.firstRunOnboardingComplete`（redux-persist），与模型引导页可同时出现。

## 2. 前端

### 业务操作与恢复

- 搜索期间显示加载状态并阻止重复提交；失败后可再次搜索。起止时间包含所选边界，服务端先筛选时间再按数量截断，倒置范围返回 400。
- 任意时段总结修改范围后必须重新预览。生成期间锁定范围，进度请求串行执行；页面卸载停止轮询，后台作业仍可继续。进度读取失败可重试原作业，避免重复提交；生成失败显示原因，取消完成显示已保留分块。
- 文件页通过 `POST /api/v1/files/import`（渠道 `v1:import-file`）导入文档、图片、代码、音视频与会议字幕：抽取正文（或媒体元数据）写入笔记树并进入既有检索/向量索引，同时保留 `uploads/` 原文件。列表接口仍返回 `status: "Uploaded"`；本次导入成功的条目在界面显示「分析成功」。导入失败可重试；同名文件替换原文件。选择文件本身不会向页面根路径发送上传请求。
- 笔记树另提供 RSS（`v1:import-rss`）、深度研究（`v1:import-research`）与本地目录导入/跟踪（`v1:import-folder` / `v1:track-folder` / `v1:sync-tracked-folders`）。能力边界见 [`context-sources.md`](./context-sources.md)。


### 2.1 结构

```
外壳（Tauri）           只做三件事：起 daemon、注入 window.mcRuntime、窗口/托盘
渲染层适配层            channel-map：渠道 → HTTP 请求；逐项安装全局
业务代码                继续用 window.dbAPI / screenMonitorAPI / …，一行不用改
```

外壳与渲染层之间只有一条约定：`runtime.json`（端口 + token），由初始化脚本注入
`window.mcRuntime.get()`。渲染层不 import 任何 Tauri API 也能跑起来（适配层探测
`__TAURI__` 才接管外壳能力）。

- **渠道映射两张表**：`CHANNEL_MAP`（前端在用的渠道，由契约夹具校验「不多不少」）
  与 `NEW_API_CHANNELS`（扩展面：总结、搜索、对话、活动来源、任意时段总结、
  补偿推断作业、启动握手）；`resolveChannel` 两张都查 —— 只查一张会让扩展面在运行时报
  「渠道未映射」而单测仍然是绿的。
- **没有实现、也没有推送来源的渠道必须显式登记**（`DEFERRED_CHANNELS`，每条写明
  原因）：托盘快捷动作、托盘录制状态、`backend:status-changed`、`store-sync:*`。
  渲染层用到这些渠道时，shim 会**告警一次并说明原因**，而不是留下一个永远不触发
  的监听器；`extract-used-ipc-channels` 还会从渲染层调用点反查，表里没有的渠道
  会让门禁第 1 步直接失败。
- **启动顺序**（顺序错了就会白屏，别再动）：

  ```
  main.tsx → bootstrapBackend：读 runtime.json（**20 × 250 ms 重试**，daemon 冷启动要几秒）
           → installHttpBackendFromRuntime：装适配层 + 把 axios 指到 daemon（带 token）
           → 渲染 App
  ```

- 适配层**逐项安装**：某一项装不上时报告并继续，不让整个 bootstrap 白屏。
  `window.electron.ipcRenderer` 由适配层自己装（`adapters/ipc-renderer-shim.ts`），
  没有第二个来源会跟它抢同一个全局。
- 交付版没有 `VITE_MC_BACKEND` 之类的开关：后端只有 daemon 一条路。

### 2.2 前端测试三层 + 静态检查

| 层 | 命令 | 现在 |
|---|---|---|
| lint | `npm run lint` | eslint 0 error（82 warning，主要是既有的 console 限制） |
| 类型 | `npm run typecheck` | 配置自身（node）+ 渲染层（web）两份 tsconfig，0 错误 |
| 适配层 | `npm run test:adapters`（`node --test`） | 81 条：渠道映射、渠道覆盖反查、后端切换、重试、形状转换、**响应形状契约**、shim 降级语义、托盘事件与外壳命令、日志出口接线 |
| 共享包 | `npm run test:shared`（`node --test`） | 8 条：日志前缀、落盘出口的时机与容错、消息格式化（循环引用/超长截断） |
| 页面级 | `npm run test:pages`（vitest + jsdom） | 70 条：总结卡片、笔记树、搜索、助手、活动来源与改名、任意时段总结、开机自启 |

页面级测试用**假后端**（`test/page-setup.ts`）按渠道给返回值，断言的是「用户能
看到什么」，不是「函数被调用过」；`installFakeBackend` 非严格模式下未配置的渠道
返回**空数组**而不是 `null`（给 `null` 会在 effect 里抛未处理 rejection，
页面看起来空白而测试仍然绿）。

渲染层日志（`packages/shared/logger/renderer.ts`）默认写控制台，**探测到外壳时同时转发**
给外壳命令 `renderer_log` 落盘（`<日志目录>/renderer.log`）。它不 import 任何外壳 API：
出口是运行时装的（`setLogSink`），因此 jsdom 里不需要替身，外壳不在时也照常工作。
出口抛异常不影响调用点，也不会自激成日志风暴。

### 2.3 响应形状契约（防"mock 绿、真实白"）

后端返回什么形状，前端解包就得按什么形状来。**形状不对不会报错，只会白屏**：
后端返回 `{ summaries: [...] }` 而前端当数组用时，页面整块空白；若测试里的 mock
恰好写的是裸数组，测试还会全绿 —— 所以形状要由契约夹具钉住。

因此加了一层契约夹具 `fixtures/contract/response-shapes.json`：

| 谁 | 断言什么 | 在哪 |
|---|---|---|
| daemon 侧 | fixture 里的样例形状 **== 真实路由返回的形状**（`v1:summaries` / `v1:conversations` / `v1:search` / `v1:adhoc-preview` / `backend:get-status` / `push:init-check-data`） | `crates/mc-server/tests/response_shapes.rs` |
| 前端侧 | 解包函数（`adapters/unpack.ts`）与页面测试的 mock **能吃下同一份样例**，且字段类型符合 UI 用法（例如总结的 `start` 必须是字符串，卡片按 `HH:mm` 解析） | `adapters/__tests__/response-shapes.test.ts`、`summary-card.page.test.tsx` |

两端任一改动都会让另一端红：后端改形状 → Rust 测试红；前端改解包/改 mock 形状 →
前端测试红。**改契约的正确姿势是同步改 fixture 与两侧**，而不是等界面白屏。

## 3. 兼容面的改动纪律

- 兼容面（`/api/db/*`、`/api/agent/chat/*`、`/api/monitoring/*`）的**字段名、
  JSON 字符串列、时间格式、信封形状都不能改** —— 调用方直接消费返回值，
  改一个字段就是空界面或白屏；
- 新增能力一律走扩展面（`/api/v1/*`），需要新渠道时同时更新
  `NEW_API_CHANNELS` 与页面级测试；
- 契约夹具是**生成物**：改渠道后跑
  `cargo run -q -p xtask -- extract-used-ipc-channels` 并提交产物，否则门禁第 1 步会红；
- 兼容面路由清单已冻结（旧后端源码不再存在），只能由 `mc-server` 的路由表增加，
  不能从夹具里删条目。

### 作业恢复、笔记图片与通知

- 时间线按 50 条分页，搜索按 20 条分页；翻页清除旧时间线选择，新搜索回到第一页。分页限制的是 DOM 渲染量，不增加服务端检索结果上限。
- 检索的最佳关键词匹配档次仅在满足时间、类型、来源和隐私条件的候选中计算；范围外的精确命中不会压掉范围内的有效命中。指定文档类型时跳过其他类型的数据表；未指定类型仍加载全部可检索类型，尚不是全链路 SQL 分页。
- 总结检索的时间范围由 SQLite 按总结开始时间过滤：`from <= start < to`，不是按区间重叠或生成时间过滤。省略任一边界表示该侧不限；无边界的旧读取入口保持原行为。活动与笔记的时间过滤仍在检索层执行。
- 旧 `#/ai-demo` 地址跳转真实助手 `#/assistant`，不再使用演示模型端点。
- 正式助手的复制、重新回答、中断、处理进度与引用展示走真实聊天链路。回答引用在消息 `metadata.sources` 保存，字段为 `document_id` / `title` / `kind`；原有 `citations` 数量字段保持兼容。历史没有 sources 时不编造引用。
- 前端文件与目录使用 kebab-case，TypeScript 标识符保留语言惯例；详见 `frontend/README.md`。命名检查与零警告约束包含在 `npm run lint` 中。

- 对话触发总结后跳转 `#/summaries?job_id=...&from=...&to=...`，接续已有作业进度与取消操作，不重复创建作业。daemon 重启后内存作业登记失效，不保证旧 job_id 可恢复。
- 编辑器图片仅接受 PNG/JPEG/GIF/WebP、单张不超过 1 MiB。正文保存本地 file URL，显示时通过认证文件接口读取；不保存短期 blob URL，也不在正文中保存认证 token。
- 文件读取接口支持 `encoding=base64` 返回二进制的 base64 文本；默认仍为 UTF-8 文本读取，不支持的编码返回错误。
- 设置中的系统通知开关是应用偏好，不代表操作系统授权。关闭后保留普通应用内提示，后台总结不再发系统通知；实际送达仍需系统授权。

### 提示词语言兼容

截图分析、活动推断与总结共享 `mc_common::locale::Locale::from_config` 的配置语言解析规则：忽略 ASCII 大小写，`en` 开头使用英文，其余回退中文。保持现有行为，不自动去除配置值两端空格。

配置值与领域对象序列化值不同：pipeline 的 Locale 保持 `zh_cn` / `en_us`，SummaryLocale 保持 `zhcn` / `enus`。总结类型保留兼容转换，不改变已有 JSON 或落盘数据的格式。

### 采集状态（`GET /api/capture/status`）

```json
{
  "canRecord": false,
  "status": "stopped",
  "enabled": false,
  "screen_recording_tcc": false,
  "windows_reason": "screen_recording_permission",
  "reason": "屏幕录制不可用：可能是缺少屏幕录制权限，或当前没有可用显示器"
}
```

- `canRecord`：本机此刻能否开始采集（同时受"是否挂载采集控制"与屏幕就绪度影响；屏幕路径可含经验证）；
- `status`：采集环是 `running` 还是 `stopped`；
- `enabled`：`capture.enabled`；为 `false` 时表示用户停过录，间隔配置不等于正在采集；
- `screen_recording_tcc`：采集进程的屏幕录制 TCC Preflight 原值（与经验证后的 `canRecord` 区分）；
- `windows_reason`：窗口列表状态——`ok` / `empty`（当前无打开窗口）/ `screen_recording_permission`（TCC 未授权）；非 macOS 可为 `null`；
- `reason`：**不能采集时的原因**，可采集时为 `null`。界面必须把它显示出来 ——
  用户看到「不能录制」却不知道是权限问题还是无显示会话时，只能去点一次「开始」才知道，
  而且很容易被误导成「现在不在录制时段」（时间窗口与权限问题需要完全不同的处置）。

原因与 `POST /api/capture/start` 的拒绝理由**同源**（都取自 `CaptureReadiness`），
因此两处不会互相矛盾。设置页应同时展示 TCC、录制开关与窗口列表原因；改完 TCC 后须从托盘完全退出再重开。
