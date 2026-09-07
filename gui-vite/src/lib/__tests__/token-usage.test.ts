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
});

