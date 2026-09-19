#!/usr/bin/env bash
# WSL 复刻启动器：setsid 后台启动主脚本，立即返回（wsl.exe 正常退出，
# 不等待任务完成——任务脱离会话，应用退出/会话清理都不影响它）。
setsid bash /mnt/f/work/git/tianyan/scripts/wsl-core-test.sh > /root/wsl-run.log 2>&1 < /dev/null &
sleep 1
echo "LAUNCHED"
