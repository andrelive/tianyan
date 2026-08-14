import { readdirSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

/**
 * Playwright globalTeardown：清理 E2E 后端数据目录残留。
 *
 * 每次运行在后端 webServer env 中注入 TIANYAN_DATA_DIR =
 * <os.tmpdir()>/tianyan-e2e-<pid>，运行结束后目录残留在临时目录。
 * 此处按前缀 tianyan-e2e-* 清理（只删本套件产物，不动其他内容）。
 * 文件被僵尸进程锁定时 rmSync 失败——忽略（下次运行仍可清理）。
 */
export default function globalTeardown(): void {
  const tmp = os.tmpdir();
  let entries: string[];
  try {
    entries = readdirSync(tmp);
  } catch {
    return;
  }
  for (const name of entries) {
    if (!name.startsWith('tianyan-e2e-')) continue;
    const target = path.join(tmp, name);
    try {
      rmSync(target, { recursive: true, force: true });
    } catch {
      // 文件占用等场景：跳过，留待下次
    }
  }
}
