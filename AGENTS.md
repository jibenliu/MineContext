# 本仓库的工作约定（人与 AI 助手都适用）

MineContext 是面向交付的产品仓库：仓库里的东西是「交付物」，不是开发过程档案。

## 1. 验证纪律（硬要求）

**改多少，验多少。** 默认跑定向验证，不跑全量门禁。

| 改动面 | 跑什么 | 量级 |
|---|---|---|
| 一两行源码 / 单个 crate | `./scripts/verify-affected.sh`（自动按改动面选检查） | 秒～分钟 |
| 单条守卫 / 单个脚本 | 直接跑那个脚本，例如 `./scripts/checks/check-comment-style.sh` | 秒 |
| 阶段或里程碑收尾、跨切面改动 | `./scripts/verify-all.sh`（源码与业务测试，默认不跑冒烟） | 按改动面 |
| 提交前 | `./scripts/verify-affected.sh --with-smoke` | 按改动面 |
| 打版本、最终交付前 | `./scripts/verify-all.sh --with-smoke` | 十几分钟～ |
| 打包与真机 | `./scripts/package-macos-tauri.sh`、`./scripts/tests/manual-smoke.sh --record` | 按需 |

- **不要**为了一两行改动跑全量门禁；那是把交付节奏拖垮的主因。
- 定向验证跑完要如实说明「没跑什么」，不得把定向通过说成全量通过。
- `./scripts/verify-affected.sh --list` 可以先看它打算跑什么。
- 全量门禁留给阶段收尾：那时一次性跑，红了才知道是这次改动引入的。
- 冒烟、daemon 真进程 E2E、产物启动与产物扫描默认不参与业务开发验证。只在提交、打版本或最终交付前显式用 `--with-smoke` 或 `./scripts/verify-smoke.sh`；不得在每轮业务改动后自动跑。

## 2. TDD 纪律（硬要求）

- 先写会失败的测试，再做实现；测试要能失败（空测试等于没有测试）。
- 红→绿证据写在本地 `docs/internal/tdd-log.md`，**不进 git**（见第 3 条）。
- 证据格式：改了什么、跑了哪条命令、输出是什么；不写感想。
- 例外只有三种：纯脚手架（CI/工作区清单）、纯文档、一次性探针。**「时间紧」不是例外**，但「时间紧」也不是跑全量门禁的理由——两者用定向验证同时满足。

## 3. 文档只留交付物

- 中间产物（改进计划、清理清单、审计报告、TDD 日志、阶段规划）**不进 git**：本机留档即可，`.gitignore` 已覆盖。
- 进仓库的文档必须面向使用者或维护者：`README*`、`docs/*.md`（含 `docs/decisions/`）。
  `docs/internal/` 整个目录是本机留档、不入库。
- 注释面向交付：写「为什么」与契约，不写开发过程叙事（守卫 `check-comment-style.sh` 会拦）。

## 4. 提交

- 一个批次一次提交，提交信息说「改了什么、为什么」，不贴命令流水。
- 提交前跑对应的定向验证并显式加 `--with-smoke`；阶段收尾提交前跑 `./scripts/verify-all.sh --with-smoke`。
