import { test, expect, type Page } from '@playwright/test';

/**
 * Markdown 列表渲染**布局**回归（0.5.7）。
 *
 * 背景：hast 会给含块级子元素的 `li` 在首尾插入 `\n` 文本节点（嵌套列表的
 * 「文本 → 子列表」之间、松散列表的 `<p>` 前后）。若把 `whitespace-pre-wrap`
 * 施加到**所有** li，这些 `\n` 会渲染成整行空行——嵌套列表块高翻倍、松散有序
 * 列表的 marker（`1.`/`2.`）与正文被挤成两行。修复 = pre-wrap 只落在
 * **不含块级子元素**的 li（`li:not(:has(p,ul,ol,pre,blockquote,table,hr))`）。
 *
 * 为什么用 e2e：jsdom 不做布局（vitest 只能锁类名），而本 bug 的本质是
 * **布局**——这里用真实 Chromium 量 ul/li 的间距，判别力直接可测。
 *
 * 为什么走**历史消息**而不是发消息：ADR-028 起 `POST /chat/stream` 已收敛为
 * 「开关」（返回 JSON），流式输出经 `GET /events`（EventSource 长连接）下发，
 * 而 `page.route` 无法模拟长驻流。历史消息走 HTTP JSON（`GET /sessions/:id/messages`），
 * 渲染路径与实时流**同一组件**（`MarkdownContent`），断言同样有效。
 *
 * 判别力：把 `MessageSegments.tsx` 的选择器改回 `[&_li]:whitespace-pre-wrap`
 * 即红（嵌套 ≈29px、松散 ≈47px 的空行会立刻出现）。
 */

const NESTED_LIST_MD = [
  '- 第一项正文（无子列表）',
  '- 第二项（含子列表）：',
  '  - 子项 A',
  '  - 子项 B',
].join('\n');

const LOOSE_ORDERED_MD = ['1. 第一项', '', '2. 第二项', '', '3. 第三项'].join('\n');

/** 启动应用，并把给定 markdown 作为**历史 assistant 消息**载入会话 s1。 */
async function bootWithHistoryMarkdown(page: Page, markdown: string): Promise<void> {
  await page.route('**/api/v1/config/status', async (route) => {
    await route.fulfill({ json: { configured: true } });
  });

  await page.route('**/api/v1/sessions', async (route) => {
    await route.fulfill({
      json: {
        sessions: [
          {
            id: 's1',
            title: '布局回归',
            created_at: '2026-09-20T00:00:00Z',
            updated_at: '2026-09-20T00:00:00Z',
            message_count: 2,
          },
        ],
        total: 1,
      },
    });
  });

  await page.route('**/api/v1/sessions/s1/messages*', async (route) => {
    await route.fulfill({
      json: {
        session_id: 's1',
        messages: [
          {
            id: 'm1',
            seq: 1,
            role: 'user',
            segments: [{ type: 'text', text: '请列出要点' }],
            timestamp: '2026-09-20T00:00:00Z',
          },
          {
            id: 'm2',
            seq: 2,
            role: 'assistant',
            segments: [{ type: 'text', text: markdown }],
            timestamp: '2026-09-20T00:00:01Z',
          },
        ],
      },
    });
  });

  await page.goto('/');
  // 选中会话 → 加载历史消息
  await page.getByRole('button', { name: /布局回归/ }).first().click();
}

test.describe('markdown 列表布局（零凭空空行）', () => {
  test('嵌套列表：子列表顶部与首个 li 之间无空行', async ({ page }) => {
    await bootWithHistoryMarkdown(page, NESTED_LIST_MD);

    const subUl = page.locator('ul ul').first();
    await expect(subUl).toBeVisible();
    const firstSubLi = subUl.locator('> li').first();
    await expect(firstSubLi).toBeVisible();

    const ulBox = await subUl.boundingBox();
    const liBox = await firstSubLi.boundingBox();
    expect(ulBox).not.toBeNull();
    expect(liBox).not.toBeNull();

    // 修复前：ul 内的前导 `\n` 被 pre-wrap 渲染成整行空行 → 差值 ≈ 29px（一行高）
    const leadingGap = liBox!.y - ulBox!.y;
    expect(
      leadingGap,
      `子列表顶部出现凭空空行（${leadingGap}px）——pre-wrap 是否又落到含块级子元素的 li？`,
    ).toBeLessThan(1);
  });

  test('松散有序列表：marker 与正文同行（li 内无前导空行）', async ({ page }) => {
    await bootWithHistoryMarkdown(page, LOOSE_ORDERED_MD);

    const ol = page.locator('ol').first();
    await expect(ol).toBeVisible();
    const firstLi = ol.locator('> li').first();
    const firstParagraph = ol.locator('> li > p').first();
    await expect(firstParagraph).toBeVisible();

    const liBox = await firstLi.boundingBox();
    const pBox = await firstParagraph.boundingBox();
    expect(liBox).not.toBeNull();
    expect(pBox).not.toBeNull();

    // 修复前：li 的**前导** `\n` 把 marker 与正文分到两行 → 差值 ≈ 47px
    const leadingGap = pBox!.y - liBox!.y;
    expect(
      leadingGap,
      `有序列表项 marker 与正文被分到两行（差 ${leadingGap}px）——pre-wrap 是否又落到含 <p> 的 li？`,
    ).toBeLessThan(1);
  });
});
