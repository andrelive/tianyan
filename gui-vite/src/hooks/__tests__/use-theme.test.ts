import { describe, it, expect, vi, afterEach } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useTheme } from '../use-theme';

afterEach(() => {
  vi.restoreAllMocks();
  document.documentElement.classList.remove('light', 'dark');
});

describe('useTheme', () => {
  it('applies the requested theme class and clears the other', () => {
    renderHook(() => useTheme('dark'));
    expect(document.documentElement.classList.contains('dark')).toBe(true);
    expect(document.documentElement.classList.contains('light')).toBe(false);

    renderHook(() => useTheme('light'));
    expect(document.documentElement.classList.contains('light')).toBe(true);
    expect(document.documentElement.classList.contains('dark')).toBe(false);
  });

  it('resolves system preference through matchMedia', () => {
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockReturnValue({ matches: true }),
    });
    renderHook(() => useTheme('system'));
    expect(document.documentElement.classList.contains('dark')).toBe(true);

    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockReturnValue({ matches: false }),
    });
    renderHook(() => useTheme('system'));
    expect(document.documentElement.classList.contains('light')).toBe(true);
  });
});
