# 故障排查（面向用户）

**先跑这两条，八成的问题当场有答案**：

```bash
mc-cli doctor                  # 本机自检：配置 / 存储 / 模型 / 平台 / 采集
curl -s -H "x-mc-token: $(python3 -c "import json;print(json.load(open('<data_dir>/runtime.json'))['token'])")" \
  http://127.0.0.1:<port>/api/diagnostics | python3 -m json.tool
```

- `doctor` 回答「这台机器能不能跑、缺什么」；
- `/api/diagnostics` 回答「跑起来之后哪一步失败了」——它的 `recent_failures`
  列出最近 20 条失败（组件、错误码、人话说明、建议），`platform` 报告系统
  是否受支持，`retention` 报告图片轮转结果，`capture.stats` 报告采集统计。

**支持范围**：最低 macOS 13，覆盖 13 / 14。低于 13 会在 `doctor` 的 `platform`
段落直接标为 `unsupported`。

---

## 错误码对照表

表中「用户可见文案」与「建议」直接来自 `mc-common::error`
（`ErrorCode::user_message` / `remediation`），因此**不会与实现漂移**。
技术细节（`detail`）只在日志与 `/api/diagnostics` 里出现，不展示给用户。

| 错误码 | 用户可见文案 | 建议 |
|---|---|---|

| `config_invalid` | 配置有误，请检查后重试。 | — |
| `config_unreadable` | 无法读取配置文件。 | — |
| `config_unknown_timezone` | 配置中的时区无法识别，请使用标准的 IANA 时区名称（例如 Asia/Shanghai）。 | 请选择有效的时区。 |
| `capture_permission_denied` | 缺少屏幕录制权限，无法截图。 | — |
| `capture_no_display` | 当前没有可用的显示器。 | 接上显示器或唤醒屏幕后会自动恢复。 |
| `capture_locked` | 屏幕已锁定，录制已暂停。 | — |
| `capture_timeout` | 截图超时，正在重试。 | — |
| `capture_io` | 写入截图文件失败。 | — |
| `capture_unsupported` | 当前系统不支持该采集方式。 | 请在采集设置中改用其它采集方式。 |
| `capture_black_frame` | 截图内容为空（可能是权限未生效），请检查权限后重试。 | 在「系统设置 → 隐私与安全性 → 屏幕录制」中勾选本应用，然后重启应用。 |
| `provider_unconfigured` | 尚未配置模型服务，AI 功能暂不可用。 | 在「设置 → 模型」中填写 Base URL、API Key 与模型名。 |
| `provider_auth_failed` | 模型服务的 API Key 无效或已过期。 | 请更新 API Key。 |
| `provider_not_found` | 指定的模型不存在或无权访问。 | 请确认模型名称，或在服务端开通该模型。 |
| `provider_rate_limited` | 模型服务限流，将稍后自动重试。 | 无需操作，系统会自动重试；也可以在设置中降低截图频率或提高预算。 |
| `provider_timeout` | 模型服务响应超时，将自动重试。 | — |
| `provider_connection` | 无法连接到模型服务，请检查网络或代理设置。 | 请检查网络连接与代理设置。 |
| `provider_server_error` | 模型服务暂时异常，将自动重试。 | — |
| `provider_invalid_response` | 模型返回的内容无法解析，已保留原始结果。 | — |
| `provider_unsupported` | 该模型服务不支持所需能力。 | 请改用支持图像输入的模型。 |
| `storage_unavailable` | 本地数据库暂时不可用，正在重试。 | — |
| `storage_corrupt` | 本地数据库损坏，已进入只读安全模式。 | 请从备份恢复数据，并导出诊断包以便排查。 |
| `storage_disk_full` | 磁盘空间不足，已暂停写入截图。 | 请清理磁盘空间，或调低截图保留天数。 |
| `storage_migration_failed` | 数据库升级失败。 | — |
| `storage_legacy_source_missing` | 找不到旧版本的数据（升级向导会告诉你找过哪些路径）。 | 请确认旧版本的数据目录（含 persist/sqlite/app.db 与 screenshots/）后重试。 |
| `storage_invalid_blob_path` | 截图引用无效。 | 该引用不是本应用生成的截图路径；若界面显示异常请重新打开应用。 |
| `storage_blob_missing` | 截图文件不存在（可能已被自动清理）。 | 截图已被自动清理；可调高「保留天数」以避免再次发生。 |
| `storage_embedding_dimension_mismatch` | 向量维度与已建索引不一致（通常是换了 embedding 模型），需要重建语义索引。 | 请在「设置 → 模型」改回原来的 embedding 模型， 或在「设置 → 检索」里重建向量索引；重建前检索会退化为关键词检索。 |
| `domain_invalid_range` | 时间范围无效。 | — |
| `domain_invalid_timestamp` | 时间格式无法识别。 | — |
| `domain_invariant_violated` | 内部状态不一致。 | — |
| `domain_nothing_to_do` | 所选时间段内没有可总结的内容。 | 换一个时间段再试。 |
| `domain_invalid_override` | 这次修改无法应用，请检查后重试。 | — |
| `provider_unavailable_fallback` | 模型暂时不可用，已用本地内容生成了一份摘要。 | — |
| `budget_exceeded` | 已达到本时段的用量上限，将降低分析频率。 | 可以调高每小时/每日上限，或降低截图频率。 |
| `privacy_blocked` | 该内容命中隐私规则，已跳过。 | — |
| `privacy_rule_engine_failed` | 隐私规则加载失败，已按最保守策略处理。 | 请检查隐私规则配置；在此之前所有内容都不会外发。 |

---

## 常见场景

### 一直有截图，但没有阶段总结

按这个顺序查：

1. `mc-cli doctor` → `capture` 段是否 ready；`storage` 是否只读；
2. `/api/diagnostics` → `invariants.stages_without_summary` **必须为 0**
   （非 0 说明「有阶段没总结」这条最高优先级不变量被破坏）；
3. `recent_failures` 里的 `summary` 组件 → 若出现
   `provider_unavailable_fallback`，说明模型不可用但**已经用本地内容兜底**
   （这是正常降级，不是故障）；
4. `components.provider.status` 为 `unconfigured` → 未配置模型，
   此时总结应该是「本地兜底」而不是没有。

> `stages_without_summary` 长期非 0 才是真故障；偶发 1 会在下一轮巡检被补上
> （巡检是第三道防线）。

### 截图是黑的 / 采集看起来在跑但没有内容

macOS 在缺少屏幕录制权限时**不报错，只返回全黑帧**。查
`capture_black_frame` 与 `capture_permission_denied`；
`doctor` 的 `capture` 段落会直接告诉你权限状态。
授权路径：系统设置 → 隐私与安全性 → 屏幕录制（勾选后**需要重启终端/应用**）。

### 检索搜不到东西

1. 关键词检索永远可用；语义检索需要 embedding 配置 + 索引已建；
2. `/api/diagnostics` 的 `recent_failures` 里若有 `embedding` 组件，
   看是否是 `storage_embedding_dimension_mismatch`（换过 embedding 模型）
   —— 那是要**重建索引**的状态，不是故障；
3. 被隐私规则拦截的内容**刻意**搜不到（`privacy_blocked`），这是设计行为。

### 磁盘一直涨 / 图片没有被清理

`capture.retention_days = 0` 表示**永久保留**。检查
`/api/diagnostics` 的 `retention` 段：`last_run_at` 为空说明轮转还没跑过；
`deleted_files` / `freed_bytes` 是最近一次的结果。手动删单张用
`DELETE /api/capture/screenshots?path=<相对路径>`。

### 改了设置但行为没变

设置写回的是用户配置文件（默认 `<data_dir>/config.toml`）并**热重载**：

- 采集间隔、录制时段、显示器选择会立即生效；
- 若 `doctor` 报 `config` 有提醒/失败，说明配置没被接受，改动不会落盘
  （校验失败时**不会**写文件，也不会改变内存里的配置）。

### 装在 macOS 12 上

`doctor` 的 `platform` 段会显示 `unsupported`，并给出最低要求。
本仓库的二进制按 macOS 13.0 构建（`minos 13.0`），在更低版本上不保证可用。

---

### 全量测试跑得特别慢，像挂住了

`scripts/verify-all.sh` 的第 5 步有 45 分钟上限：**挂住会变成可见的失败**，
而不是把门禁卡到天荒地老（当时排查记录见本机 TDD 日志）。真遇到「慢到像挂住」时，
先看机器状态，再看代码：

```bash
vm_stat | head -4          # Pages free 只剩几万页（几百 MB）时，测试会在换页里挣扎
ps aux | sort -k3 -nr | head -5   # 看有没有 spindump / logd / CacheDelete 在抢 CPU
uptime                     # load average 明显高于核数时，慢是环境而不是回归
```

本机实测过两次：一次是 `tccd`（屏幕录制权限守护进程）单核 90%+，
一次是 `CacheDelete` + `spindump` 同时跑（内存只剩几十 MB）。
两次都是**同一个提交重跑即过**，与代码无关。判断顺序因此是：
先排除环境，再用单个 crate 复现（`cargo test -p <crate> --test <name>`），
最后才怀疑挂起。

## macOS 安装后白屏与权限排查

以下命令以安装在 `/Applications/MineContext.app` 为例；安装位置不同时替换路径。

### 实时查看前端与后端日志

```bash
tail -F "$HOME/Library/Logs/com.minecontext.desktop/renderer.log" \
  "$HOME/Library/Application Support/MineContext/logs/daemon.log"
```

`renderer.log` 记录前端和外壳日志，`daemon.log` 记录后端启动、数据库和采集错误。首次启动前文件可能尚不存在，`tail -F` 会继续等待；按 `Ctrl+C` 结束查看。若设置了 `MC_DATA_DIR`，后端日志改为该目录下的 `logs/daemon.log`。

若文件日志不足，先完全退出应用，再从终端启动以捕获早期启动失败的 stderr：

```bash
/Applications/MineContext.app/Contents/MacOS/minecontext-shell \
  2>&1 | tee /tmp/minecontext-startup.log
```

不要同时启动多个实例。分享日志前检查是否包含个人信息；不要公开 `runtime.json` 中的认证 token，也不要通过删除数据目录排查白屏。

### 未签名应用被 macOS 拦截

仅对自己编译或已确认可信的安装包操作。优先使用系统提供的「打开」或「仍要打开」确认入口；需要移除这个应用的下载隔离属性时：

```bash
xattr -dr com.apple.quarantine "/Applications/MineContext.app"
```

若明确提示文件权限不足，且已核对安装路径，可仅对该应用使用管理员权限：

```bash
sudo xattr -dr com.apple.quarantine "/Applications/MineContext.app"
```

这只移除指定应用的隔离属性，不是签名、公证，也不会授予屏幕录制权限。不要关闭全局 Gatekeeper、SIP 或修改系统隐私数据库；该命令不能修复数据库迁移失败。

### 可执行文件缺少执行权限

仅在启动明确报 `Permission denied` 且检查发现缺少执行位时使用：

```bash
ls -l "/Applications/MineContext.app/Contents/MacOS/minecontext-shell" \
  "/Applications/MineContext.app/Contents/Resources/backend/mc-daemon"

chmod u+x "/Applications/MineContext.app/Contents/MacOS/minecontext-shell" \
  "/Applications/MineContext.app/Contents/Resources/backend/mc-daemon"
```

文件不存在时应检查打包内容或重新安装，而不是创建空文件。不要对整个应用或数据目录执行 `chmod -R 777`。

### 屏幕录制等系统隐私权限

在「系统设置 → 隐私与安全性 → 屏幕录制」（部分系统显示为「屏幕与系统音频录制」）中，为系统实际列出的应用或采集进程授权，然后完全退出并重新打开应用。通过终端运行时也要检查系统提示的请求方。

`chmod` 和 `xattr` 都不能代替这一授权。辅助功能权限只在功能实际要求时开启，不要把完全磁盘访问权限当作白屏的通用修复。

## 启动时报迁移 checksum 不一致

`storage_migration_failed` 表示后端拒绝打开数据库；界面白屏可能是后端未就绪的表现，不能仅据此认定前端资源损坏。

本版本严格校验迁移，不提供旧 checksum 兼容白名单。相同 SQL 字节的 checksum 不因安装机器改变；升级时修改已应用的迁移文件（包括注释）才会导致差异。后续版本必须新增迁移，不得修改已发布迁移。

重装或换机前，可先在设置页「数据备份」导出笔记树与已导入文件（`GET /api/v1/vault/export` 的 zip），新环境再导入。该包只含 vault 笔记与 `uploads/`，不含聊天、采集截图与模型密钥。

如果选择放弃旧版数据、全新安装，先从托盘完全退出应用和独立启动的 daemon。删除 `.app` 或清缓存不会删除数据库；`Application Support/MineContext` 包含笔记、聊天、采集记录和配置，不是缓存。以下操作将旧数据移到备份，下一次启动创建全新数据库：

```bash
backup="$HOME/MineContext-backup-$(date +%Y%m%d-%H%M%S)"
mkdir -m 700 "$backup" && \
  mv "$HOME/Library/Application Support/MineContext" "$backup/data"
```

确认备份成功后，在 Finder 中删除 `/Applications/MineContext.app`，按需删除 `~/Library/Caches/com.minecontext.desktop`（如存在），从新 DMG 拖入新版本再启动。不要立即删除备份，也不要把旧数据库复制回新数据目录，否则仍会遇到 checksum 错误。设置过 `MC_DATA_DIR` 时，应备份和重置其指向的目录，而非默认目录。不要手工改 checksum 或清空迁移表。

打包默认不运行启动测试。显式运行 `./scripts/package-macos-tauri.sh --with-smoke` 时，编译成功与启动验收通过是两回事：验收失败会返回非零，即使 DMG 已生成也不能视为交付通过。`20 秒内没有 runtime.json` 需查看脚本打印的临时诊断目录（`shell.log` 和 `logs/daemon.log`），而不是默认应用日志。清理时的 `Terminated: 15` 表示脚本发出 SIGTERM，不是启动失败的根因。

默认后端日志在 `~/Library/Application Support/MineContext/logs/daemon.log`；前端/外壳日志在 `~/Library/Logs/com.minecontext.desktop/renderer.log`。`MC_DATA_DIR` 会改变后端的数据和日志位置。

## 上报问题时请附上

### 后台轮询与模型鉴权日志

- `GlobalEventService` 的旧版 `/api/events/fetch` 接口可能返回 `not_implemented` 和 `data: null`。渲染层会停止该旧接口的轮询并提示一次，不把未实现伪装成空事件成功；活动和总结的 SSE 推送不受影响。
- `kind="embedding" status=401` 是上游模型服务拒绝凭据，不是本机 `X-MC-Token` 失效。核对 embedding 的服务地址、密钥及模型权限；不要把 API Key 发到日志或反馈里。后台索引在鉴权失败后暂停，保留诊断记录，配置成功重载或重启后再尝试，不再每个周期重复请求。
- 当前模型设置保存密钥到私有 `model-keys.json`，配置只保存 Keychain 引用；仅保存设置并不代表密钥已导入系统钥匙串。按配置引用更新钥匙串中的密钥，完成后删除侧车明文文件，再重新保存配置或重启。不同模型服务的凭据不可混用。
- `Screen Recording permission not granted` / UI「0 张截图已采集」且界面卡住：在「系统设置 → 隐私与安全性 → 屏幕录制」中勾选 **MineContext**，若列表里还有 **mc-daemon**（或助手/后台进程）也一并勾选；改完后从菜单栏托盘**完全退出**再重开（仅关窗口不够）。未授权时 macOS 可能返回黑帧或只有壁纸的桌面、采集线程挂起；应用不能代替系统授权。

采集或向量索引报错时，如果数据库连失败记录也无法保存，运行日志会出现
`event=failure_record_write_failed`。`component` 区分 `capture` / `embedding`，
`original_code` 是原始业务错误码，`code` 是诊断写入错误码，`at_ms` 是事件时间。
这条日志不包含原始错误正文、路径、标题或密钥；写诊断失败不会再次写库或中断后台循环。
因此诊断列表为空不等于没有失败，还应检查运行日志和存储状态。

1. `mc-cli doctor` 的完整输出；
2. **诊断包**（可以直接贴给别人，里面不含内容）：

   ```bash
   curl -s -H "x-mc-token: <token>" http://127.0.0.1:<port>/api/v1/diagnostics/export
   ```

   它带 `schema_version`、平台信息、组件状态、各类计数、不变量、保留策略结果与
   最近失败的**错误码 + 用户文案**；响应体里的 `excluded` 字段列明刻意不放的东西
   （标题、文本内容、文件路径、密钥、截图内容）。
3. `sw_vers` 与是否插电、机器负载（`uptime`）—— 延迟类问题脱离环境无法判断。

需要更细的现场时再附 `/api/diagnostics`（那是给自己看的，字段更多，
含配置警告与队列水位）。
