#!/usr/bin/env bash
# WSL Linux 复刻 v2：跑 tianyan-core 全量测试（定位 CI Quality 红的失败测试）。
#
# v2 抗中断改造（2026-09-19 教训：应用退出 → 关停清理子进程杀掉 wsl.exe →
# WSL 会话终止 → cargo 编译中断、/tmp 被清）：
# - 源码/编译缓存放 /root（持久；不再用会被清空的 /tmp）
# - 结果日志写 /mnt/f（Windows 侧仓库，持久、可见、可续查）
# - 由 wsl-launch.sh 经 setsid 后台启动（脱离会话；wsl.exe 正常退出不清理）
set -x

export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/root/tianyan-linux-target

LOG=/mnt/f/work/git/tianyan/.wsl-core-test.log
echo "=== WSL TEST RUN START $(date -u +%FT%TZ) ===" > "$LOG"

# 源码同步到 WSL 原生文件系统（/mnt/f 的 9p IO 编译慢数倍）
rsync -a --exclude target --exclude node_modules /mnt/f/work/git/tianyan/ /root/tianyan-src/
cd /root/tianyan-src || exit 9

cargo test -p tianyan-core --lib > "$LOG" 2>&1
echo "CARGO_EXIT=$?" >> "$LOG"
tail -80 "$LOG"
echo "WSL_RUN_DONE" >> "$LOG"
