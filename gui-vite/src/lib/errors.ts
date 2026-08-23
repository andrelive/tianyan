/**
 * 错误 → 人类可读消息的唯一提取点（E5：收敛全前端 35 处
 * `err instanceof Error ? err.message : '...'` 样板，措辞统一）。
 */

/** 提取错误的人类可读消息；非 Error 实例（字符串/网络异常等）时用兜底文案。 */
export function toErrorMessage(err: unknown, fallback = '操作失败'): string {
  return err instanceof Error ? err.message : fallback;
}
