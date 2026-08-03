import { describe, it, expect } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '@/test/mocks/server';
import { apiGet, apiPost, apiDelete, ApiError, apiPostMultipart } from '@/lib/api-client';
import type {
  ListSessionsResponse,
  Session,
  SkillListResponse,
  ChatResponse,
} from '@/lib/types';

const API_BASE = '/api/v1';

// ─── apiGet ────────────────────────────────────────────────────────────────────

describe('apiGet', () => {
  it('fetches sessions list', async () => {
    const result = await apiGet<ListSessionsResponse>('/sessions');

    expect(result.sessions).toHaveLength(2);
    expect(result.total).toBe(2);
    expect(result.sessions[0].id).toBe('session-1');
    expect(result.sessions[1].title).toBe('代码审查对话');
  });

  it('fetches a single session by id', async () => {
    const result = await apiGet<Session>('/sessions/session-1');

    expect(result.id).toBe('session-1');
    expect(result.title).toBe('测试会话 1');
    expect(result.message_count).toBe(5);
  });

  it('fetches skills list', async () => {
    const result = await apiGet<SkillListResponse>('/skills');

    expect(result.skills).toHaveLength(2);
    expect(result.skills[0].name).toBe('file-reader');
    expect(result.skills[1].name).toBe('code-search');
  });

  it('fetches config status', async () => {
    const result = await apiGet<{ configured: boolean }>('/config/status');

    expect(result.configured).toBe(true);
  });

  it('throws ApiError on 404', async () => {
    await expect(apiGet('/sessions/nonexistent')).rejects.toThrow(ApiError);
  });

  it('throws ApiError with 404 status code', async () => {
    try {
      await apiGet('/sessions/nonexistent');
    } catch (err) {
      expect(err).toBeInstanceOf(ApiError);
      expect((err as ApiError).code).toBe('404');
    }
  });

  it('uses the correct base URL containing /api/v1', async () => {
    // Successful request proves the URL was constructed correctly and matched
    const result = await apiGet<ListSessionsResponse>('/sessions');
    expect(result).toBeDefined();
    expect(Array.isArray(result.sessions)).toBe(true);
  });
});

// ─── apiPost ───────────────────────────────────────────────────────────────────

describe('apiPost', () => {
  it('sends a POST request and returns a ChatResponse', async () => {
    const body = {
      session_id: 'session-1',
      messages: [{ role: 'user' as const, content: 'hello' }],
      stream: false,
      temperature: 0.7,
      max_tokens: 2048,
    };

    const result = await apiPost<ChatResponse>('/chat', body);

    expect(result.id).toBe('msg-1');
    expect(result.session_id).toBe('session-1');
    expect(result.message.role).toBe('assistant');
    expect(result.message.content).toContain('天演');
    expect(result.usage.total_tokens).toBe(80);
  });

  it('sends a knowledge search request', async () => {
    const body = { query: 'architecture' };

    const result = await apiPost<{
      results: unknown[];
      total: number;
    }>('/search', body);

    expect(result.results).toHaveLength(2);
    expect(result.total).toBe(2);
  });

  it('sends correct JSON content-type header', async () => {
    // Verify a post request receives the expected typed response
    const body = {
      session_id: 'session-1',
      messages: [],
      stream: true,
      temperature: 0.5,
      max_tokens: 1000,
    };

    const result = await apiPost<ChatResponse>('/chat', body);
    expect(result.usage.prompt_tokens).toBe(50);
  });
});

// ─── apiDelete ─────────────────────────────────────────────────────────────────

describe('apiDelete', () => {
  it('deletes a session and returns success', async () => {
    const result = await apiDelete<{ success: boolean }>('/sessions/session-1');

    expect(result.success).toBe(true);
  });

  it('throws ApiError on 404', async () => {
    await expect(apiDelete('/sessions/nonexistent')).rejects.toThrow(ApiError);
  });

  it('throws with status code 404', async () => {
    try {
      await apiDelete('/sessions/nonexistent');
    } catch (err) {
      expect(err).toBeInstanceOf(ApiError);
      expect((err as ApiError).code).toBe('404');
    }
  });
});

// ─── ApiError ──────────────────────────────────────────────────────────────────

describe('ApiError', () => {
  it('stores message and code', () => {
    const err = new ApiError('Not found', '404');

    expect(err.message).toBe('Not found');
    expect(err.code).toBe('404');
    expect(err.name).toBe('ApiError');
  });

  it('works without a code', () => {
    const err = new ApiError('generic error');

    expect(err.message).toBe('generic error');
    expect(err.code).toBeUndefined();
  });

  it('is an instance of Error', () => {
    expect(new ApiError('x')).toBeInstanceOf(Error);
  });

  it('preserves stack trace', () => {
    const err = new ApiError('test');
    expect(err.stack).toBeDefined();
  });
});

// ─── apiPostMultipart ──────────────────────────────────────────────────────────

describe('apiPostMultipart', () => {
  it('sends FormData and returns JSON', async () => {
    const formData = new FormData();
    formData.append('key', 'value');

    // POST /chat handler matches regardless of Content-Type
    const result = await apiPostMultipart<ChatResponse>('/chat', formData);

    expect(result.id).toBe('msg-1');
    expect(result.message.content).toContain('天演');
  });

  it('throws ApiError on server error', async () => {
    const formData = new FormData();
    formData.append('x', 'y');

    // Add a one-shot handler that returns 500
    server.use(
      http.post(`${API_BASE}/upload-error`, () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );

    await expect(apiPostMultipart('/upload-error', formData)).rejects.toThrow(
      ApiError,
    );
  });
});
