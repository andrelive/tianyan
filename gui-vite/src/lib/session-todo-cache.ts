/**
 * 会话待办面板的**快照缓存**（stale-while-revalidate 的载体）。
 *
 * 为什么需要：todo/goal 是「按会话绑定」的小数据，切换会话时必须**同帧**换掉
 * 上一个会话的内容（否则旧待办滞留），又不能让「切回看过的会话」再等一次往返。
 * 缓存让切换/切回都变成同步读，随后再发请求用最新数据覆盖（命中即渲染 +
 * 后台 revalidate），把延迟从「一个轮询间隔（3s）」压到 0 / 一次往返（~10ms）。
 *
 * 独立成模块而非放在组件文件里：① 组件文件只导出组件（react-refresh 规则）；
 * ② 缓存的生命周期与 React 无关（跨挂载保留），本就不属于组件状态。
 */

import type { GoalWithProgress, TodoItem } from '@/lib/types';

/** 一个会话的待办面板快照（已按面板口径过滤：目标只留活跃项）。 */
export interface TodoSnapshot {
  todos: TodoItem[];
  goals: GoalWithProgress[];
}

/** 稳定空快照（引用稳定——避免每次渲染新建对象）。 */
export const EMPTY_TODO_SNAPSHOT: TodoSnapshot = { todos: [], goals: [] };

/**
 * 缓存上限：防长会话列表下无界增长（每条很小，但会话可能是几百个）。
 * 超限淘汰**最早写入**的会话（见 writeTodoSnapshot 的 Map 保序用法）。
 */
const MAX_CACHED_SESSIONS = 50;

const snapshotCache = new Map<string, TodoSnapshot>();

/** 读取会话快照（未命中返回稳定的空快照）。 */
export function readTodoSnapshot(sessionId: string | null): TodoSnapshot {
  if (!sessionId) return EMPTY_TODO_SNAPSHOT;
  return snapshotCache.get(sessionId) ?? EMPTY_TODO_SNAPSHOT;
}

/** 写入会话快照（超上限淘汰最早写入的会话）。 */
export function writeTodoSnapshot(sessionId: string, snapshot: TodoSnapshot): void {
  // Map 保序：先删后插把该会话移到「最新」，最早写入的自然沉到队首被淘汰
  snapshotCache.delete(sessionId);
  snapshotCache.set(sessionId, snapshot);
  if (snapshotCache.size > MAX_CACHED_SESSIONS) {
    const oldest = snapshotCache.keys().next().value;
    if (oldest !== undefined) snapshotCache.delete(oldest);
  }
}

/** 清空缓存（测试隔离用：模块级状态跨用例共享）。 */
export function clearTodoSnapshotCache(): void {
  snapshotCache.clear();
}
