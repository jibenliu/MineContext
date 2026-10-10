# 运维与交付

> 这份文档回答「怎么把它跑起来、怎么知道它没问题、出问题看哪里、发出去之前还差什么」。
> 它是交付视图；用户可读的排查与隐私说明见
> [`troubleshooting.md`](troubleshooting.md) 与 [`privacy.md`](privacy.md)。
> 开发过程中的验收登记（外部条件项判据、发布清单、交付过程说明）属**本机留档**，
> 不进交付仓库。

## 0. 形态（先说清楚，避免误判）

- **桌面外壳只有一个：Tauri 2**（`src-tauri/` 是**独立 crate**，tauri 依赖树不进根
  workspace，因此不会拖慢后端门禁）。渲染层是纯 `vite` 构建的静态产物
  （`frontend/out/renderer`，`base: ./` 相对路径，否则 Tauri 自定义协议下资源 404）。
  Electron 主进程 / preload / electron-vite / electron-builder 已从仓库移除，不再有第二条
  外壳，也不再有「切默认」这件事。产物默认 adhoc `.dmg`（配置 §5.0 凭据后可公证）；
  窗口与托盘观感、系统通知是否真的弹出仍属人工确认项（§7）。
- **后端只有一个：Rust 守护进程 `mc-daemon`**。没有 Python 后端、没有第二个后端
  路径、没有构建期开关；应用起来就拉起 daemon，渲染层从它写的 `runtime.json`
  拿端口与 token。
- 数据都在本机：`<数据目录>/{data/minecontext.db, blobs/, logs/, rules/}`；
  控制面只监听 `127.0.0.1`，除健康检查外都要求 `X-MC-Token`。

## 1. 构建与运行

```bash
# 后端（daemon + CLI）
cargo build --release -p mc-daemon -p mc-cli

# 起后端：数据目录默认 ~/Library/Application Support/MineContext
./target/release/mc-daemon --data-dir ~/Library/Application\ Support/MineContext --port 0
#   它会写 runtime.json（端口 + token，0600），前端据此连上来
#   落盘位置清单与手动清理步骤见 docs/privacy.md 的 §2 / §2.1

# 前端（渲染层）
cd frontend && pnpm install
pnpm dev                       # 开发服务器：http://localhost:5173（= Tauri 的 devUrl，端口钉死）
pnpm build:web                 # 生产构建 → frontend/out/renderer（= Tauri 的 frontendDist）

# 开发态跑外壳（另开一个终端）
cd src-tauri && cargo tauri dev

# 打包：release daemon → 渲染层 → Tauri 打包（不启动应用）
./scripts/package-macos-tauri.sh

# 发版或提交前显式验收：失败返回非零，不可作为验收通过的版本交付
./scripts/package-macos-tauri.sh --with-smoke
```

**安装包**：`src-tauri/target/release/bundle/dmg/MineContext_<版本>_aarch64.dmg`
（哈希每次打包都变，以脚本输出为准）。默认 **adhoc 签名、未公证**；配置 §5.0 凭据后
为 Developer ID + 公证。未公证包从网上下载后若提示「已损坏」，先清隔离属性：

```bash
xattr -cr ~/Downloads/MineContext_*.dmg
xattr -dr com.apple.quarantine "/Applications/MineContext.app"
```

仅对自己编译或已确认可信的安装包移除隔离属性；此操作不授予屏幕录制权限。实时日志、启动输出、执行权限和系统隐私授权步骤见 [macOS 安装后白屏与权限排查](troubleshooting.md#macos-安装后白屏与权限排查)。

### 打包细节（Tauri）

- 产物：`src-tauri/target/release/bundle/dmg/MineContext_<版本>_x64.dmg`；
  包里 daemon 是 **release** 二进制（脚本用 `--config` 覆盖 `bundle.resources`；
  仓库配置默认指向 debug，方便日常 `--debug` 打包）；
- 渲染层产物必须落在 `frontend/out/renderer`，与 `tauri.conf.json` 的
  `frontendDist` 一致；改任一处都要同步改另一处，否则打出来是空壳；
- 启动检查 `scripts/tests/launch-check-tauri.sh` **六项**断言：daemon 在包里可执行、
  外壳可执行文件存在、`runtime.json` 写出、**渲染层真的完成 bootstrap**（判据是
  "适配层已装"那条自检日志经由外壳落盘）、**SIGTERM 后 `runtime.json` 与
  `.shell.lock` 都被清掉**、**外壳进程真的退出**（文件清掉 ≠ 进程退出，实测过
  "文件已清但进程以 200% CPU 空转"）；
- 门禁第 6 步**只挂载已有 dmg、不负责打包**：产物不存在时它会 SKIP（=「没验证」，
  不是「通过」）。要拿到「新构建的产物也能起来」的证据，得先跑一次
  `./scripts/package-macos-tauri.sh --with-smoke`。默认打包只确认构建完成，不代表启动验收通过。
  已有产物可单独运行 `./scripts/tests/launch-check-tauri.sh <dmg路径>`，无需重新编译。
  启动失败时会打印并保留临时诊断目录中的 `shell.log` 与 `logs/daemon.log`；
- 默认 adhoc、未公证（§5.0）；窗口显示、托盘图标与菜单、关窗是否真的隐藏属人工确认项（§7）。

**外壳与渲染层的边界**：渲染层与适配层完全共用同一份产物；外壳只负责窗口、托盘、
daemon 生命周期与打包。Tauri 侧已实现：读 `runtime.json` 的桥（初始化脚本注入
`window.mcRuntime`）、拉起/停止 daemon、单实例锁、SIGTERM/SIGINT 清理、
托盘（显示窗口 / 开始-暂停录制 / 屏幕监控 / 退出）、关窗收进托盘、系统通知、开机自启。
托盘动作**不在外壳里实现业务**：外壳只发事件（`push:tray-toggle-recording` /
`push:tray-navigate-to-screen-monitor`），由渲染层调 daemon 接口，外壳不持第二份
采集开关状态；两个动作都会把窗口显示出来（渲染层的处理是「跳到屏幕监控页并切换」，
窗口藏着不动等于「点了没反应」）。渲染层上报的录制状态走 `tray_recording_status`
命令更新托盘提示与菜单文案。渲染层日志经 `renderer_log` 命令落盘到
`<日志目录>/renderer.log`（同时进 stdout），因此渲染层除了控制台还有一条
可查的通道。**检查更新**：外壳命令 `check_for_update` 对照
`jibenliu/MineContext` 的 GitHub Releases 最新 tag，与本机版本比较后返回发布页 /
dmg 链接（设置页可点「检查更新」）；**静默下载与安装未实现**（需要签名与公证，
`quitAndInstall` / `cancelDownload` 仍明确失败）。

## 2. 怎么知道它没问题

### 2.1 一条命令（本地）

日常改动（一两行、单个 crate、单个页面）：

```bash
./scripts/verify-affected.sh          # 按改动面选最小检查集
./scripts/verify-affected.sh --list   # 只打印将要跑什么
```

阶段收尾、跨切面源码验证（不运行冒烟）：

```bash
./scripts/verify-all.sh
```

八步：契约新鲜度（含**从渲染层调用点反查**渠道）→ 四类守卫与守卫自检 → `fmt` →
`clippy -D warnings` → 全量 Rust 测试（默认排除真进程 e2e）→ 可选冒烟 → 前端（lint + typecheck + 三层测试 + 纯
`vite` 生产构建）→ 汇总。

提交前用 `./scripts/verify-affected.sh --with-smoke`；打版本或最终交付前用
`./scripts/verify-all.sh --with-smoke`。冒烟包含 daemon 真进程 e2e、macOS 产物校验与
产物启动检查（只挂载已有 dmg）。开发过程中不自动触发；需要单独执行时使用
`./scripts/verify-smoke.sh`。打包脚本仍检查刚生成的 dmg。

这套检查各自证明了什么，比某一次的数字更重要：

- 第 5 步（Rust 测试）证明**代码与契约**；产物数量与 minos 由第 6 步按当前构建核对，
  数字随代码变化，不在本文里固定；
- 前端的 lint / 类型 / 三层测试 / 生产构建各自独立，任何一层红都不算通过；
- 门禁第 6 步**只挂载已有产物**，不重新打包 —— 别把它当成「新产物也验过」。
  要验新产物，单独跑 `./scripts/package-macos-tauri.sh`（它会重新打包并做启动检查）。

### 2.1b 为什么必须定期跑全量门禁（不是「有空再说」）

按片验证只跑「与本片相关的门禁」，跨片的结构性不变量不会有人查。实测代价：
连续 8 片功能/外壳改动之后跑一次全量，**前 3 步连红三次**，全是按片验证覆盖不到的：

1. **契约夹具来源失效**：preload 从 175 行缩到 32 行后不再引用任何渠道，而渠道
   抽取器仍扫 preload 源码 —— 清单从 60 个掉到 5 个。若不跑第 1 步，这条
   「适配层必须覆盖这些渠道」的保护会**静默失效**（数字变小但仍然绿）；
2. **模块注释超限**：新增模块的头部注释 11 行 > 棘轮 10 行；
3. **守卫脚本路径漏改**：守卫从 `scripts/` 搬到 `scripts/checks/` 时，
   `verify-all.sh` 里有一行没跟着改 —— 这个 bug 存活了两轮才被跑到；
4. **格式**：一次脚本化编辑留下多余空行，`cargo fmt --check` 报差异。

结论：功能片默认只跑 `./scripts/verify-affected.sh`（改动面定向验证），
但**阶段收尾要跑一次 `./scripts/verify-all.sh`**（默认只验证源码，
冒烟留到提交或打版本时显式开启）；发现的红灯不要「顺手改掉」，
先判断它是**不变量失效**还是纯格式问题 —— 上面第 1 条就属于前者，修的时候要
同时说明新的不变量是什么。

### 2.2 需要外部条件的项

```bash
./scripts/verify-external.sh
```

**SKIP ≠ PASS**：脚本把每条判据、命令与「未执行」状态一起打出来。包括：

| 项 | 判据 | 现状 |
|---|---|---|
| 真机像素路径与延迟 | p50 < 100 ms / p95 < 250 ms（授权后） | 未执行 |
| 真机窗口采集 | 应用名 / 标题 / 窗口图像正确 | 未执行 |
| 8 / 24 / 72 小时长跑 | 无崩溃、RSS 增长 < 10% | 未执行 |
| 真机 4 小时活动数量合理性 | 活动数量与人工观察一致 | 未执行 |
| 打包产物冒烟（录制/助手/摄入） | `./scripts/tests/packaged-smoke-checklist.sh --auto` + `--record` | 未执行 |
| 界面人工确认（托盘/通知/自启） | `./scripts/tests/manual-smoke.sh --launch` 的 8 项清单 | 未执行 |
| 签名与公证、新机安装 | Gatekeeper 通过、可直接安装 | 豁免（无签名身份） |
| 黄金数据集准确率 | Activity ≥ 90% / App ≥ 98% / Stage F1 ≥ 85% | 未测（需人工标注） |

### 2.3 三条容易漏的验证

- **产物启动检查**（`scripts/tests/launch-check-tauri.sh`）：`verify-packaged-app.sh`
  只验「产物里的 daemon 能不能打」，界面拿不到端口、适配层没装全这类问题只有
  把 `.app` 真拉起来才发现 —— 启动期缺陷只在这一步暴露。
  注意它断言的是**外壳 + daemon**，不是「webview 真的画出了界面」：后者仍在
  §7 的人工确认项里。
- **真进程冒烟**（`scripts/tests/smoke-daemon.sh`）：真起 daemon，17+ 条接口
  `code=0`、无 token 一律 401、运行日志落盘且可 grep、退出清理 `runtime.json`。
- **守卫自检**（`scripts/tests/selftest-lints.sh`）：往源码里**真的植入违规**，
  断言每条守卫都会失败 —— 「永远通过的 lint 等于没有 lint」。整条自检约
  **2 分钟**：产物守卫（`check-macos-artifacts`）要体检 80+ 个 Mach-O 产物，
  它并行跑 `vtool`/`otool`；早先串行实现要 35 分钟以上，慢到没人愿意等。

## 3. 出问题先看哪里

```bash
mc-cli doctor                    # 五段自检：config / storage / model / platform / capture
# 设置页「导出诊断包」会生成脱敏 zip（日志尾巴 + info/diagnostics）；
# 也可只拉 JSON：
tail -50 "<数据目录>/logs/daemon.log"
curl -s -H "x-mc-token: <token>" http://127.0.0.1:<port>/api/v1/diagnostics/export
tail -50 ~/Library/Logs/MineContext/main.log     # 桌面外壳日志
```

**运行日志**：`<数据目录>/logs/daemon.log`（> 8 MiB 时启动阶段归档成 `.1`）；
级别用 `MC_LOG`（`trace`…`off`，默认 `info`，`debug` 能看到每轮采集计数、
四道模型闸门决策与检索条数）。每条日志带 `component` 与 `event`，可直接 grep：
`grep 'event="listening"' daemon.log`。

**内容纪律**：日志不写用户内容（截图、消息、提示词、窗口标题）、密钥（API key、
token）与文件系统路径 —— 由 `scripts/check-source.sh` 里的日志卫生守卫把守
（字段名命中 `content`/`prompt`/`token`/`path` 即失败，`.display()` 与未脱敏的
`error.detail()` 同样失败）。

**错误码 → 处置办法**：36 个错误码逐条写在
[`troubleshooting.md`](troubleshooting.md)，`check-docs.sh`
保证它与代码里的错误码集合一致（改了代码没改文档就红）。

## 4. 隐私边界（运维视角）

- **拦截发生在落盘之前**：`privacy.blocked_apps` / `blocked_windows` 命中的观测
  **不落盘**（同轮屏幕帧一起拦），只在诊断里留下「拦了多少条」的计数；
- **脱敏发生在出网之前**：`privacy.redact_patterns` 命中的文本在构造提示词时替换，
  本地记录不受影响（脱敏做在采集或存储位置会损失本地信息）；
- **被拦截的内容不参与检索**，包括向量检索（否则换个方式就能捞回来）；
- 已知缺口：`blocked_domains` 目前只有窗口标题启发式；「屏幕截图里后台运行的
  拉黑应用」尚未拦截。完整口径见
  [`privacy.md`](privacy.md)。

## 5. 发版标签与 CI 打包

版本号以 workspace `Cargo.toml` 为准，并与 `src-tauri/Cargo.toml`、
`frontend/package.json`、`src-tauri/tauri.conf.json` 四处一致
（守卫：`./scripts/checks/check-version-consistency.sh`）。

打新的 `v*` 标签（**不**打包）：

```bash
# 本机预览
./scripts/create-release-tag.sh --dry-run

# 本机创建并推送（会触发下面的 Release workflow）
./scripts/create-release-tag.sh
```

或在 GitHub：**Actions → Tag release → Run workflow**
（`tag-release.yml`；`version` 留空则用仓库版本，`dry_run` 只校验不推送）。

推送 `v*` 后由 **Release (binaries + macOS dmg)**（`release.yml`）自动：

1. 在 `macos-14` / `ubuntu-22.04` / `windows-latest` 构建 `mc-daemon` + `mc-cli`
   （脚本：`./scripts/package-release-binaries.sh`，产物进 `dist/release/<os>-<arch>/`）；
2. 在 `macos-14` 打 Tauri `.dmg`（`./scripts/package-macos-tauri.sh`）：
   **Apple secrets 齐全则 Developer ID 签名并公证**，否则 adhoc 签名、未公证（日志里有
   `WARN: 跳过公证`，构建不失败）；
3. 汇总为 **draft GitHub Release**（zip + dmg；dmg 文件名带 tag 版本，如 `MineContext_1.0.0_aarch64.dmg`）。

**打 tag 前**先把 Cargo.toml / src-tauri / frontend / tauri.conf 四处版本改成与即将打的 `vX.Y.Z` 一致
（`create-release-tag.sh` 会校验；勿绕过脚本直接推一个与仓库版本不符的 tag）。
Release CI 打包前也会跑 `./scripts/create-release-tag.sh --assert-only <tag>`，不一致即失败。

macOS 若提示「已损坏」（仅未公证包）：`xattr -cr ~/Downloads/MineContext_*.dmg` 或
`xattr -dr com.apple.quarantine /Applications/MineContext.app`。

也可对 `release.yml` 手动 `workflow_dispatch` 只打包、不打标签。

### 5.0 签名与公证（维护者：环境变量 / GitHub secrets）

契约与 [Tauri 2 macOS signing](https://v2.tauri.app/distribute/sign/macos/) 一致；
检测逻辑在 `scripts/lib/macos-notarize-env.sh`。**不要**把真实凭据写进仓库。

**签名（CI 必填三项）**

| 变量 | 含义 |
|---|---|
| `APPLE_CERTIFICATE` | Developer ID Application 的 `.p12` 经 `openssl base64 -A -in cert.p12` 后的内容 |
| `APPLE_CERTIFICATE_PASSWORD` | 导出该 `.p12` 时设置的密码 |
| `APPLE_SIGNING_IDENTITY` | 钥匙串身份名，如 `Developer ID Application: Your Name (TEAMID)`（不可为 `-`） |

本机已把证书装进钥匙串时，可只设 `APPLE_SIGNING_IDENTITY`（再加下面的公证认证）。

**公证认证（二选一）**

Apple ID：

| 变量 | 含义 |
|---|---|
| `APPLE_ID` | Apple 账户邮箱 |
| `APPLE_PASSWORD` | [App-specific password](https://appleid.apple.com/account/manage)（勿用登录密码） |
| `APPLE_TEAM_ID` | 团队 ID（开发者账户 Membership） |

App Store Connect API key：

| 变量 | 含义 |
|---|---|
| `APPLE_API_ISSUER` | Users and Access → Integrations 页顶部的 Issuer ID |
| `APPLE_API_KEY` | Key ID |
| `APPLE_API_KEY_PATH` | 本机 `AuthKey_<KEYID>.p8` 路径；或 |
| `APPLE_API_KEY_P8` | 同上 `.p8` 文件全文（CI secret；打包脚本会落到临时路径并设置 `APPLE_API_KEY_PATH`） |

在 GitHub：**Settings → Secrets and variables → Actions** 添加与上表同名的 repository secrets。
`release.yml` 的 dmg 任务会注入这些变量；缺任一必需项时打包脚本打印 WARN 并继续 adhoc 路径。

本地：`export` 上述变量后执行 `./scripts/package-macos-tauri.sh`（齐全则公证，否则 WARN + adhoc）。

## 5.1 发布前还差什么

自动部分已经固化在 `verify-all.sh`（门禁 + 产物 + 启动检查）与
`verify-external.sh`（外部条件项）。打版本前再跑一遍打包产物冒烟清单
（**不**接入日常 `verify-affected`，避免每笔业务提交跑完整冒烟）：

```bash
./scripts/package-macos-tauri.sh --with-smoke   # 出 dmg + 产物启动验收
./scripts/tests/packaged-smoke-checklist.sh --auto   # R1 产物 / R2 daemon·渲染层
# 人确认 R3 录制、R4 助手一条消息、R5 文件或链接摄入：
./scripts/tests/packaged-smoke-checklist.sh --launch --record
```

**人在回路的其余项**：

1. 托盘 / 通知 / 自启观感：`./scripts/tests/manual-smoke.sh --launch`（8 项清单；
   与上表互补，不重复录制/助手/摄入主路径）；
2. 签名与公证：配置 §5.0 的 secrets 后由 Release / `package-macos-tauri.sh` 自动走公证；
   未配置时产物仍为 adhoc，属豁免项，分发需附上面的 `xattr` 说明；
3. 长跑与准确率：8 / 24 / 72 小时 soak 与黄金数据集评测 —— 脚本与判据在本机留档，
   逐项状态属开发过程记录，不进交付仓库。

## 5.9 迁移文件不可修改（含注释）

`crates/mc-storage/migrations/*.sql` 一经发布就是**只读**的：库里记录了每个已应用迁移的
checksum，改动文件（哪怕只改注释、只调空格）会让**已有数据库拒绝启动**，报
`storage_migration_failed: 迁移 N (...) 的 checksum 与已应用的记录不一致`，
表现为「daemon 起不来 → 前端白屏」。

- 要改 schema：**新增一个迁移文件**（`0008_xxx.sql`）；
- 排查时先看 `~/Library/Application Support/MineContext/logs/daemon.log`：
  只有「启动 + 配置已加载」而没有「事件库已打开」，基本就是这条；
- 注意 daemon 的迁移是**编译期嵌进二进制**的（`include_str!`）——
  改完迁移文件必须**重新构建** daemon 才生效。

## 6. 已知缺口（不假装完成）

功能面：补偿推断已有入队与消费者（`POST /api/v1/jobs/backfill` + daemon
`jobs_worker`；界面入口仍缺）；提示词模板的 `config.summary.template_id` 已真正生效并在落库记录里如实标注
（`summary.template_yaml` 给自定义模板，坏 YAML 明确报错）；**file / browser 采集源与
OCR 廉价层未做**（语义未定义，配置点名时启动日志如实报告，不会悄悄退回整屏）。
语义索引覆盖活动、笔记与总结（`kind=activity|document|summary`）。

外壳面：渲染层日志经外壳命令写进 `<日志目录>/renderer.log`
（`~/Library/Logs/com.minecontext.desktop/renderer.log`，同时进 stdout），
启动检查就靠它判断"渲染层真的起来了"；
**托盘状态由 app shell 同步**（采集 SSE + `/api/capture/status`；每 3 秒轮询
`/api/backend/status` 与 `/api/health`，提示/菜单/短标题走产品 i18n：录制中、
索引暂停 `embedding.status=paused`、本地服务不可用）；
**检查更新可用、静默安装不可用**（GitHub Releases 比对 + 打开发布页/dmg；
Tauri updater 静默安装需要签名与公证，仍明确失败）；**多窗口状态同步是 no-op**（单窗口产品，登记为 DEFERRED）；
**`backend:status-changed` 不推送**（渲染层每 3 秒轮询 `/api/backend/status`）。

文档面：根 `README.md` / `README_zh.md` 已按当前形态重写 —— 不再有上游项目的
Python 后端安装章节（`uv sync` 之类），装法与命令都指向本仓库的 Tauri 外壳与 daemon。

工程面：守卫脚本已不依赖系统 Python（内联 Python 全部迁进 `apps/xtask`）；
契约门禁现在**双向**检查 —— 既从适配层渠道表生成夹具，也从渲染层调用点反查
（字面量与枚举成员），表里没有的渠道会让门禁第 1 步直接失败；
规划推演原文（本机留档、不入库）与本机 TDD 证据日志是历史记录，其中的路径与阶段编号
按当时状态保留。

产品与性能的完整结论见 [`product-performance.md`](product-performance.md)。

**界面语言（如实登记）**：前端有轻量 i18n（`frontend/.../i18n/index.ts`，设置页可切 zh/en），
Arco 组件库语言与业务文案同源。后端 `general.locale` 影响提示词与证据语言；
总结模板的字段 label 仍以中文模板为主（模板级多语言未做）。

**语义分类是封闭集合**：活动推断的 `category` 未知值统一归一到「其他」，
英文分类映射回中文标签（同一类活动在中英环境下同标签）。新增语言时要同步这张表。

**样式缺口（迁移遗留，需一次设计走查）**：`search-results__*`、`summary-card__*`、
`assistant-stream__*`、`assistant-history__*` 这几组类名在全部 CSS 里**零定义**，
相关元素目前只吃布局容器的默认样式；这些死类名已删除（对应的结构类名保留在
`data-testid` 上）。总结质量徽标是例外：已改为真正可见的配色（兜底=琥珀、模型=翠绿）。
`vault.css` 的 `.vault-card` 原本写的是无效声明 `display: fixed`（已删）：卡片当前按
普通块级流布局，是否需要固定定位由设计确认。

**待办（todo）链路仍完整接线**：存储、路由（`/api/db/todos`）、前端类型与
数据库表都在。它是上游项目的功能，**产品已决定保留、不下线**（2026-10-07）。

**行为陷阱（如实登记，不是缺陷）**：直接编辑 `<数据目录>/config.toml` **不会**被运行中的
daemon 感知 —— 只有设置页保存（`POST /api/model_settings/update` → `apply_patch`）才会
触发配置重载。启动时读一次，之后不再监听文件变化；改完配置请重启，或走设置页。

**界面健壮性**：渲染层有全局与卡片级错误边界，单个组件崩溃只降级那一块
（显示原因 + 重试），不再整页白屏；崩溃会写进 `renderer.log` 便于排查。
诊断包（`/api/v1/diagnostics/export`）新增 `counts.pending_inference`
（待推断积压量）—— 报「一个 job 跑几小时」时先看这个数。

## 7. 需要真机或外部条件的验证项（可执行判据）

下面这些**不能在本机自动化验证**，因此一直没有标成完成。每项都给出判据，具备条件时可直接执行。

| 项 | 判据（做到什么算通过） | 怎么执行 |
|---|---|---|
| Tauri 外壳观感（**部分已验**） | **已自动验过**：窗口正常显示、**配好模型时跳过引导页进入主界面**（侧边栏 Home / Screen Monitor / Search / Assistant / Summaries / Settings 都在，截图 + 落盘日志双重证据）、daemon 连通（渲染层 0 条 401）、事件流真的接上（`ready` + `push:init-check-data` 两帧）、`pkill -TERM` 后文件清理**且进程真的退出**。**仍待人确认**：托盘菜单四项逐个点击、托盘提示随录制状态变化、关窗后收进托盘且继续采集、**各页面的真实交互**（搜索命中 / 助手逐块出字 / 设置读写 —— 这三项需要真实模型或你现有的数据） | `./scripts/tests/manual-smoke.sh --launch` 会挂 dmg、拉起应用并逐项记录；托盘与通知必须人点人看 |
| 系统通知真的弹出 | 触发一条系统通知（例如窗口失焦时产生事件），macOS 通知中心出现该通知 | 设置页开关打开后操作触发；脚本只能断言「桥已接线」，弹窗与否必须人看 |
| 开机自启真的生效 | 勾选后重启（或注销再登录），应用自动启动；取消勾选后不再启动 | 设置页 Startup 分组切换，再用 `launchctl list | grep -i minecontext` 核对 LaunchAgent |
| 签名与公证 | `codesign -dv --verbose=4` 显示 Developer ID；`spctl -a -vv` 通过；Gatekeeper 无双击拦截 | 配置 §5.0 secrets 后由打包脚本走公证；未配置时 dmg 仍为 adhoc |
| 长跑（8 / 24 / 72 小时） | 期间无 panic；`runtime.json` 端口不漂移；活动数量与人工观察量级相符；磁盘占用增长在预期内 | 真机挂着跑，期间抽查 `/api/diagnostics` |
| 金标准准确率 | 用带标注的数据集算活动标题/分类的准确率，达到约定阈值 | 需标注数据集；当前无 |
| 磁盘将满的真实降级 | 真机把可用空间压到 512 MiB 以下：采集应跳过并写节流失败，恢复后自动继续 | 代码路径已有单测与接线，**未在真机灌满磁盘验证** |

**说明**：以上任何一项都不会因为"代码写了"而被记成完成；本地 TDD 日志里的对应条目保持未勾选状态，直到有真机证据。

## 8. 作业与队列：现状与边界

- **异步作业在内存里**：任意时段总结的异步作业由 `mc-server` 的内存注册表跟踪
  （`AdhocJobs`，`Mutex<Registry>`）。**daemon 重启后未完成的作业会丢失** —— 界面再查
  同一个 job id 会拿不到它。长范围总结请在一次会话内跑完，或改用同步接口。
- **SQLite `jobs` 表已接线补偿推断**：`POST /api/v1/jobs/backfill` 入队，daemon
  `jobs_worker` 租约执行；同范围幂等。**前端仍无「回填」按钮**——只能 API/诊断调用。
- **任意时段总结的异步作业仍在内存里**（见上）：与 SQLite jobs 是两条轨，不要混为一谈。

## 9. 待决策事项（需要产品判断，不是技术阻塞）

下面四项**技术上都能做**，但"要不要这么做"需要产品决定；在决定前保持现状，且现状是**可用且自洽**的。

| 事项 | 选项 | 现状 | 建议与代价 |
|---|---|---|---|
| ~~默认桌面外壳~~（**已决定并执行**） | 曾有三选项：保持 Electron 默认 / 切 Tauri / 移除 Electron | **Electron 已移除**（2026-10-06）：主进程、preload、electron-vite、electron-builder、相关依赖与脚本全部删除，渲染层改纯 `vite` 构建，门禁与 CI 只走 Tauri。渲染层在打包版里的实际观感仍待真机确认（§7） | 已执行；若将来要恢复第二条外壳，需要重新引入主进程/预加载/打包配置三块 —— 这也是当初建议「先真机确认再切」的原因，现按用户决定直接切了 |
| 对话触发总结的进度 | ① 只给提示（现状：文案指明进度在总结页）② 自动跳转总结页 ③ 对话内嵌进度卡 | 提示已做且可测 | ②成本最低但会打断对话；③体验最好但要复用轮询与取消逻辑（约一天量级）；建议先①，收集真实使用反馈再定 |
| 回填作业的界面入口 | 设置页「补推断」（from/to + 入队 + 轮询状态）；亦可直调 API | API + worker + 设置页入口已通 | 自动缺口提示仍可后续加 |
| 浏览器 URL 级隐私规则 | ① 保持降级提示（现状）② 接 Accessibility API 取地址栏 | `blocked_domains` 已能按 host 匹配；没有 URL 来源时如实报告降级 | ②需要真机与辅助功能权限，且要处理"权限被拒"的路径；建议等有真实浏览器使用场景时再做 |

**外部条件项**（第 7 节的判据表）执行顺序建议：先做①Tauri 观感（**现在只剩这一条外壳**，它不通就等于交付不通）+②系统通知是否真的弹出，再做⑤长跑与⑦磁盘降级（能暴露长期问题），最后做④签名公证与⑥金标准准确率（需要外部凭据与数据集）。

## 10. 验证节奏（改多少，验多少）

**默认：定向验证** —— `./scripts/verify-affected.sh`。它按改动面自动选最小检查集：
受影响 crate 的 fmt / clippy / 测试（含一层反向依赖）、前端三层测试与构建、被改动的守卫、
文档与契约新鲜度。改一两行就该在一两分钟内拿到信号；`--list` 可以先看它打算跑什么。

**全量源码验证**（`./scripts/verify-all.sh`）用于阶段收尾与跨切面改动（工作区清单 /
依赖 / 公共契约），默认跳过冒烟与产物扫描。它覆盖契约夹具、守卫自检、fmt、clippy、
Rust 业务测试和前端 lint/typecheck/三层测试/构建。默认 `cargo test --workspace` 同样不会执行 daemon 真进程 E2E。

**提交前**执行 `./scripts/verify-affected.sh --with-smoke`；**打版本或最终交付前**执行
`./scripts/verify-all.sh --with-smoke`。也可独立运行 `./scripts/verify-smoke.sh`，它包含显式
开启 `process-smoke` feature 的 daemon E2E、产物检查、HTTP 冒烟和 Tauri 启动检查。
日常业务开发不执行这些检查。默认验证通过不代表交付验收通过；外部条件 SKIP 也不算通过。

**门禁本身也要快**（不然纪律会被绕过）：

- Rust 测试并行跑（`scripts/tests/rust-tests-parallel.sh`：129 个测试二进制、8 并发）：
  实测 40 分钟 → **7 分 17 秒**，结果一致（1155 条断言全绿）；
- 守卫共用同一个 `xtask` 二进制（`scripts/lib/xtask.sh`）：`check-source.sh` 从
  105 秒降到 **15 秒** —— 原来每条守卫都要付一遍 `cargo run` 的指纹与取锁开销。

### 7.1 剪贴板采集的真机验证（需你操作）

剪贴板源默认关闭，且**必须显式配置**才启用（`capture.sources` 里写上 `clipboard`）。
启用后可按下面步骤验；注意第 2 步会**改写你的剪贴板**，请先用一段无关紧要的文本。

```bash
# 1) 配置里点名剪贴板源（只留 screen 会导致剪贴板不采）
#    capture.sources = ["screen", "clipboard"]

# 2) 往剪贴板放一段测试文本（这会替换你当前复制的内容）
printf 'minecontext-clipboard-probe' | pbcopy

# 3) 等一个采集周期后查库里的剪贴板观测（text_origin = clipboard）
sqlite3 "<数据目录>/data/minecontext.db" \
  "select id, kind, privacy_verdict, substr(text_content,1,40) from observations where kind='clipboard' order by ts desc limit 3;"
```

判据：

- 出现 `kind='clipboard'` 的观测，`text_content` 是那段文本；
- 同一个内容**只出现一条**（连续 tick 不重复入库）；
- 若文本命中 `privacy.redact_patterns`，`privacy_verdict` 必须是 `redacted`，且 `text_content`
  里看不到原文；
- 把 `privacy.redact_patterns` 写成非法正则后重启：**不应**产生新的剪贴板观测，
  而应出现一条「脱敏规则非法，剪贴板内容未存储」的失败记录（fail-closed）。

## 11. 当前可交付状态快照

「可用」指代码路径完整且有测试；「默认关闭」指需要用户在配置里显式点名；「待真机」指
逻辑已实现但效果必须在真机上确认（判据见 §7）。

| 能力 | 状态 | 说明 |
|---|---|---|
| 屏幕 / 窗口采集 + 活动推断 | 可用（**仅 macOS**） | Windows/Linux：`capture_supported=false`，界面禁用开始录制；权限缺失时接口如实报告原因（见 [`context-sources.md`](context-sources.md)） |
| 锁屏暂停采集 | 可用（默认开） | `capture.pause_on_lock`（UI：`pauseOnLock`）；锁屏硬暂停优先于空闲降频；不改 `enabled`，解锁按原状态继续；探测失败按未锁定（宁可多采）；组合见 [`decisions/capture-idle-and-lock.md`](decisions/capture-idle-and-lock.md) |
| 空闲降频 | 可用 | 空闲 ≥ `capture.idle_threshold_secs`（默认 300s）后改用 `capture.idle_interval_secs`（默认 60s）；锁屏时硬暂停优先于降频 |
| 磁盘将满停采 | 可用 | 低于 512 MiB 跳过本轮并写节流失败；**真机灌满未验** |
| 采集状态自述 | 可用 | `GET /api/capture/status` 含 `canRecord` / `status` / `reason`，界面直接显示原因 |
| 任意时段总结 | 可用 | 四条入口：预设、时间轴拖选、深链 `#/summaries?from=&to=`、对话指令 |
| 活动置信度 / 改名 / 合并 / 切分 | 可用 | 时间线内操作，改完回读服务端结果 |
| 总结模板 | 可用 | `summary.template_id` 选内置模板、`summary.template_yaml` 给自定义模板；未知 id 与坏 YAML 明确报错；模板**真正影响产出**并如实记录 |
| 剪贴板采集 | **默认关闭** | 需 `capture.sources` 显式写 `clipboard`；落库前过脱敏，脱敏规则非法时不落库；**真机端到端未验**（§7.1） |
| file / browser 采集源 | 未实现 | 语义未定义（**待产品决策**）；配置点名时启动日志如实报告，不会悄悄退回整屏 |
| OCR 廉价抽取层 | 未实现 | 截图理解当前全走视觉模型 |
| 后台拉黑应用的截图拦截 | 未实现 | 依赖视觉识别，需真机验证 |
| 开机自启 / 系统通知（Tauri） | 可用 | 设置页 Startup 分组；**是否真的弹出/真的自启需真机确认**（§7） |
| Tauri 外壳 | **唯一外壳** | 托盘（显示窗口 / 开始-暂停录制 / 屏幕监控 / 退出）、单实例、通知、自启、关窗收进托盘、未签名 dmg 均已实现；渲染层在打包版里的观感待真机确认（§7） |
| 托盘快捷动作 / 托盘状态 | 可用 | 菜单事件由外壳发、业务由渲染层做（不复制采集开关）；录制/索引暂停/断连由 app shell 同步（§6） |
| 检查更新 | 可用 | 设置页对照 GitHub Releases；有新版本时打开发布页 / dmg（§6） |
| 静默自动安装 | 未实现 | 需要签名与公证；`quitAndInstall` / `cancelDownload` 显式失败（§6） |
| 渲染层日志落盘 | 可用 | 走外壳 `renderer_log` 命令写 `~/Library/Logs/com.minecontext.desktop/renderer.log`（同时进 stdout）；外壳不在时只写控制台，日志器本身不依赖外壳（§6） |
| 多窗口状态同步 | no-op | 单窗口产品；`store-sync:*` 四个渠道登记为 DEFERRED，并写明原因（§6） |
