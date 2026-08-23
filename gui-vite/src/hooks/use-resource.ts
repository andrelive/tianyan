/**
 * useResource —— 数据获取抽象（fetch-on-mount 样板的唯一定义点）。
 *
 * 收敛「useState×3 + useEffect + try/catch/finally」四件套：组件只需要
 * 声明 fetcher，获得 data/loading/error/reload。依赖数组变化或调用
 * reload() 时重新执行；StrictMode 双跑与竞态（旧请求晚到）安全。
 */

import { useCallback, useEffect, useRef, useState, type DependencyList } from 'react';

export interface UseResourceOptions {
  /** 错误兜底文案（error 非 Error 实例时）。 */
  errorFallback?: string;
}

export interface UseResourceResult<T> {
  /** 最近一次成功结果（未成功过为 null）。 */
  data: T | null;
  /** 请求进行中（首次与 reload 均置 true）。 */
  loading: boolean;
  /** 最近一次失败的人类可读消息（成功时为 null）。 */
  error: string | null;
  /** 重新执行 fetcher（置 loading；刷新按钮/重试用）。 */
  reload: () => void;
}

export function useResource<T>(
  fetcher: () => Promise<T>,
  deps: DependencyList = [],
  options: UseResourceOptions = {},
): UseResourceResult<T> {
  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);

  const fetcherRef = useRef(fetcher);
  fetcherRef.current = fetcher;
  const fallbackRef = useRef(options.errorFallback ?? '加载失败');
  fallbackRef.current = options.errorFallback ?? '加载失败';

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    fetcherRef
      .current()
      .then(
        (result) => {
          if (!cancelled) setData(result);
        },
        (err: unknown) => {
          if (!cancelled) {
            setError(err instanceof Error ? err.message : fallbackRef.current);
          }
        },
      )
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // deps 由调用方声明（fetcher 本身经 ref 读取，不参与依赖）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, nonce]);

  const reload = useCallback(() => setNonce((n) => n + 1), []);
  return { data, loading, error, reload };
}
