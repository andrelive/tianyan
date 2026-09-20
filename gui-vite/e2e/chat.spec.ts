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
    const textarea = page.locator('textarea');
    await expect(textarea).toBeFocused();
  });

  test('typing message and sending with Enter', async ({ page }) => {
    const textarea = page.locator('textarea');
    await expect(textarea).toBeVisible();

    // Type a message
    await textarea.fill('你好');

    // Press Enter to send
    await textarea.press('Enter');

    // 乐观渲染（ADR-031）：用户消息本地立即插入，不依赖任何流式事件。
    // exact: true —— 默认子串匹配会同时命中助手回复文本
    await expect(page.getByText('你好', { exact: true })).toBeVisible();
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

  test('send button is enabled when input has text', async ({ page }) => {
    const textarea = page.locator('textarea');
    const sendButton = page.locator('button', { hasText: '发送' });

    // Send button should be disabled initially (no text)
    await expect(sendButton).toBeDisabled();

    // Type some text
    await textarea.fill('测试消息');

    // Send button should now be enabled
    await expect(sendButton).toBeEnabled();
  });
});
