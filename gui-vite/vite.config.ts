import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

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
        target: 'http://localhost:3000',
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
        // /health（非 API 前缀）：关于页版本展示 / 连接测试 / 重启轮询同样代理到后端
        '/health': {
          target: 'http://localhost:3000',
          changeOrigin: true,
        },
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
