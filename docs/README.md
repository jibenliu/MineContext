# MineContext 文档

> 交付形态一句话：**Tauri 2 桌面外壳 + Rust 守护进程（`mc-daemon`）**。外壳只负责
> 窗口、托盘与 daemon 生命周期；渲染层是纯 `vite` 产物，通过本机 HTTP + SSE 与 daemon
> 说话。没有 Python 后端，也没有第二个后端路径。
> 打包：`./scripts/package-macos-tauri.sh`（未签名 `.dmg`）—— 用法与产物说明见
> [`operations.md`](operations.md) §1。

## 从这里开始

| 你想知道 | 看这份 |
|---|---|
| 这东西干什么、解决了哪些问题、性能与成本的账、和同类产品比在哪 | [`product-performance.md`](product-performance.md) |
| 怎么构建、怎么跑、验过什么、没验什么、出问题先看哪里、发布前还差什么 | [`operations.md`](operations.md) |
| 架构与领域模型（分层、事件溯源、Activity/Stage/Summary、保留策略、并发与背压、取舍） | [`architecture.md`](architecture.md) |
| 控制面接口、鉴权与信封；前端渠道映射、适配层与启动顺序 | [`api-and-frontend.md`](api-and-frontend.md) |
| 已交付的上下文来源与仍需外部账号/硬件的方向 | [`context-sources.md`](context-sources.md) |
| 外部条件项怎么验（真机、签名、长跑） | [`operations.md`](operations.md) §2.2 与 §5 |
| 发布清单与豁免项 | [`operations.md`](operations.md) §5 |
| 出错先查什么（36 个错误码逐条） | [`operations.md`](operations.md) §3 → [`troubleshooting.md`](troubleshooting.md) |
| 隐私说明（采集、拦截、脱敏、上传边界） | [`operations.md`](operations.md) §4 → [`privacy.md`](privacy.md) |
| 前端测试怎么分层、契约夹具怎么生成 | [`api-and-frontend.md`](api-and-frontend.md) §2.2、§3 |
| 某一处设计为什么这么取舍 | [`architecture.md`](architecture.md) §12 → [`decisions/`](decisions/) |

本目录只放**交付物**：使用者看的前四篇 + 需要随代码一起交付的参考文档（排查、隐私、发布清单、
外部验证判据、注释风格、交付说明、决策记录）。规划推演原文、阶段规划、TDD 证据日志这些
开发过程中间产物不入库，只在本机留档。

## 三条命令

```bash
./scripts/verify-affected.sh   # 定向验证：按改动面选最小检查集（日常改动用这个）
./scripts/verify-all.sh        # 全量门禁：契约、四类守卫、fmt、clippy、Rust 测试、真进程冒烟、产物启动检查、前端
./scripts/verify-external.sh   # 需要外部条件的项（SKIP ≠ PASS）
./scripts/package-macos-tauri.sh  # 出 dmg（未签名）并跑产物启动检查
```

## 目录

```
docs/
├── README.md                  ← 你在这里
├── product-performance.md     产品与性能（合并 problem/simlar/suggestion）
├── architecture.md            架构（分层 / 事件溯源 / 领域模型 / 存储 / 并发）
├── api-and-frontend.md        控制面接口与前端（鉴权 / 信封 / 渠道映射 / 启动顺序）
├── context-sources.md         已交付上下文来源与外部依赖边界
├── operations.md              运维与交付（构建/验证/排查/隐私/发布/缺口）
├── troubleshooting.md         36 个错误码逐条排查（门禁校验一致性）
├── privacy.md                 隐私说明（采集/拦截/脱敏/上传边界）
├── comment-style.md           注释风格规则（门禁依据）
└── decisions/                 决策记录（性能基线、向量规模）
```
