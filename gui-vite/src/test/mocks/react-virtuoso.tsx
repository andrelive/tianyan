/**
 * vitest 用 react-virtuoso mock（经 vitest.config.ts alias 注入）。
 *
 * jsdom 无布局引擎（所有元素尺寸为 0），虚拟化无法真实工作——mock 渲染
 * `data` 的全部项（调用 itemContent），保证既有测试的消息断言可用；
 * ref 暴露 no-op 的滚动方法。`startReached` 不自动触发（上滚加载逻辑
 * 由 use-session-history 单测覆盖 loadOlder 本身；真实虚拟化行为由
 * e2e（Playwright，真实浏览器）与手工验证覆盖）。
 *
 * 注意：本文件只影响 vitest 运行时（alias）；`tsc --noEmit` 解析的是
 * node_modules 中的真实类型——业务代码的 API 用法始终按真实库校验。
 */
import { forwardRef, Fragment, useEffect, useImperativeHandle, useRef } from 'react';
import type { ReactNode } from 'react';

export interface MockVirtuosoProps {
  data?: unknown[];
  /** 真实库语义：itemContent 的 index = 数组下标 + firstItemIndex（全局索引），
   * 与 react-virtuoso 源码一致（z(originalIndex + firstItemIndex, ...)）——
   * 业务代码自行换算数组下标（ChatPanel itemContent）。 */
  firstItemIndex?: number;
  itemContent?: (index: number, item: unknown) => ReactNode;
  className?: string;
  /** 真实库语义：挂载后回传滚动容器元素（供"滚到顶才加载"的原生 scroll 判定）。 */
  scrollerRef?: (ref: HTMLElement | Window | null) => void;
}

/** no-op 滚动句柄（业务代码仅做 null 检查 + 方法调用）。 */
export interface VirtuosoHandle {
  scrollToIndex: (
    location: number | { index: number | 'LAST'; align?: string; behavior?: string },
  ) => void;
  scrollTo: (location: { top?: number; left?: number }) => void;
  scrollBy: (location: { top?: number; left?: number }) => void;
}

export const Virtuoso = forwardRef<VirtuosoHandle, MockVirtuosoProps>(function MockVirtuoso(
  { data, firstItemIndex = 0, itemContent, className, scrollerRef },
  ref,
) {
  useImperativeHandle(
    ref,
    () => ({
      scrollToIndex: () => {},
      scrollTo: () => {},
      scrollBy: () => {},
    }),
    [],
  );
  const scrollerElRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (scrollerRef) scrollerRef(scrollerElRef.current);
    return () => {
      if (scrollerRef) scrollerRef(null);
    };
  }, [scrollerRef]);
  const items = data ?? [];
  return (
    <div ref={scrollerElRef} className={className} data-testid="virtuoso">
      {items.map((item, i) => (
        <Fragment key={i}>{itemContent ? itemContent(i + firstItemIndex, item) : null}</Fragment>
      ))}
    </div>
  );
});
