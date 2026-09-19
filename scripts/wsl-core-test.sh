#!/usr/bin/env bash
# WSL Linux 复刻：跑 tianyan-core 全量测试（本地 Windows 之外的 Linux 行为验证）。
#
# 历史教训（两层，均已固化）：
# - v2（2026-09-19）：应用退出 → 关停清理子进程杀掉 wsl.exe → WSL 会话终止、
#   /tmp 被清 → 源码/编译缓存放 /root（持久）、结果日志写 /mnt/f（Windows 侧可见）、
#   由 wsl-launch.sh 经 setsid 后台启动。
# - v3（2026-09-19）：本机 WSL 实例**一次性跑全量 cargo test 会崩**
#   （Wsl/Service/E_UNEXPECTED；子集分批则稳定通过，且测试总数/结果与
#   Windows 端一致）→ 默认按模块前缀**分批**执行，每批独立 cargo 进程；
#   传参时只跑指定批，例：`wsl -d Ubuntu -e bash wsl-core-test.sh executor::`

export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/root/tianyan-linux-target
LOG=/mnt/f/work/git/tianyan/.wsl-core-test.log
echo "=== WSL TEST RUN START $(date -u +%FT%TZ) ===" > "$LOG"

# 源码同步到 WSL 原生文件系统（/mnt/f 的 9p IO 编译慢数倍）
rsync -a --exclude target --exclude node_modules /mnt/f/work/git/tianyan/ /root/tianyan-src/
cd /root/tianyan-src || exit 9

FAILED=0
run_batch() {
  local label="$1"; shift
  local out rc summary
  out=$(cargo test -p tianyan-core --lib -- "$@" 2>&1)
  rc=$?
  summary=$(printf '%s\n' "$out" | grep -E '^test result:' | tail -1)
  {
    printf -- '--- batch %s rc=%s %s ---\n' "$label" "$rc" "$summary"
    printf '%s\n' "$out" | tail -20
  } >> "$LOG"
  echo "[$label] rc=$rc ${summary:-<no test result>}"
  if [ "$rc" -ne 0 ]; then FAILED=1; fi
}

if [ "$#" -gt 0 ]; then
  run_batch "custom" "$@"
else
  # 分批覆盖全部 1344 个 lib 测试（Windows 端同数；分批规避本地 WSL 崩溃）
  run_batch "executor" executor::
  run_batch "agent" agent::
  run_batch "storage" vfs:: session:: db::
  run_batch "context" context:: memory:: skills:: knowledge::
  run_batch "infra" config:: observability:: scheduler:: snapshot:: events:: goals:: todos:: lsp:: common:: roles:: notification
  run_batch "model" model:: role_store:: test_utils:: tests::test_version tests::test_name
fi

if [ "$FAILED" -eq 0 ]; then
  echo "ALL_BATCHES_OK" >> "$LOG"
else
  echo "SOME_BATCHES_FAILED" >> "$LOG"
fi
tail -5 "$LOG"
echo "WSL_RUN_DONE"
exit "$FAILED"
