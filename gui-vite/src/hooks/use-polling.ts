/**
 * usePolling —— 轮询 hook（interval + cleanup + 竞态收敛的唯一实现）。
 *
 * 三个轮询组件（任务/审批/剪贴板）此前各自手写「setInterval + ref 持有 +
 * cleanup + 立即执行一次」，间隔与错误策略互不一致。本 hook 收敛：
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
  const { immediate = true, onError, enabled = true } = options;
  const [loading, setLoading] = useState(false);

  const fnRef = useRef(fn);
  fnRef.current = fn;
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;

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
  }, [enabled, immediate, intervalMs, run]);

  const refresh = useCallback((forceLoading = false) => run(forceLoading), [run]);
  return { loading, refresh };
}
