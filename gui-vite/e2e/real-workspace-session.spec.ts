import { test, expect, type APIRequestContext } from '@playwright/test';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { assertE2eBackend } from './helpers';

/**
 * 会话级工作区（工作区 = 会话的父级分组）真实链路：
 * 通过真实后端创建绑定工作区的会话（ChatRequest.working_directory）→
 * 会话头部固化工作区（GET /sessions 可见）→ 侧边栏按工作区分组 →
 * 工作区页解析会话工作目录展示夹具树。不做任何 API mock。
 */

const __dirname = path.dirname(fileURLToPath(import.meta.url));
/** scripts/e2e/fixtures/workspace 的绝对路径（与 e2e 配置 working_directory 同源）。
 * 注意：本文件位于 gui-vite/e2e/，仓库 fixtures 需两级回退。 */
const FIXTURE_WORKSPACE = path.resolve(__dirname, '../../scripts/e2e/fixtures/workspace');

test.describe('real backend session workspace grouping', () => {
  test.beforeAll(async ({ request }) => {
    await assertE2eBackend(request);
  });

  test('binds a workspace to a new session and groups it in the sidebar', async ({
    page,
    request,
  }) => {
    // 1. 首条消息携带 working_directory → 后端为新会话固化工作区归属
    //    非流式 /chat 已移除（GUI 仅用 SSE），改用 /chat/stream 并解析首事件。
    const message = '会话工作区绑定测试';
    const chat = await createSession(request, message, FIXTURE_WORKSPACE);
    expect(chat.session_id).toBeTruthy();

    // 2. GET /sessions 确认工作区归属已固化（侧边栏分组的数据源）
    await expect
      .poll(async () => {
        const res = await request.get('/api/v1/sessions');
        if (!res.ok()) return null;
        const body = (await res.json()) as {
          sessions: Array<{ id: string; working_directory: string | null }>;
        };
        const s = body.sessions.find((x) => x.id === chat.session_id);
        return s ? s.working_directory : null;
      })
      .toBe(FIXTURE_WORKSPACE);

    // 3. UI：会话页左栏按目录分组 —— 分组头显示目录 basename（title 携带完整路径）
    await page.goto('/');
    await expect(
      page.getByRole('button', { name: `分组 ${path.basename(FIXTURE_WORKSPACE)}` }),
    ).toBeVisible({
      timeout: 30000,
    });
    await expect(page.getByText(message, { exact: true })).toBeVisible({ timeout: 30000 });

    // 4. 进入该会话 → 文件视图页解析会话工作目录并展示夹具树
    await page.getByText(message, { exact: true }).click();
    await page.getByRole('button', { name: '文件视图' }).click();
    await expect(page).toHaveURL('/workspace');
    await expect(page.getByRole('heading', { name: '文件', exact: true })).toBeVisible({
      timeout: 30000,
    });
    // 页头显示会话目录标签
    await expect(page.getByLabel(`目录：${FIXTURE_WORKSPACE}`)).toBeVisible({ timeout: 15000 });
    // 文件树从会话绑定的工作区加载夹具
    const helloRow = page.getByRole('treeitem', { name: 'hello.txt' }).first();
    await expect(helloRow).toBeVisible({ timeout: 30000 });

    // 5. 清理：删除该会话
    const del = await request.delete(`/api/v1/sessions/${chat.session_id}`);
    expect(del.ok()).toBeTruthy();
  });

  test('clears a session workspace binding via PUT /sessions/:id/workspace', async ({
    request,
  }) => {
    // 1. 创建绑定工作区的会话
    const chat = await createSession(request, '清除绑定测试', FIXTURE_WORKSPACE);

    // 2. 清除绑定（空串）→ 会话回落全局配置
    const putRes = await request.put(`/api/v1/sessions/${chat.session_id}/workspace`, {
      data: { working_directory: '' },
    });
    expect(putRes.ok(), await putRes.text()).toBeTruthy();
    const updated = (await putRes.json()) as { working_directory?: string | null };
    expect(updated.working_directory ?? null).toBeNull();

    // 3. 绑定不存在的目录 → 400（存在性校验）
    const badRes = await request.put(`/api/v1/sessions/${chat.session_id}/workspace`, {
      data: { working_directory: 'Z:/no-such-dir-e2e' },
    });
    expect(badRes.status()).toBe(400);

    // 4. 会话不存在 → 404
    const missingRes = await request.put('/api/v1/sessions/no-such-session/workspace', {
      data: { working_directory: FIXTURE_WORKSPACE },
    });
    expect(missingRes.status()).toBe(404);

    // 5. 清理
    await request.delete(`/api/v1/sessions/${chat.session_id}`);
  });
});

/**
 * 通过流式端点创建会话并返回 { session_id }。
 *
 * 非流式 POST /chat 已随"GUI 仅用 SSE"决策移除（4a5b3c8d）；流式端点
 * 返回 SSE 流，首事件（chunk_type=message）携带后端生成的 session_id。
 */
async function createSession(
  request: APIRequestContext,
  message: string,
  workingDirectory: string,
): Promise<{ session_id: string }> {
  const res = await request.post('/api/v1/chat/stream', {
    data: {
      message: { role: 'user', content: message },
      temperature: 0.7,
      max_tokens: 100,
      working_directory: workingDirectory,
    },
  });
  expect(res.ok(), await res.text()).toBeTruthy();
  const body = await res.text();
  // SSE：data: {json} 行；[DONE] 后流结束。取首个携带 session_id 的事件。
  const sessionId = extractFirstSessionId(body);
  expect(sessionId, `SSE 响应应包含 session_id:\n${body.slice(0, 500)}`).toBeTruthy();
  return { session_id: sessionId };
}

/** 从 SSE 文本中提取首个非空 session_id（跳过 [DONE] 与心跳行）。 */
function extractFirstSessionId(sseBody: string): string {
  for (const line of sseBody.split('\n')) {
    if (!line.startsWith('data: ')) continue;
    const payload = line.slice('data: '.length);
    if (payload === '[DONE]') continue;
    try {
      const event = JSON.parse(payload) as { session_id?: string };
      if (event.session_id) return event.session_id;
    } catch {
      // 心跳/非 JSON 行跳过
    }
  }
  return '';
}
