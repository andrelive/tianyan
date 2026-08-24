import { test, expect } from '@playwright/test';
import { assertE2eBackend } from './helpers';

/**
 * 真实前后端集成冒烟：后端（cargo run + e2e 配置）通过 vite 代理提供全部数据，
 * 本 spec 不做任何 API mock。
 */

/** 侧边栏展开态导航项（与 Sidebar.tsx 的 NAV_ITEMS 对齐）。 */
const NAV_PAGES = [
  { label: '任务', url: '/tasks', heading: '任务' },
  { label: '审批', url: '/approval', heading: '审批' },
  { label: '洞察', url: '/insights', heading: '洞察' },
  { label: '记忆', url: '/memory', heading: '记忆' },
  { label: '子智能体', url: '/roles', heading: '子智能体' },
  { label: '工具', url: '/tools', heading: '工具' },
] as const;

test.describe('real backend boot', () => {
  // 身份守卫：确认 3000 上是 e2e 实例（工作区夹具含 hello.txt）而非开发后端。
  // 防止误连开发后端——真实 spec 会写入共享数据目录（会话/知识条目）。
  test.beforeAll(async ({ request }) => {
    await assertE2eBackend(request);
  });

  test('boots to main chat UI (real /config/status returns configured)', async ({ page }) => {
    await page.goto('/');
    // 真实配置返回 configured:true → 渲染主界面聊天输入框（而非 ConfigWizard）
    await expect(page.getByRole('textbox', { name: '输入消息' })).toBeVisible({
      timeout: 30000,
    });
    await expect(page).toHaveURL('/chat');
  });

  test('skills page renders built-in skill rows from the real backend', async ({
    page,
    request,
  }) => {
    await page.goto('/');
    await page.getByRole('button', { name: '技能', exact: true }).click();
    await expect(page).toHaveURL('/skills');
    await expect(page.getByRole('heading', { name: '技能', exact: true })).toBeVisible({
      timeout: 30000,
    });

    // 数据源校验：后端真实返回技能列表（非空）
    const res = await request.get('/api/v1/skills');
    expect(res.ok()).toBeTruthy();
    const body = (await res.json()) as {
      skills: Array<{ id: string; name: string; category: string }>;
    };
    expect(body.skills.length).toBeGreaterThan(0);

    // 面板只展示方法论技能（custom 类：GEPA 学习技能 + planning）；
    // 内置桥接技能（file/system/network）不渲染
    const methodology =
      body.skills.find((s) => s.category === 'custom') ?? body.skills[0];
    await expect(page.locator('button', { hasText: methodology.name }).first()).toBeVisible({
      timeout: 15000,
    });
    await expect(page.locator('[role="alert"]')).toHaveCount(0);
  });

  test('navigates to settings and renders the tab list without errors', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page).toHaveURL('/settings');
    // 设置面板无 h1/h2 标题——以 tablist 与首个 tab 断言
    await expect(page.getByRole('tablist', { name: '设置选项卡' })).toBeVisible({
      timeout: 30000,
    });
    await expect(page.getByRole('tab', { name: '模型服务' })).toBeVisible();
    await expect(page.locator('[role="alert"]')).toHaveCount(0);
  });

  for (const { label, url, heading } of NAV_PAGES) {
    test(`navigates to ${label} and renders its heading without errors`, async ({ page }) => {
      await page.goto('/');
      await page.getByRole('button', { name: label, exact: true }).click();
      await expect(page).toHaveURL(url);
      await expect(page.getByRole('heading', { name: heading, exact: true })).toBeVisible({
        timeout: 30000,
      });
      // 面板不应出现任何错误提示（真实后端数据加载成功）
      await expect(page.locator('[role="alert"]')).toHaveCount(0);
    });
  }
});
