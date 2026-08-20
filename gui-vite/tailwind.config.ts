import type { Config } from 'tailwindcss';

export default {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        accent: {
          DEFAULT: '#2563eb',
          hover: '#1d4ed8',
          light: '#eff6ff',
        },
      },
      // 非标题字体整体上调一号：text-xs 0.75→0.875rem、text-sm 0.875→1rem。
      // text-base/text-lg 保持默认——对话历史正文（text-lg）与二级栏目
      // 标题（text-lg = 1.125rem）不受影响。
      fontSize: {
        xs: '0.875rem',
        sm: '1rem',
      },
    },
  },
  plugins: [],
} satisfies Config;
