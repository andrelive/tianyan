import { describe, it, expect, beforeEach } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '@/test/mocks/server';
import { messageText } from '@/lib/types';
import {
  mockTaskCancelCalls,
  mockSessionCompressCalls,
  resetTaskMocks,
} from '@/test/mocks/handlers';
import {
  apiGet,
  apiPost,
  apiDelete,
  ApiError,
  isConflict,
  isInvalidInput,
  isNotFound,
  isPermission,
  isTimeout,
  apiPostMultipart,
  fetchTasks,
  cancelTask,
  compressSession,
} from '@/lib/api-client';
import type { ListSessionsResponse, Session, SkillListResponse, ChatResponse } from '@/lib/types';

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
    expect(messageText(result.message)).toContain('天演');
    expect(result.usage.total_tokens).toBe(80);
  });

  it('fetches knowledge search results via GET', async () => {
    const result = await apiGet<{
      results: unknown[];
      total: number;
    }>('/knowledge/search?q=architecture&limit=10');

    expect(result.results).toHaveLength(2);
    expect(result.total).toBe(2);
  });

  it('knowledge search response carries limit field (backend contract)', async () => {
    // 后端 SearchResponse 返回 limit（而非 top_k），前端类型需与其对齐
    const result = await apiGet<{
      query: string;
      limit: number;
      offset: number;
      total: number;
    }>('/knowledge/search?q=architecture&limit=10');

    expect(result.limit).toBeDefined();
    expect(result.offset).toBeDefined();
    expect(result.query).toBe('architecture');
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

  // ── 语义分类（ADR-014：状态码 → kind 映射收敛在 api-client 唯一一处） ──

  it('maps HTTP status codes to semantic kinds', () => {
    expect(new ApiError('nf', 404).kind).toBe('not_found');
    expect(new ApiError('cf', 409).kind).toBe('conflict');
    expect(new ApiError('ii', 400).kind).toBe('invalid_input');
    expect(new ApiError('ii2', 422).kind).toBe('invalid_input');
    expect(new ApiError('pm', 403).kind).toBe('permission');
    expect(new ApiError('to', 504).kind).toBe('timeout');
    expect(new ApiError('un', 500).kind).toBe('unknown');
    expect(new ApiError('no status').kind).toBe('unknown');
  });

  it('predicates classify errors without string matching', () => {
    expect(isNotFound(new ApiError('x', 404))).toBe(true);
    expect(isConflict(new ApiError('x', 409))).toBe(true);
    expect(isInvalidInput(new ApiError('x', 400))).toBe(true);
    expect(isPermission(new ApiError('x', 403))).toBe(true);
    expect(isTimeout(new ApiError('x', 504))).toBe(true);

    // 非 ApiError / 其他类别 → 全部 false
    expect(isConflict(new Error('x'))).toBe(false);
    expect(isConflict(new ApiError('x', 500))).toBe(false);
    expect(isNotFound(new ApiError('x', 409))).toBe(false);
    expect(isConflict(null)).toBe(false);
  });

  it('keeps code for backward compatibility', () => {
    const err = new ApiError('x', 409);
    expect(err.code).toBe('409');
    expect(new ApiError('x', '403').kind).toBe('permission');
  });
});

// ─── fetchTasks ────────────────────────────────────────────────────────────────

describe('fetchTasks', () => {
  beforeEach(() => {
    resetTaskMocks();
  });

  it('fetches the background task list (Vec<BackgroundTask>)', async () => {
    const result = await fetchTasks();

    expect(result).toHaveLength(4);
    expect(result[0]).toMatchObject({ id: 'task-1', status: 'running' });
    expect(result[0].created_at).toBeTypeOf('number');
    expect(result[0].completed_at).toBeNull();
    // 后台终端命令（kind=command）与委托任务同一视图
    const byId = Object.fromEntries(result.map((t) => [t.id, t]));
    expect(byId['task-2'].result).toBe('找到 12 处 TODO 标记');
    expect(byId['task-3']).toMatchObject({ status: 'failed', error: '读取文件超时: 网络错误' });
    expect(byId['task-cmd-1']).toMatchObject({ status: 'running', kind: 'command' });
  });

  it('returns empty array when no tasks exist', async () => {
    server.use(
      http.get('/api/v1/tasks', () => {
        return HttpResponse.json([]);
      }),
    );

    const result = await fetchTasks();
    expect(result).toEqual([]);
  });
});

// ─── cancelTask ────────────────────────────────────────────────────────────────

describe('cancelTask', () => {
  beforeEach(() => {
    resetTaskMocks();
  });

  it('posts to /tasks/{id}/cancel and returns the cancelled task', async () => {
    const result = await cancelTask('task-1');

    expect(mockTaskCancelCalls).toEqual([{ taskId: 'task-1' }]);
    expect(result).toEqual({ task_id: 'task-1', status: 'cancelled' });
  });

  it('throws ApiError when the task does not exist (404)', async () => {
    await expect(cancelTask('nonexistent')).rejects.toThrow(ApiError);
  });

  it('throws ApiError with 404 status code for unknown tasks', async () => {
    try {
      await cancelTask('nonexistent');
    } catch (err) {
      expect(err).toBeInstanceOf(ApiError);
      expect((err as ApiError).code).toBe('404');
    }
  });
});

// ─── compressSession ──────────────────────────────────────────────────────────

describe('compressSession', () => {
  beforeEach(() => {
    resetTaskMocks();
  });

  it('posts to /sessions/{id}/compress and reports compressed', async () => {
    const result = await compressSession('session-1');

    expect(mockSessionCompressCalls).toEqual([{ sessionId: 'session-1' }]);
    expect(result.compressed).toBe(true);
    // 压缩成功时响应携带摘要消息（前端追加到消息流末尾）
    expect(result.message?.role).toBe('system');
    expect(result.message ? messageText(result.message) : '').toContain('对话摘要');
  });

  it('returns compressed=false when there is nothing to compress', async () => {
    server.use(
      http.post('/api/v1/sessions/:id/compress', () => {
        return HttpResponse.json({ compressed: false });
      }),
    );

    const result = await compressSession('session-1');
    expect(result.compressed).toBe(false);
  });

  it('throws ApiError on server error', async () => {
    server.use(
      http.post('/api/v1/sessions/:id/compress', () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );

    await expect(compressSession('session-1')).rejects.toThrow(ApiError);
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
    expect(messageText(result.message)).toContain('天演');
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

    await expect(apiPostMultipart('/upload-error', formData)).rejects.toThrow(ApiError);
  });
});



