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

    // Mock 目录选择器（目录浏览）：浏览根 → C:\ → sub1/sub2
    await page.route('**/api/v1/workspace/dirs*', async (route) => {
      const url = new URL(route.request().url());
      const path = url.searchParams.get('path');
      if (!path) {
        await route.fulfill({
          json: {
            current: '浏览根',
            parent: null,
            entries: [
              { name: 'C:\\', path: 'C:\\', is_root: true },
              { name: 'D:\\', path: 'D:\\', is_root: true },
            ],
          },
        });
      } else if (path === 'C:\\') {
        await route.fulfill({
          json: {
            current: 'C:\\',
            parent: null,
            entries: [
              { name: 'sub1', path: 'C:\\sub1' },
              { name: 'sub2', path: 'C:\\sub2' },
            ],
          },
        });
      } else {
        await route.fulfill({ json: { current: path, parent: null, entries: [] } });
      }
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

  test('clicking 新建会话 shows the 新会话 placeholder in the 默认 group', async ({ page }) => {
    await page.getByRole('complementary', { name: '会话列表' }).getByLabel('新建会话').click();
    // 占位条目出现在默认分组下（用户可感知新会话归属）
    await expect(page.getByRole('button', { name: '新会话' })).toBeVisible({ timeout: 5000 });
    await expect(page.getByRole('button', { name: '分组 默认' })).toBeVisible();
  });

  test('新目录 picks a directory and shows the new group with a placeholder', async ({ page }) => {
    await page.getByRole('button', { name: '新目录' }).click();
    const dialog = page.getByRole('dialog', { name: '选择目录' });
    await expect(dialog).toBeVisible({ timeout: 5000 });

    // 浏览根 → 双击 C:\ → 选中 sub1 → 确认
    await page.getByRole('button', { name: '目录 C:\\' }).dblclick();
    await expect(page.getByRole('button', { name: '目录 sub1' })).toBeVisible();
    await page.getByRole('button', { name: '目录 sub1' }).click();
    await page.getByRole('button', { name: '确认选择' }).click();

    // 左栏立即出现新分组（sub1）+「新会话」占位；原有「默认」分组仍在
    await expect(page.getByRole('button', { name: '分组 sub1' })).toBeVisible({ timeout: 5000 });
    await expect(page.getByRole('button', { name: '新会话' })).toBeVisible();
    await expect(page.getByRole('button', { name: '分组 默认' })).toBeVisible();
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

  test('empty list still shows the 默认 group (no 暂无会话 empty state)', async ({ page }) => {
    // 语义漂移修复：引入工作区分组后默认组恒存在（sessionGroups 恒含 ''），
    // 「暂无会话」空态已被移除（见 SessionList.tsx 注释）——空列表下左栏仍显示
    // 「默认」分组（可 hover「＋」新建会话）。前端单测 SessionList.test.tsx 已同步为
    // queryByText('暂无会话') 不存在，本 e2e 此前仍断言该文案可见 → 恒红。
    await expect(page.getByRole('button', { name: '分组 默认' })).toBeVisible({ timeout: 5000 });
    await expect(page.getByText('暂无会话')).toHaveCount(0);
  });
});
