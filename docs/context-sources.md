# Context sources

MineContext 把用户主动导入与本机采集的内容写入笔记树（`document_type=vaults`），复用既有检索 / 向量索引。根 README 不再维护规划表；本页记录**当前已交付能力**与仍需外部账号 / 硬件的方向。

## Shipped in-repo

| Capability | How |
|---|---|
| Screen screenshot / window metadata | Capture pipeline |
| Note editing | Vault tree |
| Link upload | `POST /api/v1/links`；笔记树「导入链接」 |
| File upload（文档 / 图片 / 代码 / 音视频 / 会议字幕） | `POST /api/v1/files/import`；文件页「导入并分析」。音视频不做转写（与图片不做 OCR 同形） |
| File tracking / local folder（Obsidian、Memory Bank 等） | `POST /api/v1/files/import-folder`、`/api/v1/files/track`、`/api/v1/files/track/sync`；笔记树「导入本地目录」会登记跟踪 |
| Meeting records | `.vtt` / `.srt` / `.ics` 经文件导入，标签 `meeting` |
| RSS / Atom | `POST /api/v1/rss`；笔记树「导入 RSS」 |
| Deep Research（轻量） | `POST /api/v1/research`：主题 + 公开 URL 汇编一篇研究笔记（不接独立搜索引擎账号） |

## Deferred（需外部账号 / 硬件 / 独立产物）

| Capability | Blocker |
|---|---|
| Browser extension（AI 对话 / 网页精炼） | 独立浏览器扩展与分发 |
| Application MCP/API（Notion 云、Slack、Jira、Figma、Linear、Todoist、邮件、新闻、支付、论文 API 等） | 各服务 OAuth / API 密钥与线上联调 |
| WeChat / QQ 聊天捕获 | 平台私有协议与合规边界 |
| Mobile screenshot monitor | 移动端配套与硬件 |
| Smart glasses / bracelet sync | 可穿戴硬件与厂商 SDK |

本地 Obsidian 库与 Memory Bank 目录通过「导入本地目录 / 文件跟踪」覆盖，不依赖云端 MCP。
