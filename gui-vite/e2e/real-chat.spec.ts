import { test, expect, type APIRequestContext } from '@playwright/test';
import { assertE2eBackend } from './helpers';

/**
 * 真实对话链路：UI 发送消息（session_id 为 null）→ 后端自动创建会话 →
 * mock-llm 流式回复 → SSE 逐块渲染 → 会话持久化到后端。
 * 不做任何 API mock；request fixture 仅用于会话存在性校验与幂等清理。
 */

/** mock-llm.mjs 的固定回复文本（scripts/e2e/mock-llm.mjs 导出 REPLY）。 */
const MOCK_REPLY = '你好，我是天演 E2E 模拟助手，这条回复来自 mock-llm。';
/** 消息唯一前缀：Date.now() 保证每次运行/重试的会话内容唯一。 */
const MESSAGE_MARKER = 'E2E聊天消息';

test.describe('real backend chat', () => {
  // 身份守卫：防止误连开发后端（本 spec 的清理会删除会话数据）
  test.beforeAll(async ({ request }) => {
    await assertE2eBackend(request);
  });

  const uniqueMessage = `${MESSAGE_MARKER}${Date.now()}`;

  test.afterEach(async ({ request }) => {
    // 唯一数据目录（playwright webServer 按 PID 隔离）：清理全部会话，幂等
    await deleteAllSessions(request);
  });

  test('sends message, streams the mock reply and persists the session', async ({
    page,
    request,
  }) => {
    await page.goto('/');
    await page.locator('button[title="新建对话"]').click();
    await expect(page).toHaveURL('/chat');

    const textarea = page.getByRole('textbox', { name: '输入消息' });
    await expect(textarea).toBeVisible({ timeout: 30000 });
    await textarea.fill(uniqueMessage);
    await textarea.press('Enter');

    // 用户消息立即渲染
    await expect(page.getByText(uniqueMessage)).toBeVisible();

    // 后端 agent loop 首轮往返：mock 回复流式到达（SSE 3 块 + 终止块）
    await expect(page.getByText(MOCK_REPLY)).toBeVisible({ timeout: 60000 });

    // 后端已持久化会话（标题默认"新对话"，以会话数增长为凭据）
    await expect
      .poll(
        async () => {
          const res = await request.get('/api/v1/sessions');
          if (!res.ok()) return 0;
          const body = (await res.json()) as { sessions: unknown[] };
          return body.sessions.length;
        },
        { timeout: 15000 },
      )
      .toBeGreaterThan(0);

    // 重新加载后侧边栏从 /sessions 拉到该会话
    await page.reload();
    const sessionItem = page
      .locator('[role="list"][aria-label="会话列表"] [role="button"]')
      .first();
    await expect(sessionItem).toBeVisible({ timeout: 15000 });
  });
});

/** 删除全部会话（幂等；数据目录每次运行唯一，不会误删其他数据）。 */
async function deleteAllSessions(request: APIRequestContext): Promise<void> {
  const res = await request.get('/api/v1/sessions');
  if (!res.ok()) return;
  const body = (await res.json()) as { sessions: Array<{ id: string }> };
  for (const session of body.sessions) {
    const del = await request.delete(`/api/v1/sessions/${session.id}`);
    if (!del.ok() && del.status() !== 404) {
      throw new Error(`删除会话失败 ${session.id}: HTTP ${del.status()}`);
    }
  }
}
