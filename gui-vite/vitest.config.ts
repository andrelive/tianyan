import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import path from 'path';

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    css: false,
    coverage: {
      provider: 'v8',
      reporter: ['text', 'json', 'html'],
      include: ['src/**/*.{ts,tsx}'],
      exclude: [
        'src/test/**',
        'src/**/__tests__/**',
        'src/main.tsx',
        'src/vite-env.d.ts',
      ],
      // G5：覆盖率门禁（基线 2026-08：84.41/84.14/77.9/84.41；
      // 阈值留余量防小波动误伤，新增代码不得显著拉低覆盖率）
      thresholds: {
        statements: 80,
        branches: 75,
        functions: 70,
        lines: 80,
      },
    },
    include: ['src/**/__tests__/**/*.{test,spec}.{ts,tsx}'],
  },
});
