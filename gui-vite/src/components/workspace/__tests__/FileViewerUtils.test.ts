/**
 * FileViewer 纯函数测试（F5）。
 *
 * computeNextOffset（分页 offset 计算）与 stripHashline（锚点剥离）是
 * 历史 bug 高发区（截断提示行被计入真实行 → 从第 3 页起静默跳行），
 * 提取导出后直接以纯函数锁死行为。
 */

import { describe, it, expect } from 'vitest';
import { computeNextOffset, stripHashline } from '../fileViewerUtils';
import type { WorkspaceReadResponse } from '@/lib/types';

function res(partial: Partial<WorkspaceReadResponse>): WorkspaceReadResponse {
  return { path: 'a.rs', ...partial };
}

describe('computeNextOffset', () => {
  it('uses the showing window when present (offset + min(limit, remaining))', () => {
    const r = res({
      showing: { offset: 1, limit: 2000 },
      total_lines: 5000,
      truncated: true,
      content: 'line1\nline2',
    });
    expect(computeNextOffset(r)).toBe(2001);
  });

  it('caps the window by remaining lines near the end of file', () => {
    const r = res({
      showing: { offset: 4501, limit: 2000 },
      total_lines: 5000,
      truncated: true,
    });
    expect(computeNextOffset(r)).toBe(5001);
  });

  it('returns total+1 when exactly at the last line', () => {
    const r = res({
      showing: { offset: 5000, limit: 2000 },
      total_lines: 5000,
    });
    expect(computeNextOffset(r)).toBe(5001);
  });

  it('falls back to content line count when showing is absent', () => {
    const r = res({
      content: 'a\nb\nc',
      truncated: false,
    });
    expect(computeNextOffset(r)).toBe(3);
  });

  it('excludes the truncation hint line from the fallback count (historical 3rd-page skip bug)', () => {
    const r = res({
      content: 'a\nb\n(Showing lines 1-2 of 5000)',
      truncated: true,
    });
    expect(computeNextOffset(r)).toBe(2);
  });

  it('treats empty content as a single (empty) line', () => {
    // ''\n' 语义：split 产生 ['']，长度为 1；因 loadMore 仅在 truncated 时出现，
    // 空文件不会触发分页循环，此值无害。
    const r = res({ content: '' });
    expect(computeNextOffset(r)).toBe(1);
  });
});

describe('stripHashline', () => {
  it('strips a hashline prefix and keeps the content', () => {
    expect(stripHashline('12#a1|fn main() {')).toBe('fn main() {');
  });

  it('leaves lines without a hashline prefix untouched', () => {
    expect(stripHashline('fn main() {')).toBe('fn main() {');
    expect(stripHashline('')).toBe('');
  });

  it('requires exactly the 2-hex-digit separator (4 hex digits are not stripped)', () => {
    // 契约：短哈希恰好 2 位；更长的 ID 前缀不应被误剥
    expect(stripHashline('12#a1b2|content')).toBe('12#a1b2|content');
  });

  it('strips prefixes at any line start, preserving the remainder verbatim', () => {
    expect(stripHashline('3#ff|  indented')).toBe('  indented');
  });
});
