# Context sources

MineContext 把用户主动导入与本机采集的内容写入笔记树（`document_type=vaults`），复用既有检索 / 向量索引。根 README 不再维护规划表；本页记录**当前已交付能力**与仍需外部账号 / 硬件的方向。

## Platform: screen / window capture

| Platform | Screen / window capture | What still works |
|---|---|---|
| **macOS 13+** | Supported（需「屏幕录制」权限） | 全量：采集、导入、检索、总结 |
| **Windows / Linux** | **Not implemented** | daemon / UI / 笔记导入 / 检索与总结可跑；**不会采集屏幕内容** |

实现位置：`crates/mc-capture/src/platform/`（macOS 真实现；其它平台走 `unsupported` 源）。
控制面通过 `GET /api/capture/permissions` 与 `GET /api/capture/status` 的
`capture_supported` 字段如实报告；屏幕监控页在 `false` 时显示横幅、禁用「开始录制」，
**不会**再引导去开 macOS 式屏幕录制权限。Windows 安装包尚未在本仓库构建或真机验证
（见根 README 与 [`operations.md`](operations.md)）。

## Shipped in-repo

| Capability | How |
|---|---|
| Screen screenshot / window metadata | Capture pipeline（**macOS only**；见上表） |
| Note editing | Vault tree |
| Link upload | `POST /api/v1/links`；笔记树「导入链接」 |
| File upload（文档 / 图片 / 代码 / 音视频 / 会议字幕） | `POST /api/v1/files/import`；文件页「导入并分析」。音视频不做转写（与图片不做 OCR 同形） |
| File tracking / local folder（Obsidian、Memory Bank 等） | `POST /api/v1/files/import-folder`、`/api/v1/files/track`、`/api/v1/files/track/sync`；笔记树「导入本地目录」会登记跟踪 |
| Meeting records | `.vtt` / `.srt` / `.ics` 经文件导入，标签 `meeting` |
| RSS / Atom | `POST /api/v1/rss`；笔记树「导入 RSS」 |
| Deep Research（轻量） | `POST /api/v1/research`：主题 + 公开 URL 汇编一篇研究笔记（不接独立搜索引擎账号） |

## Deferred（需外部账号 / 硬件 / 独立产物）

| Capability | Blocker | Tracking |
|---|---|---|
| Browser extension（AI 对话 / 网页精炼） | 独立浏览器扩展与分发 | [#50](https://github.com/jibenliu/MineContext/issues/50) |
| Application MCP/API（Notion 云、Slack、Jira、Figma、Linear、Todoist、邮件、新闻、支付、论文 API 等） | 各服务 OAuth / API 密钥与线上联调 | 下一外部入口见下；其余仍按服务拆分 |
| WeChat / QQ 聊天捕获 | 平台私有协议与合规边界 | [#52](https://github.com/jibenliu/MineContext/issues/52)（伞形） |
| Mobile screenshot monitor | 移动端配套与硬件 | [#52](https://github.com/jibenliu/MineContext/issues/52)（伞形） |
| Smart glasses / bracelet sync | 可穿戴硬件与厂商 SDK | [#52](https://github.com/jibenliu/MineContext/issues/52)（伞形） |

本地 Obsidian 库与 Memory Bank 目录通过「导入本地目录 / 文件跟踪」覆盖，不依赖云端 MCP。

## Next（外部入口）

**下一外部 Context Source：Google Calendar（MCP/OAuth）** — [#51](https://github.com/jibenliu/MineContext/issues/51)。

优先于完整浏览器扩展：会议记录已支持 `.ics` / 字幕文件导入，日历 OAuth 是最小的云账号闭环。验收草图与非目标写在该 issue；本仓库暂无足够安全的半成品 OAuth 钩子可单独合入。

相关已知坑（非新能力）：屏幕录制 TCC → 0 张截图 [#54](https://github.com/jibenliu/MineContext/issues/54)；embedding/API 401 与模型配置 [#55](https://github.com/jibenliu/MineContext/issues/55)；打 tag 前版本对齐 [#53](https://github.com/jibenliu/MineContext/issues/53)。
