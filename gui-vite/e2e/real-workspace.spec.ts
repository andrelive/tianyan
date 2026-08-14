import { test, expect } from '@playwright/test';
import { assertE2eBackend } from './helpers';

/**
 * 真实工作区链路：文件树从真实后端 /workspace/tree 加载夹具，
 * 点击 hello.txt → /workspace/read 读取 → CodeMirror 渲染内容。
 * 不做任何 API mock。
 */

/** scripts/e2e/fixtures/workspace/hello.txt 首行（后端按工作区夹具读取）。 */
const FIXTURE_FIRST_LINE = '天演 E2E 工作区夹具文件';

test.describe('real backend workspace', () => {
  // 身份守卫：确认工作区夹具来自 e2e 实例
  test.beforeAll(async ({ request }) => {
    await assertE2eBackend(request);
  });

  test('workspace tree loads the fixture and renders its content', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: '文件视图' }).click();
    await expect(page).toHaveURL('/workspace');
    await expect(page.getByRole('heading', { name: '文件', exact: true })).toBeVisible({
      timeout: 30000,
    });

    // 文件树（react-arborist treeitem，双层结构：外层 wrapper + 内层行）
    // 从真实后端加载夹具文件；取 .first() 避免 strict mode 二义性
    const helloRow = page.getByRole('treeitem', { name: 'hello.txt' }).first();
    await expect(helloRow).toBeVisible({ timeout: 30000 });
    await helloRow.click();

    // FileViewer 经 /workspace/read 读取并渲染夹具文本（CodeMirror 内容区）
    await expect(page.locator('.cm-content')).toContainText(FIXTURE_FIRST_LINE, {
      timeout: 15000,
    });
  });
});
