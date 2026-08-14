import { test, expect } from '@playwright/test';

/**
 * 会话页左栏（二级列表，deepseek harness 式）：mock 数据驱动。
 * 全局导航见 sidebar.spec.ts；真实后端链路见 real-chat / real-workspace-session。
 */
test.describe('session page left column', () => {
  test.beforeEach(async ({ page }) => {
    await page.route('**/api/v1/config/status', async (route) => {
      await route.fulfill({ json: { configured: true } });
    });

    await page.route('**/api/v1/sessions', async (route) => {
      await route.fulfill({
        json: {
          sessions: [
            {
              id: 's1',
              title: '测试会话',
              created_at: '2026-07-20T10:00:00Z',
              updated_at: '2026-07-23T08:00:00Z',
              message_count: 5,
            },
          ],
          total: 1,
        },
      });
    });

    await page.route('**/api/v1/sessions/*', async (route) => {
      await route.fulfill({ json: { success: true } });
    });

    await page.goto('/');
  });

  test('shows session list grouped under 默认', async ({ page }) => {
    await expect(page.getByRole('button', { name: '分组 默认' })).toBeVisible({
      timeout: 5000,
    });
    await expect(page.getByText('测试会话')).toBeVisible();
  });

  test('clicking a session navigates to /chat/s1', async ({ page }) => {
    await page.getByText('测试会话').click();
    await expect(page).toHaveURL('/chat/s1');
  });

  test('delete session button appears on hover', async ({ page }) => {
    await page.getByText('测试会话').hover();
    await expect(page.locator('button[title="删除会话"]')).toBeVisible();
  });

  test('新目录 opens the directory picker dialog', async ({ page }) => {
    await page.getByRole('button', { name: '新目录' }).click();
    await expect(page.getByRole('dialog', { name: '选择目录' })).toBeVisible({ timeout: 5000 });
  });

  test('文件视图 button navigates to /workspace', async ({ page }) => {
    await page.getByRole('button', { name: '文件视图' }).click();
    await expect(page).toHaveURL('/workspace');
  });
});

test.describe('session page empty state', () => {
  test.beforeEach(async ({ page }) => {
    await page.route('**/api/v1/config/status', async (route) => {
      await route.fulfill({ json: { configured: true } });
    });

    await page.route('**/api/v1/sessions', async (route) => {
      await route.fulfill({ json: { sessions: [], total: 0 } });
    });

    await page.goto('/');
  });

  test('empty state shows 暂无会话', async ({ page }) => {
    await expect(page.getByText('暂无会话')).toBeVisible({ timeout: 5000 });
  });
});
