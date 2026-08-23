/**
 * useDebouncedValue —— 防抖值 hook（延迟稳定后才生效）。
 *
 * 收敛「useRef 持 setTimeout + cleanup + 竞态取消」样板：组件只需声明
 * value 与延迟，消费端 effect 以防抖后的值做依赖。首次渲染立即返回当前值
 * （不延迟初始值）；快速连续变化时只有最后一次在延迟后生效。
 */

import { useEffect, useState } from 'react';

export function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);

  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);

  return debounced;
}
