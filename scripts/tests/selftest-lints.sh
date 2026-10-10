#!/usr/bin/env bash
# lint 脚本的自检。
#
# 一个「永远通过」的 lint 等于没有 lint。所以这里植入真实违规，
# 断言脚本**确实会失败**，然后清理。
set -euo pipefail

cd "$(dirname "$0")/../.."

# 探针与备份必须在**任何**退出路径上被清掉：残骸会让后续门禁报出难解释的红
# （残留的探针会被注释风格/日志卫生当成真源码），也会污染工作树。
probe_dir=crates/mc-common/src
cleanup() {
  rm -f "$probe_dir/__lint_probe.rs" \
    crates/mc-common/tests/__lint_probe.rs \
    crates/mc-pipeline/prompts/__lint_probe.md \
    scripts/__lint_probe.sh \
    frontend/src/renderer/src/assets/__lint_probe.css \
    docs/internal/__lint_probe_tdd-log.md
  git rm -f --cached docs/internal/__lint_probe_tdd-log.md >/dev/null 2>&1 || true
  rmdir docs/internal 2>/dev/null || true
  find . -name '*.selftest-backup' -not -path './node_modules/*' -not -path './target/*' 2>/dev/null |
    while read -r backup; do mv "$backup" "${backup%.selftest-backup}"; done
}
trap cleanup EXIT

fail=0

expect_fail() {
  local script="$1" probe="$2" content="$3"
  printf '%s\n' "$content" > "$probe"
  if "$script" >/dev/null 2>&1; then
    # 变量名后面紧跟全角括号时，bash 3.2 会把括号的字节当成变量名的一部分
    # （`probe）：unbound variable`）。用 ASCII 括号 + 花括号界定，别踩这个坑。
    echo "FAIL: $script 未能拦截植入的违规 (${probe})"
    fail=1
  else
    echo "PASS: $script 正确拦截了植入的违规"
  fi
  rm -f "$probe"
}

expect_pass() {
  local script="$1"
  if "$script" >/dev/null 2>&1; then
    echo "PASS: $script 在干净代码上通过"
  else
    echo "FAIL: $script 在干净代码上失败"
    fail=1
  fi
}

mkdir -p "$probe_dir"

expect_fail scripts/checks/check-no-naive-datetime.sh "$probe_dir/__lint_probe.rs" \
  'pub fn bad() -> chrono::NaiveDateTime { todo!() }'
expect_pass scripts/checks/check-no-naive-datetime.sh

expect_fail scripts/checks/check-no-naive-datetime.sh "$probe_dir/__lint_probe.rs" \
  'pub fn bad() -> u64 { SystemTime::now().elapsed().unwrap().as_secs() }'
expect_pass scripts/checks/check-no-naive-datetime.sh

# 注释里提到被禁类型名不应误报
printf '%s\n' '// 我们不允许 NaiveDateTime 出现在这里' > "$probe_dir/__lint_probe.rs"
if scripts/checks/check-no-naive-datetime.sh >/dev/null 2>&1; then
  echo "PASS: 注释中的类型名不会误报"
else
  echo "FAIL: 注释中的类型名被误报"
  fail=1
fi
rm -f "$probe_dir/__lint_probe.rs"

# 孤儿提示词：文件存在但没人 include_str! 它 —— 必须被拦截
mkdir -p crates/mc-pipeline/prompts
printf '%s\n' "orphan prompt" > crates/mc-pipeline/prompts/__lint_probe.md
if scripts/checks/check-prompt-embedded.sh >/dev/null 2>&1; then
  echo "FAIL: check-prompt-embedded.sh 未能发现孤儿提示词"
  fail=1
else
  echo "PASS: check-prompt-embedded.sh 正确拦截了孤儿提示词"
fi
rm -f crates/mc-pipeline/prompts/__lint_probe.md
rmdir crates/mc-pipeline/prompts 2>/dev/null || true

for s in scripts/checks/check-dep-direction.sh scripts/checks/check-prompt-embedded.sh \
         scripts/checks/check-provider-purity.sh scripts/checks/check-summary-invariant.sh; do
  expect_pass "$s"
done

# macOS 支持范围：把 deployment target 调低必须被拦住
# （检查脚本读的是固定路径，因此这里备份-替换-还原）
cp .cargo/config.toml .cargo/config.toml.selftest-backup
printf '[env]\nMACOSX_DEPLOYMENT_TARGET = "11.0"\n' > .cargo/config.toml
if scripts/checks/check-macos-target.sh >/dev/null 2>&1; then
  echo "FAIL: scripts/checks/check-macos-target.sh 未能拦截调低的部署目标"
  fail=1
else
  echo "PASS: scripts/checks/check-macos-target.sh 正确拦截了调低的部署目标"
fi
mv .cargo/config.toml.selftest-backup .cargo/config.toml

if scripts/checks/check-macos-target.sh >/dev/null 2>&1; then
  echo "PASS: scripts/checks/check-macos-target.sh 在干净配置上通过"
else
  echo "FAIL: scripts/checks/check-macos-target.sh 在干净配置上失败"
  fail=1
fi

# 故障排查文档：临时改坏一个错误码文案必须被拦
doc=docs/troubleshooting.md
cp "$doc" "$doc.selftest-backup"
# 用 sed 原地改坏一处文案（只为改这一处文本，不引入别的依赖）
sed 's/配置有误，请检查后重试。/配置错了。/' "$doc.selftest-backup" > "$doc"
if scripts/checks/check-troubleshooting-freshness.sh >/dev/null 2>&1; then
  echo "FAIL: scripts/checks/check-troubleshooting-freshness.sh 未能拦截文案漂移"
  fail=1
else
  echo "PASS: scripts/checks/check-troubleshooting-freshness.sh 正确拦截了文案漂移"
fi
mv "$doc.selftest-backup" "$doc"
if scripts/checks/check-troubleshooting-freshness.sh >/dev/null 2>&1; then
  echo "PASS: scripts/checks/check-troubleshooting-freshness.sh 在干净文档上通过"
else
  echo "FAIL: scripts/checks/check-troubleshooting-freshness.sh 在干净文档上失败"
  fail=1
fi

# 前端样式变量：植入「未定义变量」与「裸用 Arco 三元组」必须被拦
theme_probe=frontend/src/renderer/src/assets/__lint_probe.css
expect_fail scripts/checks/check-theme-tokens.sh "$theme_probe" \
  '.a { color: var(--text-color-text-1); }'
expect_fail scripts/checks/check-theme-tokens.sh "$theme_probe" \
  '.a { background-color: var(--primary-6); }'
expect_pass scripts/checks/check-theme-tokens.sh

# 注释风格：植入过程叙事必须被拦
expect_fail scripts/checks/check-comment-style.sh "$probe_dir/__lint_probe.rs" \
  '//! 本轮踩到了一个问题'
expect_pass scripts/checks/check-version-consistency.sh
expect_pass scripts/checks/check-comment-style.sh

# shell 变量展开：`$var（` 这种写法（bash 3.2 会把全角标点当变量名）必须被拦
# 探针内容放在 fixtures/ 里：它本身含违规写法，放在 scripts/ 下会被守卫自己扫到
expect_fail scripts/checks/check-shell-vars.sh "scripts/__lint_probe.sh" \
  "$(cat fixtures/lint/shell-var-probe.txt)"
expect_pass scripts/checks/check-shell-vars.sh

# 函数注释长度棘轮：9 行的函数注释（超过 8 行上限）必须被拦
fn_doc="$(for i in $(seq 1 9); do printf '/// 契约说明第 %d 行\n' "$i"; done)"
expect_fail scripts/checks/check-comment-style.sh "$probe_dir/__lint_probe.rs" \
"$fn_doc
pub fn long_doc() {}"

# 交付向注释：开发过程引用（阶段编号 / 内部文档号 / 规划文档名 / 新旧对比 / 切片号）必须被拦
expect_fail scripts/checks/check-comment-style.sh "$probe_dir/__lint_probe.rs" \
  '//! Phase 4 / 4.51：先做这个'
expect_fail scripts/checks/check-comment-style.sh "$probe_dir/__lint_probe.rs" \
  '/// 见 docs/internal/12-concurrency.md §7'
expect_fail scripts/checks/check-comment-style.sh "$probe_dir/__lint_probe.rs" \
  '// 旧实现先判超长，这里改成按时刻先后结算'
expect_fail scripts/checks/check-comment-style.sh "$probe_dir/__lint_probe.rs" \
  '// problem.md #15 的对策'
expect_fail scripts/checks/check-comment-style.sh "$probe_dir/__lint_probe.rs" \
  '// 切片 87：注释长度棘轮'
# tests/ 也在扫描范围内：测试文件里的开发过程叙事同样要拦
expect_fail scripts/checks/check-comment-style.sh "crates/mc-common/tests/__lint_probe.rs" \
  '//! Phase 2：测试文件里的阶段编号同样要拦'

# 安全属性门禁：把某个被承诺的安全测试改名（等于它不再存在）必须被拦
security_test=crates/mc-server/tests/security_surface.rs
cp "$security_test" "$security_test.selftest-backup"
# 用 sed 把测试函数改名（等于它不再存在）
sed 's/async fn every_api_route_requires_the_token(/async fn every_api_route_requires_the_token_renamed(/' "$security_test.selftest-backup" > "$security_test"
if [ ! -f scripts/checks/check-security-tests.sh ]; then
  # 脚本不存在时「运行失败」也算拦截成功 —— 那种探针证明不了任何东西
  echo "FAIL: scripts/checks/check-security-tests.sh 不存在，探针无法证明它能拦住违规"
  fail=1
elif scripts/checks/check-security-tests.sh >/dev/null 2>&1; then
  echo "FAIL: scripts/checks/check-security-tests.sh 未能发现被承诺的安全测试不见了"
  fail=1
else
  echo "PASS: scripts/checks/check-security-tests.sh 正确拦截了「安全测试消失」"
fi
mv "$security_test.selftest-backup" "$security_test"
expect_pass scripts/checks/check-security-tests.sh

# 模块注释长度棘轮：新增的「11 行模块注释」必须被拦
long_header="$(for i in $(seq 1 11); do printf '//! 第 %d 行\n' "$i"; done)"
expect_fail scripts/checks/check-comment-style.sh "$probe_dir/__lint_probe.rs" "$long_header"

# 棘轮不许留陈旧条目：已经收敛（≤ 10 行）的文件仍留在白名单里必须被拦，
# 否则白名单只会越滚越长，收敛进度无从保证。
cp scripts/comment-length-allow.txt scripts/comment-length-allow.txt.selftest-backup
printf '%s\n' 'crates/mc-common/src/__lint_probe.rs' >> scripts/comment-length-allow.txt
printf '%s\n' '//! 短注释' > "$probe_dir/__lint_probe.rs"
if scripts/checks/check-comment-style.sh >/dev/null 2>&1; then
  echo "FAIL: 模块注释长度白名单的陈旧条目未被发现"
  fail=1
else
  echo "PASS: 模块注释长度白名单的陈旧条目被拦下"
fi
mv scripts/comment-length-allow.txt.selftest-backup scripts/comment-length-allow.txt
rm -f "$probe_dir/__lint_probe.rs"

expect_pass scripts/checks/check-comment-style.sh

# 日志卫生：密钥、路径与绕过出口都必须被拦
expect_fail scripts/checks/check-log-redaction.sh "$probe_dir/__lint_probe.rs" \
  'pub fn leak() { info!(component = "x", api_key = "sk-live-abc", "带密钥"); }'
expect_pass scripts/checks/check-log-redaction.sh

expect_fail scripts/checks/check-log-redaction.sh "$probe_dir/__lint_probe.rs" \
  'pub fn leak(p: &std::path::Path) { info!(component = "x", "打开 {}", p.display()); }'
expect_pass scripts/checks/check-log-redaction.sh

expect_fail scripts/checks/check-log-redaction.sh "$probe_dir/__lint_probe.rs" \
  'pub fn bypass() { tracing::info!("绕过统一出口"); }'
expect_pass scripts/checks/check-log-redaction.sh

# 登记了理由的 crate 一旦自己有了日志点，登记必须销账
cp scripts/log-coverage.txt scripts/log-coverage.txt.selftest-backup
printf '%s\n' 'crates/mc-server    # 陈旧条目：它已经有日志点了' >> scripts/log-coverage.txt
if scripts/checks/check-log-redaction.sh >/dev/null 2>&1; then
  echo "FAIL: scripts/checks/check-log-redaction.sh 没有拦下陈旧的覆盖登记"
  fail=1
else
  echo "PASS: 陈旧的日志覆盖登记被拦下"
fi
mv scripts/log-coverage.txt.selftest-backup scripts/log-coverage.txt
expect_pass scripts/checks/check-log-redaction.sh

# 测试夹具：植入固定时间基准必须被拦（生产代码里不许有夹具值）
expect_fail scripts/checks/check-no-test-fixtures.sh "$probe_dir/__lint_probe.rs" \
  'pub fn leaky() -> i64 { 1_790_758_800_000 }'
expect_pass scripts/checks/check-no-test-fixtures.sh

# 本机留档目录：强制纳入索引的探针必须被拦；干净索引必须通过
iteration_probe=docs/internal/__lint_probe_tdd-log.md
mkdir -p docs/internal
printf '%s\n' 'selftest probe — must not stay tracked' > "$iteration_probe"
git add -f "$iteration_probe"
if scripts/checks/check-no-iteration-artifacts.sh >/dev/null 2>&1; then
  echo "FAIL: check-no-iteration-artifacts.sh 未能拦截已跟踪的本机留档探针"
  fail=1
else
  echo "PASS: check-no-iteration-artifacts.sh 正确拦截了已跟踪的本机留档探针"
fi
git rm -f --cached "$iteration_probe" >/dev/null 2>&1 || true
rm -f "$iteration_probe"
rmdir docs/internal 2>/dev/null || true
if scripts/checks/check-no-iteration-artifacts.sh >/dev/null 2>&1; then
  echo "PASS: check-no-iteration-artifacts.sh 在干净索引上通过"
else
  echo "FAIL: check-no-iteration-artifacts.sh 在干净索引上失败"
  fail=1
fi

# 产物校验：要求一个不可能达到的最低版本时必须失败（证明它真的读产物）
if [ "${MC_RUN_SMOKE:-0}" -ne 1 ]; then
  echo "SKIP: 产物校验探针（提交/打版本前显式运行冒烟）"
elif scripts/checks/check-macos-artifacts.sh --min 99.0 >/dev/null 2>&1; then
  echo "FAIL: scripts/checks/check-macos-artifacts.sh 未能拦截不合理的最低版本"
  fail=1
else
  echo "PASS: scripts/checks/check-macos-artifacts.sh 正确拦截了不合理的最低版本"
fi

if [ "$fail" -ne 0 ]; then
  echo
  echo "lint 自检失败"
  exit 1
fi
echo
echo "全部 lint 脚本自检通过"
