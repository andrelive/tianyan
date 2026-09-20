import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

/**
 * 后端代理目标：默认 `http://localhost:3000`（独立 server 与桌面端的默认端口）。
 * e2e 经 `TIANYAN_API_TARGET` 指向备用端口——本地 3000 常被运行中的桌面应用
 * （tianyan-tauri）占用，e2e 不该为此要求用户关掉自己的应用（见 playwright.config.ts）。
 */
const API_TARGET = process.env.TIANYAN_API_TARGET ?? 'http://localhost:3000';

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  server: {
    port: 5100,
    proxy: {
      '/api': {
        target: API_TARGET,
        changeOrigin: true,
        configure: (proxy) => {
          // SSE 长连接（GET /events）：http-proxy 的 writeHeaders 在 proxyRes
          // 事件后同步执行——flushHeaders 必须延迟到 writeHeaders 之后
          // （否则先发空头 → headersSent=true → content-type 丢失）
          proxy.on('proxyRes', (proxyRes, _req, res) => {
            if (proxyRes.headers['content-type']?.includes('text/event-stream')) {
              setImmediate(() => res.flushHeaders());
            }
          });
        },
      },
      // /health（非 API 前缀）：关于页版本展示 / 连接测试 / 重启轮询同样代理到后端。
      // 注：此前误嵌在 '/api' 的配置对象**内部**——http-proxy 只识别 proxy 的
      // **顶层**键，故该条实际从未生效（顺带修正）。
      '/health': {
        target: API_TARGET,
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: {
      output: {
        // 大依赖按域分块（C6）：配合路由级 React.lazy，首屏只加载 react 壳 +
        // chat 域；markdown/编辑器块按需进入
        manualChunks: {
          'vendor-react': ['react', 'react-dom', 'react-router-dom', 'zustand'],
          'vendor-editor': [
            '@codemirror/state',
            '@codemirror/view',
            '@codemirror/language',
            '@codemirror/commands',
            '@codemirror/merge',
            '@codemirror/lang-javascript',
            '@codemirror/lang-python',
            '@codemirror/lang-rust',
          ],
          'vendor-markdown': ['react-markdown', 'remark-gfm', 'react-syntax-highlighter'],
        },
      },
    },
  },
});
