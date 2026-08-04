import { http, HttpResponse } from 'msw';
import type {
  Session,
  Skill,
  ListSessionsResponse,
  ChatResponse,
  SkillListResponse,
  ModelsResponse,
} from '@/lib/types';

const API_BASE = '/api/v1';

// ========== Session handlers ==========

export const mockSessions: Session[] = [
  {
    id: 'session-1',
    title: '测试会话 1',
    created_at: '2026-07-20T10:00:00Z',
    updated_at: '2026-07-23T08:00:00Z',
    message_count: 5,
  },
  {
    id: 'session-2',
    title: '代码审查对话',
    created_at: '2026-07-22T14:00:00Z',
    updated_at: '2026-07-23T09:30:00Z',
    message_count: 12,
  },
];

// ========== Skill handlers ==========

export const mockSkills: Skill[] = [
  {
    id: 'skill-1',
    name: 'file-reader',
    description: '读取文件内容',
    parameters: [{ name: 'path', type: 'string', description: '文件路径', required: true }],
    category: '文件操作',
    version: '1.0.0',
    enabled: true,
  },
  {
    id: 'skill-2',
    name: 'code-search',
    description: '搜索代码库',
    parameters: [
      { name: 'query', type: 'string', description: '搜索关键词', required: true },
      { name: 'path', type: 'string', description: '搜索路径', required: false },
    ],
    category: '代码工具',
    version: '1.0.0',
    enabled: true,
  },
];

// ========== Chat mock ==========

export const mockChatResponse: ChatResponse = {
  id: 'msg-1',
  session_id: 'session-1',
  message: {
    role: 'assistant',
    content: '你好！我是天演，有什么可以帮助你的？',
    timestamp: '2026-07-23T10:00:00Z',
  },
  usage: {
    prompt_tokens: 50,
    completion_tokens: 30,
    total_tokens: 80,
  },
};

// ========== Knowledge mock ==========

export const mockKnowledgeResults = {
  query: 'architecture',
  results: [
    {
      id: 'doc-1',
      title: '架构设计文档',
      content: '## 系统架构\n\n天演采用四层架构...',
      score: 0.95,
      source: 'docs/architecture.md',
    },
    {
      id: 'doc-2',
      title: 'API 文档',
      content: '## API 端点\n\nGET /api/v1/sessions...',
      score: 0.82,
      source: 'docs/api.md',
    },
  ],
  total: 2,
  limit: 10,
  offset: 0,
  suggestions: ['架构', 'VFS', 'Session'],
};

// ========== Config mock ==========

export const mockConfigStatus = { configured: true };

export const mockModelsResponse: ModelsResponse = {
  providers: [
    { name: 'openai', endpoint: 'https://api.openai.com/v1', enabled: true, model_count: 2 },
  ],
  models: [
    { name: 'gpt-4o', provider: 'openai', capabilities: ['chat', 'vision'] },
    { name: 'text-embedding-3-small', provider: 'openai', capabilities: ['text-embedding'] },
  ],
  preferences: {
    chat: { provider: 'openai', model: 'gpt-4o' },
    embedding: null,
    vision: null,
  },
};

export const mockTianyanConfig = {
  agent: {
    enable_skills: true,
    enable_memory: true,
    stream_responses: true,
    enable_thinking: false,
    default_top_k: 5,
    loaded_rules_top_k: 5,
    loaded_rules_max_tokens: 800,
    max_turns: 200,
  },
  models: {
    providers: [
      {
        name: 'openai',
        endpoint: 'https://api.openai.com/v1',
        api_key: 'sk-test',
        models: [
          { name: 'gpt-4o', capabilities: ['chat', 'vision'] },
          { name: 'text-embedding-3-small', capabilities: ['text-embedding'] },
        ],
        timeout: 60,
        enabled: true,
        headers: {},
      },
    ],
    preferences: {
      chat: { provider: 'openai', model: 'gpt-4o' },
      embedding: null,
      vision: null,
    },
  },
  storage: {
    data_dir: '~/.local/share/tianyan',
    max_storage_size: 0,
    auto_cleanup: true,
    cleanup_days: 365,
    vector: {
      collection_name: 'tianyan_data',
      vector_dimension: 1536,
    },
  },
  logging: {
    level: 'info',
    format: 'text',
    max_file_size: 10,
    max_files: 5,
    include_timestamp: true,
    include_location: false,
  },
  security: {
    enabled: true,
    confirm_commands: true,
    audit_logging: true,
    max_file_size: 10485760,
    allowed_directories: [],
    blocked_directories: [],
    allowed_commands: [],
    blocked_commands: [],
  },
  memory: {
    max_session_memory: 8000,
    max_long_term_memory: 10000,
    importance_threshold: 0.5,
    auto_consolidation: true,
    consolidation_interval: 3600,
    decay_rate: 0.01,
  },
  retrieval: {
    default_top_k: 10,
    min_score: 0.5,
    two_stage_retrieval: true,
    l0_multiplier: 3,
    max_context_tokens: 4000,
    enable_cache: true,
    cache_ttl: 300,
  },
};

// ========== Handler mapping ==========

export const handlers = [
  // Sessions
  http.get(`${API_BASE}/sessions`, () => {
    const response: ListSessionsResponse = {
      sessions: mockSessions,
      total: mockSessions.length,
    };
    return HttpResponse.json(response);
  }),

  http.get(`${API_BASE}/sessions/:id`, ({ params }) => {
    const session = mockSessions.find((s) => s.id === params.id);
    if (!session) return new HttpResponse(null, { status: 404 });
    return HttpResponse.json(session);
  }),

  http.delete(`${API_BASE}/sessions/:id`, ({ params }) => {
    const session = mockSessions.find((s) => s.id === params.id);
    if (!session) return new HttpResponse(null, { status: 404 });
    return HttpResponse.json({ success: true });
  }),

  // Skills
  http.get(`${API_BASE}/skills`, () => {
    const response: SkillListResponse = { skills: mockSkills };
    return HttpResponse.json(response);
  }),

  // Chat
  http.post(`${API_BASE}/chat`, () => {
    return HttpResponse.json(mockChatResponse);
  }),

  // Session messages
  // 重命名会话标题（后端为 POST /sessions/{id}/title）
  http.post(`${API_BASE}/sessions/:id/title`, async ({ request, params }) => {
    const body = (await request.json()) as { title?: string };
    const session = mockSessions.find((s) => s.id === params.id);
    if (session && body.title) {
      session.title = body.title;
    }
    return HttpResponse.json({ success: true, message: '标题已更新' });
  }),

  http.get(`${API_BASE}/sessions/:id/messages`, ({ params }) => {
    return HttpResponse.json({
      session_id: params.id,
      messages: [
        {
          role: 'user',
          content: '你好',
          timestamp: '2026-07-23T10:00:00Z',
        },
        {
          role: 'assistant',
          content: '你好！我是天演，有什么可以帮助你的？',
          timestamp: '2026-07-23T10:00:05Z',
        },
      ],
    });
  }),

  // Session message delete: 删除该消息及其后，返回剩余消息
  http.post(`${API_BASE}/sessions/:id/messages/delete`, ({ params }) => {
    return HttpResponse.json({
      session_id: params.id,
      messages: [
        {
          role: 'user',
          content: '你好',
          timestamp: '2026-07-23T10:00:00Z',
        },
      ],
    });
  }),

  // Session message redo: 恢复被回退的消息
  http.post(`${API_BASE}/sessions/:id/messages/redo`, ({ params }) => {
    return HttpResponse.json({
      session_id: params.id,
      messages: [
        {
          role: 'user',
          content: '你好',
          timestamp: '2026-07-23T10:00:00Z',
        },
        {
          role: 'assistant',
          content: '你好！我是天演，有什么可以帮助你的？',
          timestamp: '2026-07-23T10:00:05Z',
        },
      ],
    });
  }),

  // Chat stream (SSE)
  http.post(`${API_BASE}/chat/stream`, async ({ request }) => {
    const encoder = new TextEncoder();
    const body = (await request.json()) as { messages?: { content?: string }[] };
    const msgs = body.messages ?? [];
    const lastContent = msgs.length > 0 ? msgs[msgs.length - 1].content ?? '' : '';

    const chunks = lastContent.includes('[error-test]')
      ? [
          'data: {"id":"msg-err","session_id":"session-1","delta":"请求校验失败: [error-test] 是非法输入","finish_reason":null,"chunk_type":"error","skill_calls":null}\n\n',
        ]
      : [
          'data: {"id":"msg-1","session_id":"session-1","delta":"你好","chunk_type":"answer"}\n\n',
          'data: {"id":"msg-1","session_id":"session-1","delta":"！","chunk_type":"answer"}\n\n',
          'data: {"id":"msg-1","session_id":"session-1","delta":"","finish_reason":"stop","chunk_type":"answer"}\n\n',
        ];

    const stream = new ReadableStream({
      start(controller) {
        for (const chunk of chunks) {
          controller.enqueue(encoder.encode(chunk));
        }
        controller.close();
      },
    });
    return new HttpResponse(stream, {
      headers: { 'Content-Type': 'text/event-stream' },
    });
  }),

  // Chat clarify（追问回答）
  http.post(`${API_BASE}/chat/clarify`, () => {
    return HttpResponse.json({
      id: 'msg-2',
      session_id: 'session-1',
      message: {
        role: 'assistant',
        content: '好的，我来继续处理。',
        timestamp: '2026-07-23T10:00:10Z',
      },
      usage: { prompt_tokens: 60, completion_tokens: 20, total_tokens: 80 },
    });
  }),

  // Knowledge search（后端为 GET /knowledge/search）
  http.get(`${API_BASE}/knowledge/search`, () => {
    return HttpResponse.json(mockKnowledgeResults);
  }),

  // Knowledge ingest（后端为 POST /knowledge/ingest，multipart）
  http.post(`${API_BASE}/knowledge/ingest`, () => {
    return HttpResponse.json({
      success: true,
      job_id: 'mock-ingest-job',
      message: '导入完成',
      files: [{ filename: 'mock-doc.md', status: 'success' }],
    });
  }),

  // Knowledge entries（后端为 GET /knowledge/entries）
  http.get(`${API_BASE}/knowledge/entries`, () => {
    return HttpResponse.json({ entries: [], path: '' });
  }),

  // Config
  http.get(`${API_BASE}/config/status`, () => {
    return HttpResponse.json(mockConfigStatus);
  }),

  http.get(`${API_BASE}/config`, () => {
    return HttpResponse.json({ config: mockTianyanConfig });
  }),

  http.put(`${API_BASE}/config`, async ({ request }) => {
    // Accept and echo back success
    const body = await request.json();
    console.debug('Mock config update:', body);
    return HttpResponse.json({ success: true, message: '配置已保存' });
  }),

  http.get(`${API_BASE}/config/models`, () => {
    return HttpResponse.json(mockModelsResponse);
  }),
];
