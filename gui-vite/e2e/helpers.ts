import type { APIRequestContext } from '@playwright/test';

/**
 * e2e 后端实例身份守卫。
 *
 * 真实集成 spec 会向共享数据目录写入数据（会话/知识条目/工作区改动），
 * 若 3000 端口被开发后端占用（误连），将污染开发数据。因此每个真实 spec
 * 在 beforeAll 断言：workspace tree 必须包含 e2e 夹具文件 hello.txt——
 * 只有 e2e 配置（working_directory 指向 scripts/e2e/fixtures/workspace）
 * 的后端才会返回它。
 */
export async function assertE2eBackend(request: APIRequestContext): Promise<void> {
  const tree = await request.get('/api/v1/workspace/tree?depth=1');
  if (!tree.ok()) {
    throw new Error(
      `e2e 身份守卫失败：/workspace/tree 返回 HTTP ${tree.status()}——e2e 端口上可能不是 e2e 后端。` +
        '请确认 e2e 端口（默认 3099，TIANYAN_E2E_PORT 可改）未被其他后端占用。',
    );
  }
  const body = (await tree.json()) as { entries?: Array<{ name: string }> };
  const hasFixture = (body.entries ?? []).some((e) => e.name === 'hello.txt');
  if (!hasFixture) {
    throw new Error(
      'e2e 身份守卫失败：workspace tree 不含夹具 hello.txt——e2e 端口上不是 e2e 实例（开发后端？）。' +
        '真实 spec 会写入共享数据，已中止以防数据污染。',
    );
  }
}
