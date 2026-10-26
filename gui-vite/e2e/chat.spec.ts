import { test, expect, type Page } from '@playwright/test';

/**
 * 对话面板基础交互（mock 模式）。
 *
 * ⚠️ 本文件**不再断言「发消息 → 流式回复」**：ADR-028 起 `POST /chat/stream`
 * 已收敛为「开关」——服务端校验 + 启动 AgentLoop 后**立即返回 JSON**
 * （`{ status: 'started', session_id }`），流式输出（增量/边界/工具/usage）
 * 全部经 `GET /events`（EventSource **常驻长连接**）下发，而 `page.route`
 * 无法模拟长驻流（一次性 fulfill 后连接即关闭 → 触发自动重连 + 重复事件）。
 *
 * 历史遗留（本次修复）：本文件曾把 `/chat/stream` mock 成 SSE body，而前端
 * 现在用 `response.json()` 解析该开关响应 → 解析抛错 → 流状态复位、助手
 * 占位被移除，「助手回复」断言从此**恒红**；CI 不跑 e2e，故长期未被察觉。
 *
 * 职责分工（避免重复覆盖）：
 * - 流式端到端（真实后端 + mock-llm + 真实 EventSource）→ `real-chat.spec.ts`；
 * - 助手消息**渲染**（mock 环境）→ 本文件改走**历史消息**路径
 *   （`GET /sessions/:id/messages`，HTTP JSON，与实时流共用同一渲染组件），
 *   与 `markdown-list-layout.spec.ts` 同思路。
 */

const SESSION_TITLE = '测试会话';
const HISTORY_QUESTION = '请介绍一下你自己';
const HISTORY_REPLY = '我是天演，这条历史回复用于断言助手消息渲染。';

/** 公共 mock：跳过配置向导 + 提供一条会话（侧边栏渲染用）。 */
async function mockShell(page: Page): Promise<void> {
  // Mock config status to skip ConfigWizard
  await page.route('**/api/v1/config/status', async (route) => {
    await route.fulfill({ json: { configured: true } });
  });

  // Mock sessions list (needed because sidebar loads sessions on mount)
  await page.route('**/api/v1/sessions', async (route) => {
    await route.fulfill({
      json: {
        sessions: [
          {
            id: 's1',
            title: SESSION_TITLE,
            created_at: '2026-07-20T10:00:00Z',
            updated_at: '2026-07-23T08:00:00Z',
            message_count: 5,
          },
        ],
        total: 1,
      },
    });
  });

  // 启动端点已收敛为「开关」（ADR-028）：真实响应形状是 JSON。返回 started
  // 让启动请求成功；流式事件在本（mock）模式下不会到达——见文件头说明。
  await page.route('**/api/v1/chat/stream', async (route) => {
    await route.fulfill({ json: { status: 'started', session_id: 's1' } });
  });
}

test.describe('chat', () => {
  test.beforeEach(async ({ page }) => {
    await mockShell(page);
    // Navigate to app (redirects to /chat)
    await page.goto('/');
  });

  test('empty state shows placeholder when no messages', async ({ page }) => {
    // Chat panel should show empty state
    await expect(page.getByText('开始一段新的对话')).toBeVisible();
  });

  test('chat input is focused on page load', async ({ page }) => {
    // 输入框已从 `<textarea>` 换成 Lexical 的 contenteditable div：
    // 用 aria-label 定位（与其它 spec 一致），Playwright 的 fill/press 同样支持
    // contenteditable。
    const composer = page.getByRole('textbox', { name: '输入消息' });
    await expect(composer).toBeFocused();
  });

  test('typing message and sending with Enter', async ({ page }) => {
    const composer = page.getByRole('textbox', { name: '输入消息' });
    await expect(composer).toBeVisible();

    // Type a message
    await composer.fill('你好');

    // Press Enter to send
    await composer.press('Enter');

    // 乐观渲染（ADR-031）：用户消息本地立即插入，不依赖任何流式事件。
    // scope 消息区 + exact:true——同一文本会出现在侧栏标题 /「最近会话」条上
    // （strict mode 冲突），且默认子串匹配会命中助手回复。
    await expect(
      page.getByTestId('virtuoso-item-list').getByText('你好', { exact: true }),
    ).toBeVisible();
  });

  test('assistant message renders when a session history is loaded', async ({ page }) => {
    // 历史消息走 HTTP JSON（`GET /sessions/:id/messages`）——渲染路径与实时流
    // 共用同一组件，因此这里覆盖「助手消息渲染」（流式端到端见 real-chat.spec.ts）。
    await page.route('**/api/v1/sessions/s1/messages*', async (route) => {
      await route.fulfill({
        json: {
          session_id: 's1',
          messages: [
            {
              id: 'm1',
              seq: 1,
              role: 'user',
              segments: [{ type: 'text', text: HISTORY_QUESTION }],
              timestamp: '2026-07-23T08:00:00Z',
            },
            {
              id: 'm2',
              seq: 2,
              role: 'assistant',
              segments: [{ type: 'text', text: HISTORY_REPLY }],
              timestamp: '2026-07-23T08:00:01Z',
            },
          ],
        },
      });
    });

    // 选中会话 → 加载历史消息
    await page
      .getByRole('button', { name: new RegExp(SESSION_TITLE) })
      .first()
      .click();

    const userBubble = page.getByText(HISTORY_QUESTION).first();
    const assistantBubble = page.getByText(HISTORY_REPLY).first();
    await expect(userBubble).toBeVisible({ timeout: 10000 });
    await expect(assistantBubble).toBeVisible({ timeout: 10000 });

    // 助手回复渲染在用户提问之下（历史顺序）
    const userBox = await userBubble.boundingBox();
    const assistantBox = await assistantBubble.boundingBox();
    expect(userBox).not.toBeNull();
    expect(assistantBox).not.toBeNull();
    expect(assistantBox!.y).toBeGreaterThan(userBox!.y);
  });

  test('消息列表无横向溢出：Virtuoso scroller 不出现横向滚动条', async ({ page }) => {
    // 判别力来源（历史回归，与消息内容无关）：Virtuoso scroller 上放水平
    // padding 会使其 item 层（width:100% 相对 padding box）恒定向右溢出
    // padding 宽度（16px）→ 消息区底部出现横滚条。此断言在回归引入时必红
    // （delta=16）；修复后 delta=0（见 ChatPanel.tsx 注释）。
    await page.route('**/api/v1/sessions/s1/messages*', async (route) => {
      await route.fulfill({
        json: {
          session_id: 's1',
          messages: [
            {
              id: 'm1',
              seq: 1,
              role: 'user',
              segments: [{ type: 'text', text: '给我一段长代码' }],
            },
            {
              id: 'm2',
              seq: 2,
              role: 'assistant',
              // 超长无换行代码行：由代码块自身 overflow-x 吸收，不应外溢
              segments: [{ type: 'text', text: '```\n' + 'const x = 1; '.repeat(60) + '\n```' }],
            },
          ],
        },
      });
    });

    await page
      .getByRole('button', { name: new RegExp(SESSION_TITLE) })
      .first()
      .click();
    await expect(page.getByText('给我一段长代码').first()).toBeVisible({ timeout: 10000 });

    const delta = await page.evaluate(() => {
      const sc = document.querySelector('div[data-virtuoso-scroller]');
      if (!sc) return null;
      return sc.scrollWidth - sc.clientWidth;
    });
    expect(delta).not.toBeNull();
    expect(delta!).toBeLessThanOrEqual(0);
  });

  test('回退按钮：点击后正常发起删除请求、不报「缺少 ID」', async ({ page }) => {
    // 回归：虚拟列表 itemContent 收到的索引是「全局索引」（数组下标 +
    // firstItemIndex，1e6 基数）。若直用会取到 undefined → 报「该消息缺少
    // ID，无法回退」且不发请求——此测试在未换算时必红（5s 内无 delete 请求）。
    await page.route('**/api/v1/sessions/s1/messages*', async (route) => {
      await route.fulfill({
        json: {
          session_id: 's1',
          messages: [
            {
              id: 'm1',
              seq: 1,
              role: 'user',
              segments: [{ type: 'text', text: '需要回退的问题' }],
            },
            {
              id: 'm2',
              seq: 2,
              role: 'assistant',
              segments: [{ type: 'text', text: '收到，随后处理。' }],
            },
          ],
        },
      });
    });
    // 回退响应（服务端截断结果；回退到首条之前 → 列表空）
    await page.route('**/api/v1/sessions/s1/messages/delete', async (route) => {
      await route.fulfill({ json: { messages: [] } });
    });

    await page
      .getByRole('button', { name: new RegExp(SESSION_TITLE) })
      .first()
      .click();
    await expect(page.getByText('需要回退的问题').first()).toBeVisible({ timeout: 10000 });

    // hover 用户消息 → 回退按钮 → 点击（请求须在点击后 5s 内发出）
    await page.getByText('需要回退的问题').first().hover();
    const deleteRequest = page.waitForRequest(
      (r) => r.url().includes('/messages/delete') && r.method() === 'POST',
      { timeout: 5000 },
    );
    await page.getByRole('button', { name: '回退到此' }).click();
    await deleteRequest;

    // 走通成功路径：撤销回退横幅出现；且不出现「缺少 ID」提示
    await expect(page.getByRole('button', { name: /撤销回退/ })).toBeVisible({ timeout: 5000 });
    await expect(page.getByText('该消息缺少 ID')).toHaveCount(0);
  });

  test('send button is enabled when input has text', async ({ page }) => {
    const composer = page.getByRole('textbox', { name: '输入消息' });
    const sendButton = page.locator('button', { hasText: '发送' });

    // Send button should be disabled initially (no text)
    await expect(sendButton).toBeDisabled();

    // Type some text
    await composer.fill('测试消息');

    // Send button should now be enabled
    await expect(sendButton).toBeEnabled();
  });
});
