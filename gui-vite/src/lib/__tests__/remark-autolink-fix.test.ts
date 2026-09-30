import { describe, it, expect } from 'vitest';
import { unified } from 'unified';
import remarkParse from 'remark-parse';
import remarkGfm from 'remark-gfm';
import remarkAutolinkFix from '../remark-autolink-fix';

/**
 * 回归保护：GFM autolink literal 会吞掉紧跟 URL 的 markdown 定界符/正文
 * （`**http://x**（91 视图` → link(url 含 `**（91`)、星号字面泄露、href 被污染）。
 * 本插件在 AST 后处理中截断 URL、还原 `**` 配对、把尾巴还原为文本。
 */

interface AnyNode {
  type: string;
  value?: string;
  url?: string;
  children?: AnyNode[];
}

/** 跑完整管线（parse → gfm → 修复）返回 mdast。 */
function parse(md: string): AnyNode {
  const proc = unified()
    .use(remarkParse)
    .use(remarkGfm, { singleTilde: false })
    .use(remarkAutolinkFix);
  const tree = proc.parse(md) as unknown as AnyNode;
  return proc.runSync(tree as never) as unknown as AnyNode;
}

/** 收集所有 link（url + 父节点类型）。 */
function collectLinks(
  node: AnyNode,
  parentType = '',
  out: Array<{ url: string; parent: string }> = [],
): Array<{ url: string; parent: string }> {
  if (node.type === 'link' && typeof node.url === 'string') {
    out.push({ url: node.url, parent: parentType });
  }
  for (const child of node.children ?? []) collectLinks(child, node.type, out);
  return out;
}

/** 收集所有 text 节点的值。 */
function collectTexts(node: AnyNode, out: string[] = []): string[] {
  if (node.type === 'text' && typeof node.value === 'string') out.push(node.value);
  for (const child of node.children ?? []) collectTexts(child, out);
  return out;
}

describe('remarkAutolinkFix', () => {
  it('截断被吞的 URL 并还原 ** 配对（用户场景）', () => {
    const tree = parse('- **http://localhost:5180**（91 视图');
    const links = collectLinks(tree);
    expect(links).toEqual([{ url: 'http://localhost:5180', parent: 'strong' }]);

    const texts = collectTexts(tree).join('');
    expect(texts).not.toContain('**');
    expect(texts).toContain('（91 视图');
  });

  it('带尾斜杠的形态也还原（此前后端会收到 /*%EF%BC%88 污染路径）', () => {
    const tree = parse('- **http://localhost:5180/**（91 视图');
    expect(collectLinks(tree)).toEqual([{ url: 'http://localhost:5180/', parent: 'strong' }]);
    expect(collectTexts(tree).join('')).not.toContain('**');
  });

  it('句号/全角标点结尾不泄露星号', () => {
    for (const md of ['**http://localhost:5180**。', '**http://localhost:5180**）结尾']) {
      const tree = parse(md);
      expect(collectLinks(tree)[0].url).toBe('http://localhost:5180');
      expect(collectTexts(tree).join('')).not.toContain('**');
    }
  });

  it('裸 URL 后粘中文：只截断 URL，不吞正文', () => {
    const tree = parse('http://localhost:5180（91 视图');
    expect(collectLinks(tree)).toEqual([{ url: 'http://localhost:5180', parent: 'paragraph' }]);
    expect(collectTexts(tree).join('')).toContain('（91 视图');
  });

  // ── 不得破坏的正常形态 ────────────────────────────────────────────

  it('URL 后为行尾/空格：保持 strong > link', () => {
    for (const md of ['**http://localhost:5180**', '**http://localhost:5180** 测试']) {
      const tree = parse(md);
      expect(collectLinks(tree)).toEqual([{ url: 'http://localhost:5180', parent: 'strong' }]);
    }
  });

  it('中文粗体不受影响', () => {
    const tree = parse('- **后端未启动**（8080 / 面板 8000）');
    const texts = collectTexts(tree).join('');
    expect(texts).toContain('后端未启动');
    expect(texts).not.toContain('**');
    expect(collectLinks(tree)).toEqual([]);
  });

  it('显式 markdown 链接原样保留', () => {
    const tree = parse('[文本](http://example.com/a)');
    expect(collectLinks(tree)).toEqual([{ url: 'http://example.com/a', parent: 'paragraph' }]);
  });

  it('带中文 path 的 URL 不动（保守规则只处理 authority 粘连）', () => {
    const tree = parse('http://example.com/中文路径');
    expect(collectLinks(tree)[0].url).toBe('http://example.com/中文路径');
  });

  it('无效协议链接不改动（交给 react-markdown 的 urlTransform 处理）', () => {
    const tree = parse('[x](javascript:alert(1))');
    expect(collectLinks(tree)[0].url).toBe('javascript:alert(1)');
  });
});
