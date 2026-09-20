#!/usr/bin/env bash
# WSL Linux 复刻：CI Quality 的 Rust 步骤（fmt / clippy / lib 测试 / 工具目录）。
#
# 为什么需要它：本地 Windows 只跑 `-p tianyan-core` 会漏掉 **workspace 级** 问题。
# 2026-09-20 教训：`[executor]` 新增 config 字段后，server 集成测试工厂的
# `TianyanConfig` 字面量缺字段 → Linux CI 的
# `cargo clippy --workspace --all-targets -- -D warnings` 编译失败（Step 8），
# 而本地 core-only 检查与 `-p tianyan-core` 测试全绿、毫无察觉。
# 本脚本按 CI quality.yml 的原始命令逐条复刻（含分批 lib 测试：
# 本机 WSL 一次性跑全量会让实例崩溃，见 wsl-core-test.sh 的说明）。
#
# 用法：wsl -d Ubuntu -e bash /mnt/f/work/git/tianyan/scripts/wsl-ci-parity.sh
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/root/tianyan-linux-target

rsync -a --exclude target --exclude node_modules /mnt/f/work/git/tianyan/ /root/tianyan-src/ && echo RSYNC_OK
cd /root/tianyan-src || exit 9

FAILED=0
step() {
  local name="$1"
  shift
  echo "=== $name ==="
  "$@"
  local rc=$?
  echo "[$name] exit=$rc"
  if [ "$rc" -ne 0 ]; then FAILED=1; fi
  return 0
}

step "fmt" cargo fmt --all -- --check
step "clippy" cargo clippy --workspace --exclude tianyan-tauri --all-targets -- -D warnings
step "core-lib-executor" cargo test -p tianyan-core --lib -- executor::
step "core-lib-agent" cargo test -p tianyan-core --lib -- agent::
step "core-lib-storage" cargo test -p tianyan-core --lib -- vfs:: session:: db::
step "core-lib-context" cargo test -p tianyan-core --lib -- context:: memory:: skills:: knowledge::
step "core-lib-infra" cargo test -p tianyan-core --lib -- config:: observability:: scheduler:: snapshot:: events:: goals:: todos:: lsp:: common:: roles:: notification
step "core-lib-model" cargo test -p tianyan-core --lib -- model:: role_store:: test_utils:: tests::test_version tests::test_name
step "server-mcp-lib" cargo test -p tianyan-server -p tianyan-mcp --lib
step "catalog-check" cargo run -q -p tianyan-core --example tool_catalog -- --check

if [ "$FAILED" -eq 0 ]; then
  echo "CI_PARITY_OK"
else
  echo "CI_PARITY_FAILED"
fi
exit "$FAILED"
