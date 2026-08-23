/**
 * FileViewer 纯函数（F5 提取）：分页 offset 计算 + hashline 锚点剥离。
 *
 * 移出组件文件的原因：react-refresh 只允许组件文件导出组件；
 * 且这两个函数是历史 bug 高发区（截断提示行被计入真实行 → 从第 3 页起
 * 静默跳行），独立成纯函数模块后可直接单测锁定行为。
 */

import type { WorkspaceReadResponse } from '@/lib/types';

/**
 * 计算下一次分页请求的 offset（1 起始）：上一窗口起始行 + 窗口内真实内容行数。
 *
 * 优先用响应的 `showing` 窗口（offset + min(limit, total_lines - offset + 1)）。
 * 截断页末尾追加的提示行（"(Showing lines ...)"）不属于内容窗口，不会被计入；
 * 而按显示行数累加会把提示行当成真实行，从第 3 页起每页静默跳过一行。
 * 无 `showing` 时退化为内容行数（截断提示行不计入）。
 */
export function computeNextOffset(res: WorkspaceReadResponse): number {
  const showing = res.showing;
  if (showing && res.total_lines !== undefined) {
    const remaining = Math.max(res.total_lines - (showing.offset - 1), 0);
    const windowSize = Math.min(showing.limit, remaining);
    return showing.offset + windowSize;
  }
  const lines = (res.content ?? '').split('\n');
  const realLines = res.truncated ? Math.max(lines.length - 1, 0) : lines.length;
  return (showing?.offset ?? 0) + realLines;
}

/** hashline 前缀：行号 + 短哈希 + 分隔符（"N#ID|"），供 LLM 锚点使用，人类阅读时剥离。 */
const HASHLINE_RE = /^\d+#[0-9a-f]{2}\|/;

/** 剥离单行 hashline 前缀；不匹配的行原样返回。 */
export function stripHashline(line: string): string {
  return HASHLINE_RE.test(line) ? line.replace(HASHLINE_RE, "") : line;
}
