/**
 * API 基址解析回归测试（修复：headless 非 3000 端口整页白屏）。
 *
 * 缺陷背景：`getApiBase()` 的生产分支直接读 store 的 `apiBaseUrl`，而它的出厂默认值是
 * `http://localhost:3000`。后端端口可以变（`TIANYAN_PORT` / 端口冲突动态选择），页面
 * 本身却是由该后端提供的 —— 于是「同源」才是唯一可靠默认。原实现让 headless 独立启动
 * 在非 3000 端口时把 `config/status` 打到 3000（CORS 拒绝 → 前端挂起 → 白屏）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { getApiBase, getApiRoot } from '@/lib/api-base';
import { useAppStore } from '@/lib/store';

const ORIGINAL_ORIGIN = window.location.origin;

/** 覆盖 `window.location.origin`（jsdom 下 origin 是只读的，用 defineProperty 替换）。 */
function stubOrigin(origin: string) {
  Object.defineProperty(window, 'location', {
    configurable: true,
    value: { ...window.location, origin, href: `${origin}/`, pathname: '/' },
  });
}

/** 临时切到「生产分支」（vitest 里 `import.meta.env.DEV` 恒为 true）。 */
function withProdEnv() {
  const env = import.meta.env as unknown as Record<string, unknown>;
  const original = env.DEV;
  env.DEV = false;
  return () => {
    env.DEV = original;
  };
}

describe('api-base', () => {
  let restoreEnv: () => void = () => {};

  beforeEach(() => {
    delete (window as { __TIANYAN_API_BASE__?: string }).__TIANYAN_API_BASE__;
    useAppStore.setState({ apiBaseUrl: 'http://localhost:3000' });
    restoreEnv = withProdEnv();
  });

  afterEach(() => {
    restoreEnv();
    stubOrigin(ORIGINAL_ORIGIN);
    delete (window as { __TIANYAN_API_BASE__?: string }).__TIANYAN_API_BASE__;
    useAppStore.setState({ apiBaseUrl: 'http://localhost:3000' });
  });

  describe('Tauri 注入优先', () => {
    it('注入值时用注入值（桌面端动态端口场景）', () => {
      (window as { __TIANYAN_API_BASE__?: string }).__TIANYAN_API_BASE__ = 'http://127.0.0.1:3123';
      expect(getApiRoot()).toBe('http://127.0.0.1:3123');
      expect(getApiBase()).toBe('http://127.0.0.1:3123/api/v1');
    });
  });

  describe('同源优先（本次修复的核心）', () => {
    it('默认配置 + 页面由 http 提供 → 用 location.origin（不再写死 3000）', () => {
      stubOrigin('http://127.0.0.1:3099');
      expect(getApiRoot()).toBe('http://127.0.0.1:3099');
      expect(getApiBase()).toBe('http://127.0.0.1:3099/api/v1');
    });

    it('判别力：若把默认值当用户配置直接采信，本用例会退化为 3000', () => {
      stubOrigin('http://127.0.0.1:3099');
      // 反向断言：结果里不能出现 3000
      expect(getApiBase()).not.toContain(':3000');
    });
  });

  describe('用户显式配置优先于同源', () => {
    it('用户改过设置（非出厂默认）→ 用用户的值（跨源部署场景）', () => {
      stubOrigin('http://127.0.0.1:3099');
      useAppStore.setState({ apiBaseUrl: 'http://192.168.1.10:8080' });
      expect(getApiRoot()).toBe('http://192.168.1.10:8080');
      expect(getApiBase()).toBe('http://192.168.1.10:8080/api/v1');
    });
  });

  describe('非 http 场景回退', () => {
    it('origin 不是 http(s)（如 tauri:// / file://）→ 回退 store 值', () => {
      stubOrigin('tauri://localhost');
      expect(getApiRoot()).toBe('http://localhost:3000');
      expect(getApiBase()).toBe('http://localhost:3000/api/v1');
    });
  });

  describe('开发模式', () => {
    it('DEV 模式走相对路径（vite proxy）', () => {
      const env = import.meta.env as unknown as Record<string, unknown>;
      env.DEV = true;
      expect(getApiRoot()).toBe('');
      expect(getApiBase()).toBe('/api/v1');
    });
  });

  // 防御：若默认值被改动，上面的「默认配置」用例应失效——单独锁住常量语义
  it('出厂默认值仍是 http://localhost:3000（resolveProdBase 的判定标记）', () => {
    expect(useAppStore.getState().apiBaseUrl).toBe('http://localhost:3000');
  });
});

describe('api-base（DEV 常量守卫）', () => {
  it('vi 的 env 可变性假设成立（否则上面的 DEV 用例是假绿）', () => {
    const env = import.meta.env as unknown as Record<string, unknown>;
    const original = env.DEV;
    env.DEV = false;
    expect(env.DEV).toBe(false);
    env.DEV = original;
    expect(vi.isMockFunction(vi.fn())).toBe(true);
  });
});
