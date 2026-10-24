/**
 * usePolling —— 轮询 hook（interval + cleanup + 竞态收敛的唯一实现）。
 *
 * 五个轮询场景（任务/审批/剪贴板/唤醒轮/会话审批卡）此前各自手写
 * 「setInterval + ref 持有 + cleanup + 立即执行一次」，间隔与错误策略互不
 * 一致。本 hook 收敛（ChatPanel 的两个轮询于 D3 迁入）：
 * - fn 经 ref 读取：调用方无需 useCallback 稳定身份，组件重渲染不重启轮询；
 * - 立即执行一次（immediate=true）+ interval；enabled=false 时整体暂停；
 * - 错误策略由 onError 单一决策点决定（默认静默保留旧数据）；
 * - refresh(forceLoading) 供手动刷新/操作后同步。
 */

import { useCallback, useEffect, useRef, useState } from 'react';

export interface UsePollingOptions {
  /** 初始是否立即执行一次（默认 true；loading 置 true）。 */
  immediate?: boolean;
  /** 轮询错误回调（默认静默——轮询失败保留旧数据）。 */
  onError?: (error: unknown) => void;
  /** 轮询启用开关（false 时暂停；默认 true）。 */
  enabled?: boolean;
  /**
   * 重置键（「拉取目标变了」/「数据变脏了」）：值变化时清掉旧计时器并
   * **立即执行一次**（`immediate` 为真时，缺省即真），然后按原间隔重新计时。
   *
   * 为什么需要：`fn` 经 ref 读取（调用方无需稳定身份，组件重渲染不重启轮询），
   * 代价是 effect **看不见** `fn` 里引用的目标变化——切换会话后仍等下一个
   * tick 才去拉新目标，表现为「上一个会话的数据滞留一个轮询间隔」
   * （SessionTodoPanel 实测 2-3 秒）。
   *
   * 取值须可序列化（string / number / null 等）：内部按 `JSON.stringify`
   * 比较，调用方每次渲染传新数组也不会误重启轮询。
   */
  resetKeys?: readonly unknown[];
}

export interface UsePollingResult {
  /** 首次/强制刷新进行中。 */
  loading: boolean;
  /** 立即执行一次轮询函数（forceLoading=true 时置 loading 并显示加载态）。 */
  refresh: (forceLoading?: boolean) => Promise<void>;
}

export function usePolling(
  fn: () => Promise<unknown> | void,
  intervalMs: number,
  options: UsePollingOptions = {},
): UsePollingResult {
  const { immediate = true, onError, enabled = true, resetKeys } = options;
  const [loading, setLoading] = useState(false);

  const fnRef = useRef(fn);
  fnRef.current = fn;
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;

  // 重置键序列化：数组按内容比较（调用方每次渲染传新数组不应重启轮询）
  const resetKey = resetKeys ? JSON.stringify(resetKeys) : '';

  const run = useCallback(async (forceLoading: boolean) => {
    if (forceLoading) setLoading(true);
    try {
      await fnRef.current();
    } catch (err) {
      onErrorRef.current?.(err);
    } finally {
      if (forceLoading) setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!enabled) return;
    if (immediate) void run(true);
    const timer = setInterval(() => void run(false), intervalMs);
    return () => clearInterval(timer);
    // resetKey 进依赖：重置键变化 → effect 重跑 → 清旧计时器 + 立即执行一次
  }, [enabled, immediate, intervalMs, run, resetKey]);

  const refresh = useCallback((forceLoading = false) => run(forceLoading), [run]);
  return { loading, refresh };
}
