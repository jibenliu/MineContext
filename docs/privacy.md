# 隐私说明

**一句话**：默认**只在本机工作**——采集、识别、存储、检索都在本地；
未经你显式同意，任何内容都不会发到模型服务。

**支持范围**：macOS 13 及以上（`doctor` 的 `platform` 段会报告是否受支持）。

---

## 1. 采集什么

| 类型 | 内容 | 默认 |
|---|---|---|
| 屏幕 | 定时截图（变化检测后**只留有变化的帧**） | 开 |
| 窗口 | 前台应用名、窗口标题 + 窗口图像（`macos:window` 采集源） | 开 |
| 剪贴板 / 文件 / 浏览器 | **尚未实现**：`capture.sources` 里写了会被明确报告为「已跳过」，不会静默变成屏幕采集 | 关（也采不到） |
| 链接上传（用户主动） | 用户在笔记树提交的 http(s) URL：daemon 抓取正文写入本地笔记；拒绝回环/私网与 `privacy.blocked_domains` | 用户触发 |

**不做的事**：不记录键盘输入、不注入任何进程、不读取其他应用的内存、
不采集麦克风与摄像头。屏幕内容通过系统 Screen Recording 权限获取，
**权限未授予时不会截到任何东西**（macOS 会返回全黑帧，本项目会主动检测并报
`capture_permission_denied`，见 [`troubleshooting.md`](troubleshooting.md)）。

## 2. 存在哪里

数据目录（默认由 `--data-dir` 指定，`mc-cli doctor` 会打印实际路径）。
macOS 桌面版默认是 `~/Library/Application Support/MineContext`：

```
<data_dir>/
  data/minecontext.db      事件日志、观测、活动、阶段、总结、对话、模型调用记账
  data/minecontext.db-wal  数据库的一部分（连同 -shm），不要单独删
  blobs/screenshots/…      截图原图（按 年/月/日 分目录，文件名是内容哈希）
  blobs/thumbnails/…       缩略图
  uploads/                 「上传文件」功能保存的文件
  config.toml              你的设置（用户配置层，权限受系统默认保护）
  model-keys.json          密钥落点说明（0600）：导入系统钥匙串之前，明文可能暂存在这里
  runtime.json             端口与本次启动的 token（0600，退出时删除）
  logs/  frontend-logs/    后端与渲染层的日志
  .shell.lock              外壳单实例锁
```

密钥本体在**系统钥匙串**（配置里只写 `api_key_ref = "keychain:mc:model"`），不在数据目录里。

数据库与图片都在本机；没有云端账号，没有服务端同步。

### 2.1 手动清理

界面上还没有「删除某张截图」的入口（后端已经有 `DELETE /api/capture/screenshots`，
缺的只是入口）。要手工清理，**先退出应用**（数据库打开时不要动它），再按需要删：

| 想清掉 | 删什么 | 后果 |
|---|---|---|
| 截图与缩略图 | `blobs/screenshots/`、`blobs/thumbnails/` 下的日期目录 | 库里对应行的图片路径成为悬空引用；daemon 下次保留策略轮转时清掉这些引用，在那之前界面显示「加载失败 · 重试」占位，不是破图 |
| 全部采集内容 | 整个数据目录 | 下次启动重建空库，需重新配置模型与采集源 |
| 只清配置 | `config.toml` | 回到默认配置（含 `capture.retention_days` 默认 7 天） |
| 日志 | `logs/`、`frontend-logs/` | 无 |
| 残留运行态 | `runtime.json`、`.shell.lock` | 无，启动时会重建 |
| 钥匙串里的密钥 | `security delete-generic-password -s mc -a model` | 需要重新在设置页填入模型密钥 |

截图本身也会**自动**轮转：`capture.retention_days` 到期后由 daemon 删除，并在同一次
操作里清掉库里的引用（`crates/mc-storage/src/retention.rs`），所以长期不用手工删。

完整卸载 = 退出应用 → 删 `/Applications/MineContext.app` → 删数据目录 → 删钥匙串条目。
macOS 卸载应用不会碰数据目录，这三步要分别做。

## 3. 什么时候出网

**默认不出网**。`privacy.ai_upload` 默认为 `false`，此时
**视觉推断、总结、对话三条路径都不会组装 provider**（`crates/mc-server/tests/privacy_upload.rs`
里有断言），因此物理上不可能发出请求 —— 这不是「配了不发」，而是「没有可发的对象」。

显式把 `privacy.ai_upload` 设为 `true` 之后，且满足以下条件才会出网：

| 路径 | 需要同时满足 |
|---|---|
| 视觉推断（截图 → 活动） | `ai.enabled`、`ai.vision.base_url`+`model` 已配、能取到密钥 |
| 总结（阶段/日报/周报） | `ai.enabled`、`ai.chat.base_url`+`model` 已配、能取到密钥 |
| 对话 | 同上（chat 端点） |

出网的目标地址完全由你的配置决定；本仓库不内置任何厂商地址、不内置任何
遥测与崩溃上报。**没有「匿名统计」这类隐藏出口**。

模型不可用（未配置/限流/超时）时，总结退化为**本地确定性兜底**，
而不是把内容重试到别处。

## 4. 如何关闭

| 想关掉 | 怎么做 |
|---|---|
| 全部出网 | `privacy.ai_upload = false`（默认值） |
| 采集 | `capture.enabled = false`，或前端「暂停录制」 |
| 只关 AI 但保留本地记录 | `ai.enabled = false` |
| 只录某些时段 | `capture.enable_recording_hours` + `recording_hours`（[`troubleshooting.md`](troubleshooting.md) 有排查） |
| 只录某几块屏 | `capture.target_ids`（空 = 全部可见目标） |
| 删掉旧截图 | `capture.retention_days` 控制轮转；单张用 `DELETE /api/capture/screenshots?path=<相对路径>` |
| 完全清除 | 删除数据目录（位置见 §2） |

## 5. 拦截：被判定为敏感的内容

规则匹配在**落盘之前**执行：

- 命中黑名单的应用/窗口标题 → 判定 `blocked`：**图片不落盘**、
  观测的图片字段为空、只留一条审计元数据（应用名 + 规则 + 时间，**不含内容**）；
- 规则引擎异常 → 按 `blocked` 处理（fail-closed），而不是放行；
- 被拦截的内容还额外保证：**不进检索、不进线索、不进提示词**
  （检索层在打分前就剔除，且向量索引也不包含它们）。

`/api/diagnostics` 的 `recent_failures` 会列出 `privacy_blocked` 相关记录，
但**只有应用名与规则，没有内容**。

**前台窗口否决票**：窗口观测被拦下还不够 —— 屏幕截图的像素里
同样是那个应用。因此一轮采集里只要前台窗口命中黑名单，**同一轮的屏幕帧
也一起不落盘**。这是 fail-closed 方向：宁可少存一帧，也不多存一帧。

## 6. 已知缺口（尚未生效，请不要依赖）

诚实标注，避免造成错误的安全感：

| 项 | 现状 |
|---|---|
| ~~`privacy.redact_patterns`（文本脱敏）~~ | ✅ **已生效**：对话的提问与引用标题、总结提示词里的活动标题与正文都在**出网前**替换为 `[已脱敏]`；规则非法时两条路径都不组装 provider（fail-closed） |
| ~~`privacy.blocked_domains`~~ | 🟡 **部分生效**：规则会真的拦（窗口观测不落盘 + 同轮屏幕帧一起拦）。但当前**只有窗口标题**可用，没有浏览器 URL —— 因此匹配是「标题里出现该域名」的启发式：标题只有站点名的窗口拦不住。URL 级精确定位要等浏览器采集源 |
| ~~`privacy.blocked_apps` / `blocked_window_patterns`~~ | ✅ **已生效**：采集环把配置传进 `PumpPolicy`，命中即不落图片、不留窗口标题，拦截计数可见；macOS 现在有真正的**窗口采集源**（应用名 + 窗口标题），`capture.sources` 里默认就包含它 —— 在此之前只截显示器，规则配了也不会命中 |
| 剪贴板 / 文件 / 浏览器采集源 | **未实现**（配置里点名时会明确报告跳过，而不是静默替换成屏幕采集） |
| 黑名单与**像素** | 前台窗口命中黑名单时同轮屏幕帧会被拦下，但**后台**运行着的拉黑应用仍会出现在屏幕截图里 —— 每轮只采前台窗口的元数据（成本取舍），因此看不到后台窗口的应用名。要彻底覆盖需要「枚举全部窗口做全屏否决」，代价是大量帧被整屏丢弃 |

这些缺口都已登记，尚未完成 —— 本说明不把「计划做」写成「已经做」。
在补上之前，**唯一可靠的隐私边界是 `privacy.ai_upload = false`** ——
也就是默认状态。

## 7. 你可以自己验证

| 想验证 | 怎么做 |
|---|---|
| 默认不出网 | 保持 `ai_upload = false`，抓包或看 `provider_calls` 表为空 |
| 拦截是否真的不落盘 | 把某应用加进黑名单，在该应用前台截图后查看 `blobs/` 与 `observations.image_path` |
| 出网内容有哪些 | `provider_calls` 表记录了每次调用的模型、用途、token 数 |
| 没有任何遥测 | 代码里没有第三方上报 SDK；只有配置里写明的端点会被访问 |

对应的自动化测试：`mc-server/tests/privacy_upload.rs`（出网许可）、
`mc-search/tests/privacy.rs`（检索不可见）、`mc-server/tests/retrieval_chat.rs`
（对话不引用被拦截内容）、`mc-pipeline/src/pump.rs` 的隐私判定单测。
