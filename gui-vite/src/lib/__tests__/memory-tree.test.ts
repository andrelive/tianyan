import { describe, it, expect } from 'vitest';
import { buildMemoryTree, countLeaves } from '../memory-tree';
import type { MemoryEntry } from '@/lib/types';

/** 构造扁平记忆条目（后端平铺契约：relative_path 承载目录信息）。 */
function leaf(relativePath: string): MemoryEntry {
  const uri = `tianyan://memory/${relativePath}`;
  return {
    uri,
    is_directory: false,
    name: relativePath.split('/').pop()!,
    relative_path: relativePath,
    metadata: {
      uri: { uri, namespace: 'memory', path: relativePath.split('/') },
      is_directory: false,
      content_type: 'text/plain',
      category: null,
      source: 'MemoryTask',
      original_name: null,
      file_size: null,
      importance: 0.5,
      tags: [],
      created_at: '2026-09-11T00:00:00Z',
      updated_at: '2026-09-11T00:00:00Z',
      custom: {},
    },
    abstract: null,
    overview: null,
    detail: null,
  };
}

describe('buildMemoryTree', () => {
  it('builds nested directories from relative paths', () => {
    const tree = buildMemoryTree([leaf('facts/a'), leaf('cases/failed_tasks/x')]);

    expect(tree).toHaveLength(2);
    // 顶层目录字母序：cases < facts
    expect(tree[0]).toMatchObject({ kind: 'dir', name: 'cases', path: 'cases' });
    expect(tree[1]).toMatchObject({ kind: 'dir', name: 'facts', path: 'facts' });

    // cases > failed_tasks > x（多级嵌套）
    const cases = tree[0];
    if (cases.kind !== 'dir') throw new Error('expected dir');
    expect(cases.children).toHaveLength(1);
    const failed = cases.children[0];
    if (failed.kind !== 'dir') throw new Error('expected dir');
    expect(failed.name).toBe('failed_tasks');
    expect(failed.children[0]).toMatchObject({
      kind: 'leaf',
      name: 'x',
      path: 'cases/failed_tasks/x',
    });
  });

  it('preserves leaf order within a directory (time-desc input)', () => {
    // 输入即后端时间倒序：同目录叶子必须保持该相对顺序（不按名称重排）
    const tree = buildMemoryTree([leaf('facts/newer'), leaf('facts/older')]);
    const facts = tree[0];
    if (facts.kind !== 'dir') throw new Error('expected dir');
    expect(facts.children.map((c) => c.name)).toEqual(['newer', 'older']);
  });

  it('places directories before leaves, dirs sorted alphabetically', () => {
    const tree = buildMemoryTree([
      leaf('sub/x'), // 先来叶子（时间更新）
      leaf('zeta'),
      leaf('alpha/y'), // 后建目录
    ]);
    expect(tree.map((n) => `${n.kind}:${n.name}`)).toEqual(['dir:alpha', 'dir:sub', 'leaf:zeta']);
  });

  it('reuses existing directory nodes for siblings', () => {
    const tree = buildMemoryTree([leaf('facts/a'), leaf('facts/b'), leaf('facts/c')]);
    expect(tree).toHaveLength(1);
    const facts = tree[0];
    if (facts.kind !== 'dir') throw new Error('expected dir');
    expect(facts.children).toHaveLength(3);
    expect(countLeaves(facts)).toBe(3);
  });

  it('falls back to name when relative_path is null', () => {
    const entry = leaf('facts/a');
    entry.relative_path = null;
    const tree = buildMemoryTree([entry]);
    expect(tree).toHaveLength(1);
    expect(tree[0]).toMatchObject({ kind: 'leaf', name: 'a', path: 'a' });
  });
});
