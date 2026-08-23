import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '@/test/mocks/server';
import { useProviderScan } from '../use-provider-scan';

beforeEach(() => {
  vi.clearAllMocks();
});

describe('useProviderScan', () => {
  it('scans and defaults new models to picked, excluding already-configured ones', async () => {
    // 已配置 llama3:8b → 扫描结果中只有 nomic-embed-text 应默认勾选
    const known = new Set(['llama3:8b']);
    const { result } = renderHook(() => useProviderScan(() => known));

    await act(async () => {
      await result.current.scan(0, 'http://localhost:11434', 'ollama', 'ollama');
    });

    const st = result.current.scanState[0];
    expect(st?.scanning).toBe(false);
    expect(st?.error).toBeNull();
    expect(st?.models.map((m) => m.name)).toEqual(['llama3:8b', 'nomic-embed-text']);
    expect(st?.picked).toEqual(['nomic-embed-text']);
  });

  it('reports scan failure through error without models', async () => {
    server.use(
      http.post('/api/v1/config/providers/scan', () =>
        HttpResponse.json({ success: false, error: '端点不可达', models: [] }),
      ),
    );
    const { result } = renderHook(() => useProviderScan(() => new Set()));

    await act(async () => {
      await result.current.scan(0, 'http://bad', 'openai', 'x');
    });

    const st = result.current.scanState[0];
    expect(st?.scanning).toBe(false);
    expect(st?.error).toBe('端点不可达');
    expect(st?.models).toEqual([]);
  });

  it('surfaces thrown errors (network) as error text', async () => {
    server.use(
      http.post('/api/v1/config/providers/scan', () => new HttpResponse(null, { status: 500 })),
    );
    const { result } = renderHook(() => useProviderScan(() => new Set()));

    await act(async () => {
      await result.current.scan(0, 'http://bad', 'openai', 'x');
    });

    const st = result.current.scanState[0];
    expect(st?.scanning).toBe(false);
    expect(st?.error).toBeTruthy();
  });

  it('toggles picked models per provider', async () => {
    const { result } = renderHook(() => useProviderScan(() => new Set()));
    await act(async () => {
      await result.current.scan(0, 'http://localhost:11434', 'ollama', 'ollama');
    });

    // 空 known 集合 → 两个模型都默认勾选；取消 nomic-embed-text 后剩 llama3:8b
    act(() => {
      result.current.togglePicked(0, 'nomic-embed-text');
    });
    expect(result.current.scanState[0]?.picked).toEqual(['llama3:8b']);

    act(() => {
      result.current.togglePicked(0, 'nomic-embed-text');
    });
    expect(result.current.scanState[0]?.picked).toEqual(['llama3:8b', 'nomic-embed-text']);
  });

  it('adoptPicked returns selected models and clears the scan state', async () => {
    const { result } = renderHook(() => useProviderScan(() => new Set()));
    await act(async () => {
      await result.current.scan(0, 'http://localhost:11434', 'ollama', 'ollama');
    });

    let adopted: unknown[] | null = null;
    act(() => {
      adopted = result.current.adoptPicked(0);
    });
    expect((adopted ?? []).map((m) => (m as { name: string }).name)).toEqual([
      'llama3:8b',
      'nomic-embed-text',
    ]);
    const st = result.current.scanState[0];
    expect(st?.models).toEqual([]);
    expect(st?.picked).toEqual([]);
  });

  it('adoptPicked with nothing picked returns empty without clearing', async () => {
    const { result } = renderHook(() =>
      useProviderScan(() => new Set(['llama3:8b', 'nomic-embed-text'])),
    );
    await act(async () => {
      await result.current.scan(0, 'http://localhost:11434', 'ollama', 'ollama');
    });

    let adopted: unknown[] | null = null;
    act(() => {
      adopted = result.current.adoptPicked(0);
    });
    expect(adopted).toEqual([]);
    expect(result.current.scanState[0]?.models.length).toBe(2);
  });

  it('setScanProtocol switches protocol for an unscanned provider', () => {
    const { result } = renderHook(() => useProviderScan(() => new Set()));

    act(() => {
      result.current.setScanProtocol(3, 'ollama');
    });
    expect(result.current.scanState[3]?.protocol).toBe('ollama');
    expect(result.current.scanState[3]?.scanning).toBe(false);
  });
});
