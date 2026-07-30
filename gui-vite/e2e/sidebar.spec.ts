import { test, expect } from '@playwright/test';

test.describe('sidebar', () => {
  test.beforeEach(async ({ page }) => {
    // Mock config status to skip ConfigWizard
    await page.route('**/api/v1/config/status', async (route) => {
      await route.fulfill({ json: { configured: true } });
    });

    // Mock sessions API
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

    // Navigate to app (gets redirected to /chat)
    await page.goto('/');
  });

  test('sidebar opens and closes by clicking toggle button', async ({ page }) => {
    // Sidebar starts expanded by default
    await expect(page.locator('button[title="收起侧边栏"]')).toBeVisible();

    // Click toggle to collapse
    await page.locator('button[title="收起侧边栏"]').click();
    await expect(page.locator('button[title="展开侧边栏"]')).toBeVisible();

    // Click toggle to expand again
    await page.locator('button[title="展开侧边栏"]').click();
    await expect(page.locator('button[title="收起侧边栏"]')).toBeVisible();
  });

  test('collapsed sidebar shows only icons', async ({ page }) => {
    // Collapse sidebar
    await page.locator('button[title="收起侧边栏"]').click();

    // Nav icon buttons should be visible
    await expect(page.locator('button[title="对话"]')).toBeVisible();
    await expect(page.locator('button[title="技能"]')).toBeVisible();
    await expect(page.locator('button[title="知识"]')).toBeVisible();
    await expect(page.locator('button[title="设置"]')).toBeVisible();

    // "天演" brand text should NOT be visible when collapsed
    const sidebar = page.locator('aside').first();
    await expect(sidebar).not.toContainText('天演');
  });

  test('expanded sidebar shows session list', async ({ page }) => {
    // Wait for sessions to load and render
    await expect(page.getByText('测试会话')).toBeVisible({ timeout: 5000 });

    // Brand text should be visible when expanded
    await expect(page.locator('aside').first()).toContainText('天演');
  });

  test('clicking 新建对话 navigates to /chat', async ({ page }) => {
    await page.locator('button[title="新建对话"]').click();
    await expect(page).toHaveURL('/chat');
  });

  test('nav items navigation', async ({ page }) => {
    // In expanded sidebar, nav buttons have `<span>` text labels.
    // Use exact: true because "对话" is a substring of "新建对话" button.
    await page.getByRole('button', { name: '技能', exact: true }).click();
    await expect(page).toHaveURL('/skills');

    await page.getByRole('button', { name: '对话', exact: true }).click();
    await expect(page).toHaveURL('/chat');

    await page.getByRole('button', { name: '知识', exact: true }).click();
    await expect(page).toHaveURL('/knowledge');

    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page).toHaveURL('/settings');
  });

  test('session selection highlights correct session', async ({ page }) => {
    // Click on the session
    await page.getByText('测试会话').click();

    // Should navigate to /chat/s1
    await expect(page).toHaveURL('/chat/s1');
  });

  test('delete session button appears on hover', async ({ page }) => {
    // Hover over session item
    await page.getByText('测试会话').hover();

    // Delete button should become visible
    await expect(page.locator('button[title="删除会话"]')).toBeVisible();
  });
});

test.describe('sidebar empty state', () => {
  test.beforeEach(async ({ page }) => {
    await page.route('**/api/v1/config/status', async (route) => {
      await route.fulfill({ json: { configured: true } });
    });

    // Mock empty sessions
    await page.route('**/api/v1/sessions', async (route) => {
      await route.fulfill({ json: { sessions: [], total: 0 } });
    });

    await page.goto('/');
  });

  test('empty state shows 暂无会话', async ({ page }) => {
    await expect(page.getByText('暂无会话')).toBeVisible({ timeout: 5000 });
  });
});
