import { describe, expect, it } from 'vitest';
import { lastMessageUsage, sumSessionUsage } from '@/lib/token-usage';
import type { ChatMessage } from '@/lib/types';

function msg(overrides: Partial<ChatMessage>): ChatMessage {
  return { role: 'assistant', segments: [], ...overrides };
}

describe('lastMessageUsage', () => {
  it('returns null when no message carries usage', () => {
    expect(lastMessageUsage([msg({ segments: [{ type: 'text', text: 'a' }] }), msg({ segments: [{ type: 'text', text: 'b' }] })], 32000)).toBeNull();
  });

  it('picks the last message with prompt_tokens > 0 (skip trailing zero-usage)', () => {
    const messages = [
      msg({ usage: { prompt_tokens: 100, completion_tokens: 50, total_tokens: 150 } }),
      msg({ segments: [{ type: 'text', text: 'no usage' }] }),
      msg({ usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 } }),
    ];
    const r = lastMessageUsage(messages, 32000);
    expect(r?.prompt_tokens).toBe(100);
  });

  it('fills cache fields with 0 and uses fallback window', () => {
    const r = lastMessageUsage(
      [msg({ usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 } })],
      64000,
    );
    expect(r).toEqual({
      prompt_tokens: 10,
      completion_tokens: 5,
      total_tokens: 15,
      cache_read: 0,
      cache_write: 0,
      context_window: 64000,
    });
  });

  it('keeps real context_window when fallback is 0', () => {
    const r = lastMessageUsage(
      [msg({ usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 } })],
      0,
    );
    expect(r?.context_window).toBe(0);
  });

  it('uses system prefix + summary output for compression marker message', () => {
    const messages = [
      // 第一轮 assistant：prompt = 系统前缀 + 首条用户输入（≈ 系统前缀）
      msg({ usage: { prompt_tokens: 1000, completion_tokens: 200, total_tokens: 1200 } }),
      // 压缩摘要：prompt = 压缩请求输入（压缩前上下文），completion = 摘要输出
      msg({
        role: 'system',
        compression_marker: true,
        usage: { prompt_tokens: 50000, completion_tokens: 800, total_tokens: 50800 },
      }),
    ];
    const r = lastMessageUsage(messages, 64000);
    // 压缩后占用 ≈ 系统前缀（1000）+ 摘要输出（800），而非压缩前 50000
    expect(r?.prompt_tokens).toBe(1800);
    expect(r?.completion_tokens).toBe(800);
  });

  it('falls back to last real usage when compression marker has zero usage', () => {
    const messages = [
      msg({ usage: { prompt_tokens: 300, completion_tokens: 100, total_tokens: 400 } }),
      msg({ role: 'system', compression_marker: true }),
    ];
    const r = lastMessageUsage(messages, 64000);
    expect(r?.prompt_tokens).toBe(300);
  });

  it('prefers a newer assistant message over an old compression marker', () => {
    const messages = [
      msg({ usage: { prompt_tokens: 1000, completion_tokens: 200, total_tokens: 1200 } }),
      msg({ role: 'system', compression_marker: true, usage: { prompt_tokens: 50000, completion_tokens: 800, total_tokens: 50800 } }),
      // 压缩后新请求完成：新 assistant 带压缩后真实占用
      msg({ usage: { prompt_tokens: 2500, completion_tokens: 500, total_tokens: 3000 } }),
    ];
    const r = lastMessageUsage(messages, 64000);
    expect(r?.prompt_tokens).toBe(2500);
  });
});

describe('sumSessionUsage', () => {
  it('returns null when nothing consumed', () => {
    expect(sumSessionUsage([msg({ segments: [{ type: 'text', text: 'a' }] })])).toBeNull();
    expect(
      sumSessionUsage([
        msg({ usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 } }),
      ]),
    ).toBeNull();
  });

  it('accumulates across messages with cache split', () => {
    const messages = [
      msg({
        usage: { prompt_tokens: 100, completion_tokens: 30, total_tokens: 130, cache_read: 40 },
      }),
      msg({
        usage: { prompt_tokens: 200, completion_tokens: 70, total_tokens: 270, cache_read: 60 },
      }),
      msg({ segments: [{ type: 'text', text: 'no usage' }] }),
    ];
    expect(sumSessionUsage(messages)).toEqual({
      uncachedInput: 200, // (100-40) + (200-60)
      cachedInput: 100,
      completion: 100,
    });
  });

  it('includes compression request usage from marker message', () => {
    const messages = [
      msg({
        usage: { prompt_tokens: 100, completion_tokens: 30, total_tokens: 130, cache_read: 40 },
      }),
      msg({
        role: 'system',
        compression_marker: true,
        usage: { prompt_tokens: 2000, completion_tokens: 500, total_tokens: 2500, cache_read: 1500 },
      }),
    ];
    // 压缩请求消耗（2000 输入 / 500 输出 / 1500 缓存）计入会话累计：
    // uncached = (100-40) + (2000-1500) = 560；cached = 40 + 1500 = 1540
    expect(sumSessionUsage(messages)).toEqual({
      uncachedInput: 560,
      cachedInput: 1540,
      completion: 530,
    });
  });
});

