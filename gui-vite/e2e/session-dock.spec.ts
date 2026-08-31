import { test, expect, type APIRequestContext } from '@playwright/test';
import { assertE2eBackend } from './helpers';

// 会话停靠面板测试依赖 UI 创建的会话 + REST 造的会话绑定数据，
// 共享同一后端数据目录——串行执行保证稳定。
test.describe.configure({ mode: 'serial' });

/**
 * 0.2 修复验证（真实后端）：
 * - 任务/计划混合栏目已移除；定时任务成为独立一级栏目
 * - 数据目录只读 + 数据搬迁对话框（浏览器环境回退手动输入路径）
 * - 会话停靠面板（后台任务/待办/目标）会话绑定、完事即隐
 */

/** 消息唯一前缀：Date.now() 保证每次运行/重试的会话内容唯一（标题即首条消息）。 */
const MESSAGE_MARKER = 'E2E停靠面板消息';

test.describe('0.2 nav restructure + storage migration dialog', () => {
  test.beforeAll(async ({ request }) => {
    await assertE2eBackend(request);
  });

  test('定时任务 page renders as a top-level nav item', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: '定时任务', exact: true }).click();
    await expect(page).toHaveURL('/scheduled');
    await expect(page.getByRole('heading', { name: '定时任务', exact: true })).toBeVisible({
      timeout: 30000,
    });
    await expect(page.locator('[role="alert"]')).toHaveCount(0);
  });

  test('removed 任务/计划 mixed nav items no longer exist', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('button[title="任务"]')).toHaveCount(0);
    await expect(page.locator('button[title="计划"]')).toHaveCount(0);
  });

  test('数据目录 is read-only and opens the migration dialog', async ({ page }) => {
    await page.goto('/settings');
    await expect(page.getByRole('tablist', { name: '设置选项卡' })).toBeVisible({
      timeout: 30000,
    });
    await page.getByRole('tab', { name: '数据存储' }).click();

    // 数据目录只读展示（无输入框）+ 数据搬迁按钮
    await expect(page.getByText('数据目录')).toBeVisible();
    const migrateButton = page.getByRole('button', { name: '数据搬迁' });
    await expect(migrateButton).toBeVisible();

    await migrateButton.click();
    const dialog = page.getByRole('dialog', { name: '数据目录搬迁' });
    await expect(dialog).toBeVisible();
    // 浏览器环境（非 Tauri）回退：手动输入路径
    await expect(dialog.getByLabel('新数据目录')).toBeVisible();
    await expect(dialog.getByText(/必须为空或不存在/)).toBeVisible();
    await dialog.getByRole('button', { name: '取消' }).click();
    await expect(dialog).not.toBeVisible();
  });
});

test.describe('0.2 session dock (todos/goals session-bound)', () => {
  test.beforeAll(async ({ request }) => {
    await assertE2eBackend(request);
  });

  let sessionId = '';

  test.afterAll(async ({ request }) => {
    // 清理本 spec 创建的会话（级联清理会话绑定的 todo/goal）
    if (!sessionId) return;
    await request.delete(`/api/v1/sessions/${sessionId}`);
  });

  test('dock shows session todos/goals, completed todo stays visible as done', async ({
    page,
    request,
  }) => {
    await page.goto('/');
    await page.getByRole('complementary', { name: '会话列表' }).getByLabel('新建会话').click();
    await expect(page).toHaveURL('/chat');
    const textarea = page.getByRole('textbox', { name: '输入消息' });
    await expect(textarea).toBeVisible({ timeout: 30000 });
    await textarea.fill(`${MESSAGE_MARKER}${Date.now()}`);
    await textarea.press('Enter');
    await expect(page.getByText(MESSAGE_MARKER).first()).toBeVisible({ timeout: 60000 });

    // 取会话 id（标题 = 首条消息，带唯一标记）
    const res = await request.get('/api/v1/sessions');
    expect(res.ok()).toBeTruthy();
    const body = (await res.json()) as { sessions: Array<{ id: string; title: string }> };
    const mine = body.sessions.find((s) => s.title.startsWith(MESSAGE_MARKER));
    expect(mine).toBeDefined();
    sessionId = mine!.id;

    // REST 造会话绑定数据（todo/goal 工具的同源数据通道）
    const todoRes = await request.post('/api/v1/todos', {
      data: { title: 'E2E 停靠待办', session_id: sessionId },
    });
    expect(todoRes.ok()).toBeTruthy();
    const goalRes = await request.post('/api/v1/goals', {
      data: { title: 'E2E 停靠目标', session_id: sessionId },
    });
    expect(goalRes.ok()).toBeTruthy();
    const goalBody = (await goalRes.json()) as { goal: { id: string } };
    const todoBody = (await todoRes.json()) as { todo: { id: string } };

    // 会话页停靠面板展示活跃待办与目标
    await page.goto(`/chat/${sessionId}`);
    const dock = page.getByText('E2E 停靠待办');
    await expect(dock).toBeVisible({ timeout: 15000 });
    await expect(page.getByText('E2E 停靠目标')).toBeVisible();

    // 完成待办 → 保留展示（划线样式），不消失
    const doneRes = await request.patch(`/api/v1/todos/${todoBody.todo.id}`, {
      data: { status: 'completed' },
    });
    expect(doneRes.ok()).toBeTruthy();
    await expect(page.getByText('E2E 停靠待办')).toBeVisible({ timeout: 15000 });

    // 完成目标 → 目标条也消失
    const goalDone = await request.patch(`/api/v1/goals/${goalBody.goal.id}`, {
      data: { status: 'completed' },
    });
    expect(goalDone.ok()).toBeTruthy();
    await expect(page.getByText('E2E 停靠目标')).not.toBeVisible({ timeout: 15000 });
  });
});
