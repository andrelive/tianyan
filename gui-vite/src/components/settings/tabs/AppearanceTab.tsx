import { SectionTitle } from './shared';
import { useAppStore } from '@/lib/store';
import type { Theme, FontSize } from '@/lib/types';

export default function AppearanceTab() {
  const theme = useAppStore((s) => s.theme);
  const fontSize = useAppStore((s) => s.fontSize);
  const setTheme = useAppStore((s) => s.setTheme);
  const setFontSize = useAppStore((s) => s.setFontSize);

  return (
    <div>
      <SectionTitle title="界面外观" />

      <div className="mb-6">
        <label className="text-sm font-medium text-[var(--color-text-primary)] block mb-3">
          主题
        </label>
        <div className="flex gap-4">
          {(['light', 'dark', 'system'] as Theme[]).map((t) => (
            <label
              key={t}
              className={`flex items-center gap-2 px-4 py-2.5 rounded-lg border cursor-pointer transition-colors ${
                theme === t
                  ? 'border-accent bg-accent-light text-accent'
                  : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-text-tertiary)]'
              }`}
            >
              <input
                type="radio"
                name="theme"
                value={t}
                checked={theme === t}
                onChange={() => setTheme(t)}
                className="sr-only"
              />
              <div
                className={`w-3.5 h-3.5 rounded-full border-2 flex items-center justify-center ${
                  theme === t ? 'border-accent' : 'border-[var(--color-text-tertiary)]'
                }`}
              >
                {theme === t && <div className="w-2 h-2 rounded-full bg-accent" />}
              </div>
              <span className="text-sm capitalize">
                {t === 'light' ? '浅色' : t === 'dark' ? '深色' : '跟随系统'}
              </span>
            </label>
          ))}
        </div>
      </div>

      <div>
        <label className="text-sm font-medium text-[var(--color-text-primary)] block mb-3">
          字体大小
        </label>
        <div className="flex gap-4">
          {(['small', 'medium', 'large'] as FontSize[]).map((s) => (
            <label
              key={s}
              className={`flex items-center gap-2 px-4 py-2.5 rounded-lg border cursor-pointer transition-colors ${
                fontSize === s
                  ? 'border-accent bg-accent-light text-accent'
                  : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-text-tertiary)]'
              }`}
            >
              <input
                type="radio"
                name="fontSize"
                value={s}
                checked={fontSize === s}
                onChange={() => setFontSize(s)}
                className="sr-only"
              />
              <div
                className={`w-3.5 h-3.5 rounded-full border-2 flex items-center justify-center ${
                  fontSize === s ? 'border-accent' : 'border-[var(--color-text-tertiary)]'
                }`}
              >
                {fontSize === s && <div className="w-2 h-2 rounded-full bg-accent" />}
              </div>
              <span className="text-sm capitalize">
                {s === 'small' ? '小' : s === 'medium' ? '中' : '大'}
              </span>
            </label>
          ))}
        </div>
      </div>
    </div>
  );
}
