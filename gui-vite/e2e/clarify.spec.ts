import { test, expect, type APIRequestContext } from '@playwright/test';
import { assertE2eBackend } from './helpers';

/**
 * 真实追问链路：UI 发送消息 → mock-llm 返回 ask_user 工具调用 → AgentLoop
 * 主循环拦截转 NeedsClarification → 前端展示追问气泡 → 用户回答 →
 * /chat/clarify/stream（流式追问）→ mock-llm 回复 → 前端渲染。
 * 非流式 /chat/clarify 端点已移除，本 spec 是追问链路的唯一 E2E 覆盖。
 */

/** mock-llm.mjs 的场景常量（与 scripts/e2e/mock-llm.mjs 保持一致；
 * 不跨模块 import，避免 TS 对 .mjs 的类型解析问题）。 */
const ASK_TRIGGER = '触发追问';
const ASK_QUESTION = 'E2E 追问测试：你更喜欢哪个颜色？';
const MOCK_REPLY = '你好，我是天演 E2E 模拟助手，这条回复来自 mock-llm。';

test.describe('real backend clarify (ask_user)', () => {
  // 身份守卫：防止误连开发后端（本 spec 的清理会删除会话数据）
  test.beforeAll(async ({ request }) => {
    await assertE2eBackend(request);
  });

  // 消息内容必须包含触发词（mock 场景开关），会话标题由第一条消息生成
  const uniqueMessage = ASK_TRIGGER + Date.now();

  test.afterEach(async ({ request }) => {
    await deleteMarkerSessions(request);
  });

  test('ask_user → clarification bubble → answer → streamed reply', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('complementary', { name: '会话列表' }).getByLabel('新建会话').click();
    await expect(page).toHaveURL('/chat');

    const textarea = page.getByRole('textbox', { name: '输入消息' });
    await expect(textarea).toBeVisible({ timeout: 30000 });
    await textarea.fill(uniqueMessage);
    await textarea.press('Enter');

    // 用户消息立即渲染
    await expect(page.getByText(uniqueMessage)).toBeVisible();

    // 追问接管出现：标签 + 问题文本 + 选项行（AgentLoop 拦截 ask_user →
    // NeedsClarification → 流式 clarification chunk（含 questions + 选项描述）
    // → 前端 composer takeover：tab 分步 + 可点选项行 + 自定义输入）
    await expect(page.getByText('AI 需要确认')).toBeVisible({ timeout: 60000 });
    await expect(page.getByText(ASK_QUESTION)).toBeVisible();
    await expect(page.getByRole('radio', { name: '蓝色' })).toBeVisible();
    await expect(page.getByText('冷静的色调')).toBeVisible();
    await expect(page.getByRole('radio', { name: '绿色' })).toBeVisible();
    await expect(page.getByText('自然的色调')).toBeVisible();
    // 分步 tab：问题 1 + 补充信息
    await expect(page.getByRole('tab', { name: '问题 1' })).toBeVisible();
    await expect(page.getByRole('tab', { name: '补充信息' })).toBeVisible();

    // 点选选项回答（流式追问 /chat/clarify/stream）
    await page.getByRole('radio', { name: '蓝色' }).click();
    await page.getByRole('button', { name: '提交回答' }).click();

    // 澄清轮 mock 回复流式到达
    await expect(page.getByText(MOCK_REPLY).first()).toBeVisible({ timeout: 60000 });
  });
});

/** 删除标题带本 spec 标记前缀的会话（幂等；不影响其他 spec 的会话）。 */
async function deleteMarkerSessions(request: APIRequestContext): Promise<void> {
  const res = await request.get('/api/v1/sessions');
  if (!res.ok()) return;
  const body = (await res.json()) as { sessions: Array<{ id: string; title: string }> };
  for (const session of body.sessions) {
    if (!session.title.startsWith(ASK_TRIGGER)) continue;
    const del = await request.delete('/api/v1/sessions/' + session.id);
    if (!del.ok() && del.status() !== 404) {
      throw new Error('删除会话失败 ' + session.id + ': HTTP ' + del.status());
    }
  }
}