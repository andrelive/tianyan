import { test, expect } from '@playwright/test';

/**
 * 全局一级导航侧边栏（纯导航；会话列表在会话页左栏，见 session-page.spec.ts）。
 * 本 spec 使用 mock：config/status + sessions 均为页面路由拦截。
 */
test.describe('sidebar', () => {
  test.beforeEach(async ({ page }) => {
    // Mock config status to skip ConfigWizard
    await page.route('**/api/v1/config/status', async (route) => {
      await route.fulfill({ json: { configured: true } });
    });

    // Mock sessions API（会话页左栏需要）
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

    // Mock session delete/individual GET
    await page.route('**/api/v1/sessions/*', async (route) => {
      await route.fulfill({ json: { success: true } });
    });

    await page.goto('/');
  });

  test('sidebar opens and closes by clicking toggle button', async ({ page }) => {
    await expect(page.locator('button[title="收起侧边栏"]')).toBeVisible();

    await page.locator('button[title="收起侧边栏"]').click();
    await expect(page.locator('button[title="展开侧边栏"]')).toBeVisible();

    await page.locator('button[title="展开侧边栏"]').click();
    await expect(page.locator('button[title="收起侧边栏"]')).toBeVisible();
  });

  test('collapsed sidebar shows only icons', async ({ page }) => {
    await page.locator('button[title="收起侧边栏"]').click();

    await expect(page.locator('button[title="会话"]')).toBeVisible();
    await expect(page.locator('button[title="技能"]')).toBeVisible();
    await expect(page.locator('button[title="知识"]')).toBeVisible();
    await expect(page.locator('button[title="设置"]')).toBeVisible();

    const sidebar = page.locator('aside').first();
    await expect(sidebar).not.toContainText('天演');
  });

  test('expanded sidebar shows nav labels and brand', async ({ page }) => {
    await expect(page.locator('aside').first()).toContainText('天演');
    await expect(page.locator('aside').first()).toContainText('会话');
    await expect(page.locator('aside').first()).toContainText('技能');
  });

  test('nav items navigation', async ({ page }) => {
    await page.getByRole('button', { name: '技能', exact: true }).click();
    await expect(page).toHaveURL('/skills');

    await page.getByRole('button', { name: '会话', exact: true }).click();
    await expect(page).toHaveURL('/chat');

    await page.getByRole('button', { name: '知识', exact: true }).click();
    await expect(page).toHaveURL('/knowledge');

    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page).toHaveURL('/settings');
  });

  test('does not expose a 工作区 nav item (renamed away)', async ({ page }) => {
    await expect(page.locator('aside').first()).not.toContainText('工作区');
  });
});
