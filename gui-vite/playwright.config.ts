import { defineConfig, devices } from '@playwright/test';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// ESM 配置文件中没有 __dirname：从 import.meta.url 推导（package.json 为 "type": "module"）。
const __dirname = path.dirname(fileURLToPath(import.meta.url));

/**
 * e2e 后端端口：**刻意不用 3000**——本地 3000 常被运行中的桌面应用
 * （tianyan-tauri）占用，而后端条目 `reuseExistingServer: false` 会因端口冲突
 * 直接失败，等于「想跑 e2e 就得先关掉自己的应用」（还会掐断用户正在进行的会话）。
 * 改用备用端口后两者可共存。备用端口若也被占，用 `TIANYAN_E2E_PORT` 覆盖。
 */
const E2E_PORT = Number.parseInt(process.env.TIANYAN_E2E_PORT ?? '3099', 10);
const E2E_API = `http://localhost:${E2E_PORT}`;

export default defineConfig({
  testDir: './e2e',
  globalTeardown: './e2e/global-teardown.ts',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: 'html',
  use: {
    baseURL: 'http://localhost:5100',
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  // webServer 数组按声明顺序启动：
  // 1) mock-llm（OpenAI 兼容 mock，127.0.0.1:8765）
  // 2) tianyan-server（真实后端；TIANYAN_CONFIG / TIANYAN_DATA_DIR 环境变量注入 e2e 配置
  //    —— 配置里 working_directory 相对进程启动目录解析，webServer 命令以本配置所在目录
  //    （gui-vite/）为 cwd 启动，因此 ../scripts/e2e/fixtures/workspace 正确指向仓库 fixtures。
  //    每个运行进程独立 data dir，实现跨运行隔离。TIANYAN_PORT 指定备用端口，
  //    避开运行中的桌面应用 3000——见上方 E2E_PORT 说明）
  // 3) vite dev（前端，5100 代理 /api → e2e 后端端口）
  webServer: [
    {
      command: 'node ../scripts/e2e/mock-llm.mjs',
      port: 8765,
      reuseExistingServer: !process.env.CI,
      timeout: 30000,
    },
    {
      command: 'cargo run -p tianyan-server',
      url: `http://127.0.0.1:${E2E_PORT}/health`,
      // 后端条目无条件自启（不复用已有进程）：若备用端口也被占用，
      // 启动将因端口冲突而响亮失败——绝不静默复用并污染其他实例的数据。
      // mock 与 vite 条目保持可复用（幂等无害 / 复用用户自己的 dev server）。
      reuseExistingServer: false,
      timeout: 300000,
      env: {
        TIANYAN_CONFIG: path.resolve(__dirname, '../scripts/e2e/tianyan.e2e.toml'),
        TIANYAN_DATA_DIR: path.join(os.tmpdir(), 'tianyan-e2e-' + process.pid),
        TIANYAN_PORT: String(E2E_PORT),
      },
    },
    {
      command: 'npm run dev',
      url: 'http://localhost:5100',
      reuseExistingServer: !process.env.CI,
      timeout: 30000,
      env: {
        TIANYAN_API_TARGET: E2E_API,
      },
    },
  ],
});
