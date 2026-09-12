/**
 * 记忆树构建（纯函数，从 MemoryPanel 下沉——渲染层不再内联分组逻辑）。
 *
 * 后端 /api/v1/memory 返回按 updated_at 倒序的平铺叶子条目（含
 * relative_path）。本函数按路径分段构建树：目录节点按名称字母序，
 * 叶子保持输入顺序（后端时间倒排的自然子序列）——实现「分类树状 +
 * 类目内部按时间倒排」的浏览结构。
 */

import type { MemoryEntry } from '@/lib/types';

/** 目录节点。 */
export interface MemoryTreeDir {
  kind: 'dir';
  /** 目录名（路径最后一段）。 */
  name: string;
  /** 相对路径（memory 根起；折叠状态 key 与节点定位用）。 */
  path: string;
  children: MemoryTreeNode[];
}

/** 叶子节点（记忆条目）。 */
export interface MemoryTreeLeaf {
  kind: 'leaf';
  /** 条目名（路径最后一段）。 */
  name: string;
  /** 相对路径（memory 根起）。 */
  path: string;
  entry: MemoryEntry;
}

/** 树节点（目录或叶子）。 */
export type MemoryTreeNode = MemoryTreeDir | MemoryTreeLeaf;

/** 构建记忆树：目录按名称排序，叶子保持输入顺序（时间倒排）。 */
export function buildMemoryTree(memories: MemoryEntry[]): MemoryTreeNode[] {
  const root: MemoryTreeNode[] = [];

  for (const entry of memories) {
    const relPath = entry.relative_path ?? entry.name ?? '';
    const parts = relPath.split('/').filter(Boolean);
    if (parts.length === 0) continue;

    let level = root;
    let prefix = '';
    // 中间段 → 目录节点（已存在则复用，不重复建）
    for (let i = 0; i < parts.length - 1; i++) {
      prefix = prefix ? `${prefix}/${parts[i]}` : parts[i];
      let dir = level.find((n): n is MemoryTreeDir => n.kind === 'dir' && n.path === prefix);
      if (!dir) {
        dir = { kind: 'dir', name: parts[i], path: prefix, children: [] };
        level.push(dir);
      }
      level = dir.children;
    }

    const leafName = parts[parts.length - 1];
    level.push({
      kind: 'leaf',
      name: leafName,
      path: prefix ? `${prefix}/${leafName}` : leafName,
      entry,
    });
  }

  sortLevels(root);
  return root;
}

/** 目录下的叶子总数（含嵌套；渲染计数用）。 */
export function countLeaves(node: MemoryTreeDir): number {
  let n = 0;
  for (const c of node.children) {
    n += c.kind === 'leaf' ? 1 : countLeaves(c);
  }
  return n;
}

/**
 * 递归排序每一层：目录在前（名称字母序），叶子保持原序。
 *
 * Array.prototype.sort 为稳定排序（ES2019+），叶子相对顺序即输入的
 * 时间倒排序列——「类目内部按时间倒排」由此保证。
 */
function sortLevels(nodes: MemoryTreeNode[]): void {
  nodes.sort((a, b) => {
    if (a.kind !== b.kind) return a.kind === 'dir' ? -1 : 1;
    if (a.kind === 'dir' && b.kind === 'dir') {
      return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
    }
    return 0;
  });
  for (const n of nodes) {
    if (n.kind === 'dir') sortLevels(n.children);
  }
}
