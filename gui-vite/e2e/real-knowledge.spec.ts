import { test, expect, type APIRequestContext } from '@playwright/test';
import { createHash } from 'node:crypto';
import { assertE2eBackend } from './helpers';

/**
 * 真实知识库链路：UI 导入文件 → 后端 ingest（写入 VFS L2 + 向量索引）→
 * 浏览/查看内容 → 搜索命中。不做任何 API mock；仅用 request fixture
 * 做条目定位与幂等清理（数据本身全部来自真实后端）。
 */

/** 唯一关键词前缀：每条目内容唯一，搜索命中确定。 */
const KEYWORD_PREFIX = '天演E2E知识';

interface KnowledgeEntry {
  name: string;
  is_directory: boolean;
  uri: string;
}

interface ResolvedEntry {
  dirName: string;
  uri: string;
}

test.describe('real backend knowledge', () => {
  // 身份守卫：防止误连开发后端（本 spec 会导入/删除知识条目）
  test.beforeAll(async ({ request }) => {
    await assertE2eBackend(request);
  });

  // Date.now() 保证每次运行/重试的文件内容唯一（内容哈希 = 条目标题）
  const keyword = `${KEYWORD_PREFIX}${Date.now()}`;
  const fileName = `e2e-doc-${Date.now()}.txt`;
  const content = `${keyword}：这是通过真实后端导入的知识条目内容。\n第二行：用于验证浏览视图的完整内容渲染。`;
  // 后端按原始上传字节计算 SHA-256 作为条目名（core knowledge/ingestor calculate_hash）
  const docId = createHash('sha256').update(content, 'utf8').digest('hex');

  test.afterEach(async ({ request }) => {
    await cleanupEntry(request, docId);
  });

  test('ingest → browse → view → search roundtrip through the real backend', async ({
    page,
    request,
  }) => {
    await page.goto('/');
    await page.getByRole('button', { name: '知识', exact: true }).click();
    await expect(page).toHaveURL('/knowledge');
    await expect(page.getByRole('heading', { name: '知识库' })).toBeVisible({
      timeout: 30000,
    });

    // ── 导入：缓冲文件（真实 multipart 上传到后端）──
    await page.getByRole('tab', { name: '导入' }).click();
    await page.locator('input[type="file"]').setInputFiles({
      name: fileName,
      mimeType: 'text/plain',
      buffer: Buffer.from(content, 'utf8'),
    });
    await page.getByRole('button', { name: '开始导入' }).click();
    await expect(page.getByText('文件导入成功！')).toBeVisible({ timeout: 30000 });

    // 通过真实 API 解析条目 uri（文件名含 "doc" → Technical 分类目录）
    const entry = await resolveEntry(request, docId);
    expect(entry).not.toBeNull();
    if (entry === null) throw new Error(`知识条目未找到: ${docId}`);

    // ── 浏览：进入分类目录 → 点击条目 → 内容渲染含关键词 ──
    await page.getByRole('tab', { name: '浏览' }).click();
    await page.getByRole('button', { name: entry.dirName, exact: true }).click();
    const fileRow = page.getByRole('button', { name: docId, exact: true });
    await expect(fileRow).toBeVisible({ timeout: 30000 });
    await fileRow.click();
    await expect(page.locator('pre', { hasText: keyword }).first()).toBeVisible({
      timeout: 15000,
    });

    // ── 搜索：唯一关键词命中（固定向量 → 余弦相似度恒 1.0，检索结果确定）──
    await page.getByRole('tab', { name: '搜索' }).click();
    await page.getByRole('textbox', { name: '搜索知识库' }).fill(keyword);
    await expect(page.getByText(/共找到 [1-9]\d* 条结果/)).toBeVisible({ timeout: 30000 });
  });
});

/** 遍历知识库目录，解析文件名（内容哈希）为 dirName + uri。
 * 注意：VFS 中已 ingest 的条目以目录节点形态列出（L0/L1/L2 子文件结构），
 * 因此不按 is_directory 过滤，只按名称匹配。 */
async function resolveEntry(
  request: APIRequestContext,
  docId: string,
): Promise<ResolvedEntry | null> {
  const root = await listEntries(request);
  for (const dir of root.filter((e) => e.is_directory)) {
    const children = await listEntries(request, dir.name);
    const file = children.find((e) => e.name === docId);
    if (file) return { dirName: dir.name, uri: file.uri };
  }
  return null;
}

/** GET /api/v1/knowledge/entries（可选 path 子目录）。失败返回空列表。 */
async function listEntries(request: APIRequestContext, path?: string): Promise<KnowledgeEntry[]> {
  const url = path
    ? `/api/v1/knowledge/entries?path=${encodeURIComponent(path)}`
    : '/api/v1/knowledge/entries';
  const res = await request.get(url);
  if (!res.ok()) return [];
  const body = (await res.json()) as { entries: KnowledgeEntry[] };
  return body.entries;
}

/** 删除指定条目（幂等：已删除返回 404 视为成功）。 */
async function cleanupEntry(request: APIRequestContext, docId: string): Promise<void> {
  const entry = await resolveEntry(request, docId);
  if (entry === null) return;
  const res = await request.post('/api/v1/knowledge/entries/delete', {
    data: { uri: entry.uri },
  });
  if (!res.ok() && res.status() !== 404) {
    throw new Error(`删除知识条目失败 ${entry.uri}: HTTP ${res.status()}`);
  }
}
