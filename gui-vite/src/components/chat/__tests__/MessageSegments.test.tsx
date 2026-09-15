import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import { MarkdownContent } from '@/components/chat/MessageSegments';

/**
 * 回归保护（U4）：文本结构化内容与用户输入的换行渲染。
 *
 * 背景：正文全部走 MarkdownContent（ReactMarkdown）。Markdown 规范把
 * 段落内软换行（单个 \n）渲染为文本节点里的换行，但浏览器 white-space:
 * normal 下 `\n` 与连续空格均被折叠为单个空格——导致模型用文本画线框图
 * （ASCII art）时换行/对齐丢失，用户输入的多行文本同样"挤成一行"。
 *
 * 修复：对段落与列表项统一 whitespace-pre-wrap（保留换行 + 连续空格，
 * 且仍允许长行自动换行——区别于 pre）。
 *
 * 判别力：此测试对修复前的代码必红——容器无 [&_p]/[&_li] 的
 * whitespace-pre-wrap 变体类。jsdom 不做 CSS 级联，故以类断言作为
 * "渲染层修复存在"的回归锁；数据链路（文本节点保留 \n）单独断言。
 */
describe('MarkdownContent 换行渲染（U4）', () => {
  it('段落（assistant 文本）保留软换行：容器对 p/li 应用 whitespace-pre-wrap', () => {
    const text = '┌──────────┐\n│ 前端 → API │\n└──────────┘';
    const { container } = render(<MarkdownContent text={text} isUser={false} />);
    const root = container.firstChild as HTMLElement;

    // 渲染修复点：换行折叠由 pre-wrap 解决（判别力：修复前该类缺失）
    expect(root.className).toContain('[&_p]:whitespace-pre-wrap');
    expect(root.className).toContain('[&_li]:whitespace-pre-wrap');

    // 数据链路保真：文本节点保留换行（换行丢失纯在渲染层，不在数据层）
    const p = container.querySelector('p');
    expect(p?.textContent).toContain('\n');
    expect(p?.textContent?.split('\n')).toHaveLength(3);
  });

  it('用户输入的多行文本走同一渲染路径，同样保留换行', () => {
    const { container } = render(<MarkdownContent text={'第一行\n第二行'} isUser={true} />);
    const root = container.firstChild as HTMLElement;

    expect(root.className).toContain('[&_p]:whitespace-pre-wrap');
    expect(container.querySelector('p')?.textContent?.split('\n')).toHaveLength(2);
  });

  it('列表项场景覆盖（li 变体）：多行列表不折叠', () => {
    const { container } = render(<MarkdownContent text={'- 第一项\n- 第二项'} isUser={false} />);
    const root = container.firstChild as HTMLElement;

    expect(root.className).toContain('[&_li]:whitespace-pre-wrap');
    expect(container.querySelectorAll('li')).toHaveLength(2);
  });
});
