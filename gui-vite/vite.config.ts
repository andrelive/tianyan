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
