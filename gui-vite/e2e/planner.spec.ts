import { test, expect } from '@playwright/test';
import { assertE2eBackend } from './helpers';

// planner 测试共享同一后端数据（todo/goal 列表），并行执行会互相干扰
// （如两个测试同时点「标记完成」选中对方的条目）——串行执行保证稳定。
test.describe.configure({ mode: 'serial' });

/**
 * 0.2 新功能真实后端 E2E：
 * - 计划面板（待办清单 + 目标，含关联进度）
 * - 任务面板三 tab（内置 / 定时 / 后台）
 * - 设置页 Web 搜索 tab
 */

test.describe('0.2 planner (todos + goals)', () => {
  test.beforeAll(async ({ request }) => {
    await assertE2eBackend(request);
  });

  test('creates a todo, toggles it complete, and deletes it', async ({ page }) => {
    await page.goto('/planner');
    await expect(page.getByRole('heading', { name: '计划', exact: true })).toBeVisible({
      timeout: 30000,
    });

    // 新建待办
    await page.getByRole('button', { name: '新建待办' }).click();
    await page.getByLabel('待办标题').fill('E2E 测试待办');
    await page.getByRole('button', { name: '添加' }).click();
    await expect(page.getByText('E2E 测试待办')).toBeVisible({ timeout: 10000 });

    // 标记完成（checkbox 按钮 aria-label=标记完成）
    await page.getByRole('button', { name: '标记完成' }).click();
    await expect(page.getByText('1 项未完成 / 共 1 项')).toBeVisible({ timeout: 10000 });

    // 删除
    await page.getByRole('button', { name: '删除待办 E2E 测试待办' }).click();
    await expect(page.getByText('暂无待办')).toBeVisible({ timeout: 10000 });
  });

  test('creates a goal, links a todo, and progress updates', async ({ page }) => {
    await page.goto('/planner');

    // 目标 tab
    await page.getByRole('tab', { name: '目标' }).click();
    await page.getByRole('button', { name: '新建目标' }).click();
    await page.getByLabel('目标名称').fill('E2E 测试目标');
    await page.getByRole('button', { name: '创建' }).click();
    await expect(page.getByText('E2E 测试目标')).toBeVisible({ timeout: 10000 });

    // 待办 tab：新建待办并关联目标
    await page.getByRole('tab', { name: '待办清单' }).click();
    await page.getByRole('button', { name: '新建待办' }).click();
    await page.getByLabel('待办标题').fill('目标关联待办');
    await page.getByLabel('关联目标').selectOption({ label: 'E2E 测试目标' });
    await page.getByRole('button', { name: '添加' }).click();
    await expect(page.getByText('目标关联待办')).toBeVisible({ timeout: 10000 });

    // 目标进度 0%（未完成）
    await page.getByRole('tab', { name: '目标' }).click();
    await expect(page.getByText('0%')).toBeVisible({ timeout: 10000 });
    await expect(page.getByText('待办 0/1')).toBeVisible();

    // 完成待办 → 进度 100%
    await page.getByRole('tab', { name: '待办清单' }).click();
    await page.getByRole('button', { name: '标记完成' }).click();
    await page.getByRole('tab', { name: '目标' }).click();
    await expect(page.getByText('100%')).toBeVisible({ timeout: 10000 });
    await expect(page.getByText('待办 1/1')).toBeVisible();

    // 清理：删除目标与待办
    await page.getByRole('tab', { name: '待办清单' }).click();
    await page.getByRole('button', { name: '删除待办 目标关联待办' }).click();
    await page.getByRole('tab', { name: '目标' }).click();
    await page.getByRole('button', { name: '删除目标 E2E 测试目标' }).click();
    await expect(page.getByText('暂无目标')).toBeVisible({ timeout: 10000 });
  });
});

test.describe('0.2 tasks panel tabs', () => {
  test.beforeAll(async ({ request }) => {
    await assertE2eBackend(request);
  });

  test('shows builtin / scheduled / background tabs', async ({ page }) => {
    await page.goto('/tasks');
    await expect(page.getByRole('heading', { name: '任务', exact: true })).toBeVisible({
      timeout: 30000,
    });

    // 三个 tab 存在
    await expect(page.getByRole('tab', { name: '内置任务' })).toBeVisible();
    await expect(page.getByRole('tab', { name: '定时任务' })).toBeVisible();
    await expect(page.getByRole('tab', { name: '后台任务' })).toBeVisible();

    // 内置任务 tab（默认）：调度器任务列表（e2e 配置无 provider 时为空态或列表）
    await expect(page.getByRole('tab', { name: '内置任务' })).toHaveAttribute(
      'aria-selected',
      'true',
    );

    // 后台任务 tab 可切换
    await page.getByRole('tab', { name: '后台任务' }).click();
    await expect(page.getByRole('tab', { name: '后台任务' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
  });
});

test.describe('0.2 web search settings tab', () => {
  test.beforeAll(async ({ request }) => {
    await assertE2eBackend(request);
  });

  test('renders Web 搜索 tab with backend options', async ({ page }) => {
    await page.goto('/settings');
    await expect(page.getByRole('tablist', { name: '设置选项卡' })).toBeVisible({
      timeout: 30000,
    });
    await page.getByRole('tab', { name: 'Web 搜索' }).click();

    // 后端选择器 + 三个选项（option 在闭合 select 中不可见，断言数量与文本）
    const select = page.getByLabel('搜索后端');
    await expect(select).toBeVisible();
    await expect(select.locator('option')).toHaveCount(3);
    await expect(select.locator('option', { hasText: 'DuckDuckGo' })).toHaveCount(1);
    await expect(select.locator('option', { hasText: 'Bing' })).toHaveCount(1);
    await expect(select.locator('option', { hasText: 'SearXNG' })).toHaveCount(1);

    // 切换 searxng → 显示端点输入
    await select.selectOption('searxng');
    await expect(page.getByLabel('SearXNG 端点')).toBeVisible();
  });
});
