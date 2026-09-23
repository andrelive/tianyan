import { describe, it, expect } from 'vitest';
import { parseDocIndex, renderDocIndex, renderIndexOrText } from '../vfs-index';

describe('vfs-index（L1 目录解析与渲染）', () => {
  const indexJson = JSON.stringify({
    kind: 'index',
    sections: [
      { title: '安装', summary: '如何安装', anchor: '## 安装', start_line: 3, end_line: 10 },
      { title: '配置', summary: '如何配置' },
    ],
  });

  it('parses doc index json', () => {
    const idx = parseDocIndex(indexJson);
    expect(idx?.sections).toHaveLength(2);
    expect(idx?.sections[0].start_line).toBe(3);
  });

  it('returns null for non-index text', () => {
    expect(parseDocIndex('普通概览文本')).toBeNull();
    expect(parseDocIndex('')).toBeNull();
    expect(parseDocIndex(null)).toBeNull();
    expect(parseDocIndex('{"kind":"summary","text":"x"}')).toBeNull();
    expect(parseDocIndex('{坏 JSON')).toBeNull();
  });

  it('renders index as readable list (line range when present)', () => {
    const out = renderIndexOrText(indexJson);
    expect(out).toContain('- 安装（行 3-10）：如何安装');
    expect(out).toContain('- 配置：如何配置');
  });

  it('passes through non-index text unchanged', () => {
    expect(renderIndexOrText('普通文本')).toBe('普通文本');
    expect(renderIndexOrText(null)).toBe('');
  });

  it('renderDocIndex keeps section order', () => {
    const out = renderDocIndex(parseDocIndex(indexJson)!);
    expect(out.split('\n')[0]).toContain('安装');
    expect(out.split('\n')[1]).toContain('配置');
  });
});
