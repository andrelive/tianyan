import { http, HttpResponse } from 'msw';
import type {
  Session,
  Skill,
  ListSessionsResponse,
  ChatResponse,
  SkillListResponse,
  ModelsResponse,
  MemoryEntry,
  MemoryListResponse,
  RetrievalTrace,
  RetrievalTracesResponse,
  ApprovalStatusSnapshot,
  SchedulerStatus,
  UsageStatsSummary,
  McpServerEntry,
  McpTestResponse,
  OllamaScanResponse,
  OllamaTestResponse,
  WorkspaceEntry,
  WorkspaceTreeResponse,
  WorkspaceReadResponse,
  WorkspaceDiffResponse,
  WorkspaceApplyPatchResponse,
} from '@/lib/types';
import type { KnowledgeEntryItem } from '@/lib/api-client';

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

// 可变的浏览条目列表：delete handler 会从中移除条目，GET 返回当前状态。
export const mockKnowledgeEntries: KnowledgeEntryItem[] = [
  {
    uri: 'tianyan://knowledge/architecture',
    name: 'architecture',
    is_directory: true,
    has_abstract: false,
    has_overview: false,
    has_detail: false,
  },
  {
    uri: 'tianyan://knowledge/architecture/dual-layer-index',
    name: 'dual-layer-index',
    is_directory: false,
    has_abstract: true,
    has_overview: true,
    has_detail: true,
  },
  {
    uri: 'tianyan://knowledge/rust/ownership',
    name: 'ownership',
    is_directory: false,
    has_abstract: true,
    has_overview: false,
    has_detail: false,
  },
];

const KNOWLEDGE_ENTRIES_BASE: KnowledgeEntryItem[] = [...mockKnowledgeEntries];

/** 记录的知识条目删除调用（{ uri }），测试断言用。 */
export const mockKnowledgeDeleteCalls: { uri: string }[] = [];

/** 恢复知识条目 mock 到初始状态（delete 会就地修改 mockKnowledgeEntries）。 */
export function resetKnowledgeEntryMocks() {
  mockKnowledgeDeleteCalls.length = 0;
  mockKnowledgeEntries.length = 0;
  mockKnowledgeEntries.push(...KNOWLEDGE_ENTRIES_BASE);
}

// ========== Workspace mock ==========

/** 工作区目录树 fixture：key 为相对路径（根为空串）。 */
export const mockWorkspaceTree: Record<string, WorkspaceEntry[]> = {
  '': [
    { name: 'src', type: 'dir', path: 'src' },
    { name: 'Cargo.toml', type: 'file', path: 'Cargo.toml', size: 1234, mtime: 1712345678901 },
    { name: 'main.rs', type: 'file', path: 'main.rs', size: 512, mtime: 1712345678000 },
    { name: 'data.bin', type: 'file', path: 'data.bin', size: 4096, mtime: 1712345678000 },
  ],
  src: [
    { name: 'lib.rs', type: 'file', path: 'src/lib.rs', size: 2048, mtime: 1712345678000 },
    { name: 'utils.rs', type: 'file', path: 'src/utils.rs', size: 1024, mtime: 1712345678000 },
    { name: 'generated', type: 'dir', path: 'src/generated' },
  ],
  'src/generated': [],
};

/** 记录的工作区 tree 调用（{ path, depth }），测试断言懒加载用。 */
export const mockWorkspaceTreeCalls: { path?: string; depth: number }[] = [];

/** 记录的工作区 read 调用（{ path, offset, limit }）。 */
export const mockWorkspaceReadCalls: { path: string; offset?: number; limit: number }[] = [];

/** 记录的工作区 diff 调用（{ path, sessionId, index }）。 */
export const mockWorkspaceDiffCalls: {
  path?: string;
  sessionId?: string;
  index?: number;
}[] = [];

/** 工作区根目录绝对路径 fixture。 */
export const mockWorkspaceRoot = 'C:\\work\\tianyan';

/** hashline 前缀的文件内容 fixture（"N#ID|content"）。 */
export const mockWorkspaceReadContent = [
  '1#3f|fn main() {',
  '2#a1|    println!("hello");',
  '3#7c|}',
].join('\n');

/** 截断文件第一页 fixture（truncated=true，供“加载更多”测试）。 */
export const mockWorkspaceReadPage1 = {
  path: 'C:\\work\\tianyan\\src\\lib.rs',
  content: '1#3f|fn main() {',
  truncated: true,
  total_lines: 3,
  binary: false,
};

/** 截断文件第二页 fixture。 */
export const mockWorkspaceReadPage2 = {
  path: 'C:\\work\\tianyan\\src\\lib.rs',
  content: '2#a1|    println!("hello");\n3#7c|}',
  truncated: false,
  total_lines: 3,
  binary: false,
};

/** 二进制文件读取 fixture。 */
export const mockWorkspaceReadBinary = {
  path: 'C:\\work\\tianyan\\data.bin',
  binary: true,
  size: 4096,
  preview: '\\x00\\x01\\x02 binary preview',
};

/** 单文件 diff fixture（unified 文本带 +/- 行）。 */
export const mockWorkspaceDiff: WorkspaceDiffResponse = {
  path: 'src/lib.rs',
  status: 'modified',
  hunks: [{ old_start: 1, old_len: 3, new_start: 1, new_len: 3 }],
  unified: [
    '--- a/src/lib.rs',
    '+++ b/src/lib.rs',
    '@@ -1,3 +1,3 @@',
    '-fn main() {',
    '+fn main() {',
    '     println!("hello");',
    ' }',
  ].join('\n'),
  old_lines: 3,
  new_lines: 3,
};

/** 整体 diff fixture（不带 path 时返回文件列表）。 */
export const mockWorkspaceDiffList = {
  files: [
    mockWorkspaceDiff,
    { path: 'main.rs', status: 'added', hunks: [], unified: '', old_lines: 0, new_lines: 1 },
  ],
};

/** 记录的工作区 apply-patch 调用（{ patch }），测试断言用。 */
export const mockWorkspaceApplyPatchCalls: { patch: string }[] = [];

/** apply-patch 成功 fixture（POST /workspace/apply-patch）。 */
export const mockWorkspaceApplyPatchResult: WorkspaceApplyPatchResponse = {
  files: [{ path: 'src/lib.rs', hunks_applied: 1, lines_changed: 2 }],
  total_files: 1,
};

/** 恢复工作区 mock 到初始状态。 */
export function resetWorkspaceMocks() {
  mockWorkspaceTreeCalls.length = 0;
  mockWorkspaceReadCalls.length = 0;
  mockWorkspaceDiffCalls.length = 0;
  mockWorkspaceApplyPatchCalls.length = 0;
}

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

// ========== Settings detail mocks (MCP / Ollama / Soul / test-connection) ==========

export const mockMcpServers: McpServerEntry[] = [
  {
    name: 'filesystem-server',
    command: 'npx',
    args: ['-y', '@anthropic-ai/mcp-serve'],
    env: { GITHUB_TOKEN: 'ghp_xxx' },
    enabled: true,
    description: '文件系统操作',
  },
  {
    name: 'code-search-server',
    command: 'node',
    args: ['search.js'],
    enabled: false,
  },
];

export const mockMcpTestResult: McpTestResponse = { success: true, tools: 12 };

export const mockOllamaScanResult: OllamaScanResponse = {
  success: true,
  models: [
    { name: 'llama3:8b', size: '4700000000', capabilities: ['chat'] },
    { name: 'nomic-embed-text', size: '274000000', capabilities: ['text-embedding'] },
  ],
};

export const mockOllamaTestResult: OllamaTestResponse = { success: true, version: '0.3.6' };

export const mockSoulContent = { content: '你是天演，一个本地智能代理助手。' };

export const mockDefaultSoulContent = { content: '（默认人格设定）' };

export const mockTestConnectionResult = { success: true, message: '连接成功' };

// ========== Memory mock ==========

export const mockMemories: MemoryEntry[] = [
  {
    uri: 'tianyan://memory/rules',
    is_directory: true,
    name: 'rules',
    metadata: {
      uri: { uri: 'tianyan://memory/rules', namespace: 'memory', path: ['rules'] },
      is_directory: true,
      content_type: 'directory',
      category: null,
      source: 'memory',
      original_name: null,
      file_size: null,
      importance: 0,
      tags: [],
      created_at: '2026-07-20T10:00:00Z',
      updated_at: '2026-07-23T09:00:00Z',
      custom: {},
    },
    abstract: null,
    overview: null,
    detail: null,
  },
  {
    uri: 'tianyan://memory/preferences/response_style',
    is_directory: false,
    name: 'response_style',
    metadata: {
      uri: {
        uri: 'tianyan://memory/preferences/response_style',
        namespace: 'memory',
        path: ['preferences', 'response_style'],
      },
      is_directory: false,
      content_type: 'text/plain',
      category: 'preference',
      source: 'MemoryTask',
      original_name: 'response_style.md',
      file_size: 512,
      importance: 0.9,
      tags: ['偏好', '风格'],
      created_at: '2026-07-21T08:30:00Z',
      updated_at: '2026-07-23T09:00:00Z',
      custom: {},
    },
    abstract: '用户偏好简洁直接的回复风格，代码示例优先使用 Rust。',
    overview:
      '用户希望回复简洁直接，避免冗长铺垫；技术问答中优先给出 Rust 代码示例，并附一行说明。',
    detail:
      '用户偏好简洁直接的回复风格，代码示例优先使用 Rust。\n\n技术问答时：\n- 先给结论，再给解释\n- 代码示例默认使用 Rust 编写\n- 保留关键错误处理分支',
  },
  {
    uri: 'tianyan://memory/facts/user_name',
    is_directory: false,
    name: 'user_name',
    metadata: {
      uri: {
        uri: 'tianyan://memory/facts/user_name',
        namespace: 'memory',
        path: ['facts', 'user_name'],
      },
      is_directory: false,
      content_type: 'text/plain',
      category: 'fact',
      source: 'MemoryTask',
      original_name: 'user_name.md',
      file_size: 128,
      importance: 0.6,
      tags: ['事实', '用户'],
      created_at: '2026-07-22T11:00:00Z',
      updated_at: '2026-07-22T11:00:00Z',
      custom: {},
    },
    abstract: '用户昵称为「小天」。',
    overview: null,
    detail: null,
  },
];

// ========== Retrieval trace mock ==========

export const mockRetrievalTraces: RetrievalTrace[] = [
  {
    query: 'VFS 双层摘要索引架构',
    timestamp: '2026-07-23T09:00:00Z',
    total_time_ms: 1280,
    total_tokens: 2430,
    steps: [
      {
        step_type: 'intent_analysis',
        target_uri: {
          uri: 'tianyan://knowledge/architecture/dual-layer-index',
          namespace: 'knowledge',
          path: ['architecture', 'dual-layer-index'],
        },
        score: null,
        tokens_used: 320,
        timestamp: '2026-07-23T09:00:00.100Z',
      },
      {
        step_type: 'l0_search',
        target_uri: {
          uri: 'tianyan://knowledge/architecture/dual-layer-index',
          namespace: 'knowledge',
          path: ['architecture', 'dual-layer-index'],
        },
        score: 0.87,
        tokens_used: 150,
        timestamp: '2026-07-23T09:00:00.300Z',
      },
      {
        step_type: 'l1_search',
        target_uri: {
          uri: 'tianyan://knowledge/architecture/vfs',
          namespace: 'knowledge',
          path: ['architecture', 'vfs'],
        },
        score: 0.72,
        tokens_used: 260,
        timestamp: '2026-07-23T09:00:00.550Z',
      },
      {
        step_type: 'content_load',
        target_uri: {
          uri: 'tianyan://knowledge/architecture/dual-layer-index',
          namespace: 'knowledge',
          path: ['architecture', 'dual-layer-index'],
        },
        score: null,
        tokens_used: 1100,
        timestamp: '2026-07-23T09:00:00.800Z',
      },
      {
        step_type: 'aggregation',
        target_uri: {
          uri: 'tianyan://knowledge/architecture/vfs',
          namespace: 'knowledge',
          path: ['architecture', 'vfs'],
        },
        score: null,
        tokens_used: 600,
        timestamp: '2026-07-23T09:00:01.200Z',
      },
    ],
    results: [
      {
        uri: 'tianyan://knowledge/architecture/dual-layer-index',
        namespace: 'knowledge',
        path: ['architecture', 'dual-layer-index'],
      },
      {
        uri: 'tianyan://knowledge/architecture/vfs',
        namespace: 'knowledge',
        path: ['architecture', 'vfs'],
      },
    ],
  },
  {
    query: 'Rust 内存安全模式',
    timestamp: '2026-07-23T09:05:00Z',
    total_time_ms: 860,
    total_tokens: 1540,
    steps: [
      {
        step_type: 'intent_analysis',
        target_uri: {
          uri: 'tianyan://knowledge/rust/ownership',
          namespace: 'knowledge',
          path: ['rust', 'ownership'],
        },
        score: null,
        tokens_used: 240,
        timestamp: '2026-07-23T09:05:00.100Z',
      },
      {
        step_type: 'l1_search',
        target_uri: {
          uri: 'tianyan://knowledge/rust/ownership',
          namespace: 'knowledge',
          path: ['rust', 'ownership'],
        },
        score: 0.64,
        tokens_used: 180,
        timestamp: '2026-07-23T09:05:00.400Z',
      },
      {
        step_type: 'content_load',
        target_uri: {
          uri: 'tianyan://knowledge/rust/ownership',
          namespace: 'knowledge',
          path: ['rust', 'ownership'],
        },
        score: null,
        tokens_used: 1120,
        timestamp: '2026-07-23T09:05:00.700Z',
      },
    ],
    results: [
      {
        uri: 'tianyan://knowledge/rust/ownership',
        namespace: 'knowledge',
        path: ['rust', 'ownership'],
      },
    ],
  },
  {
    query: 'SQLite 单连接限制',
    timestamp: '2026-07-23T09:10:00Z',
    total_time_ms: 540,
    total_tokens: 980,
    steps: [
      {
        step_type: 'intent_analysis',
        target_uri: {
          uri: 'tianyan://knowledge/storage/sqlite-backend',
          namespace: 'knowledge',
          path: ['storage', 'sqlite-backend'],
        },
        score: null,
        tokens_used: 210,
        timestamp: '2026-07-23T09:10:00.100Z',
      },
      {
        step_type: 'l0_search',
        target_uri: {
          uri: 'tianyan://knowledge/storage/sqlite-backend',
          namespace: 'knowledge',
          path: ['storage', 'sqlite-backend'],
        },
        score: 0.91,
        tokens_used: 140,
        timestamp: '2026-07-23T09:10:00.300Z',
      },
      {
        step_type: 'aggregation',
        target_uri: {
          uri: 'tianyan://knowledge/storage/sqlite-backend',
          namespace: 'knowledge',
          path: ['storage', 'sqlite-backend'],
        },
        score: null,
        tokens_used: 630,
        timestamp: '2026-07-23T09:10:00.500Z',
      },
    ],
    results: [
      {
        uri: 'tianyan://knowledge/storage/sqlite-backend',
        namespace: 'knowledge',
        path: ['storage', 'sqlite-backend'],
      },
    ],
  },
];

// ========== Approval mock ==========

export const mockApprovalStatus: ApprovalStatusSnapshot = {
  config: {
    default_timeout_secs: 300,
    enable_auto_approval: true,
    persist_records: true,
    max_pending_approvals: 100,
    unattended_mode: false,
    wait_for_approval: true,
  },
  pending_approvals: [
    {
      request_id: 'req-1',
      session_id: 's1',
      action_description: '写入文件 /home/user/data.txt',
      action: {},
      risk_level: 'High',
      requested_at: '2026-08-05T10:00:00Z',
      timeout_secs: 300,
    },
    {
      request_id: 'req-2',
      session_id: 's1',
      action_description: '执行命令 rm -rf /tmp/cache',
      action: {},
      risk_level: 'Critical',
      requested_at: '2026-08-05T10:01:00Z',
      timeout_secs: 300,
    },
  ],
  pending_confirmations: [],
  recent_records: [
    {
      request: {
        request_id: 'req-0',
        session_id: 's1',
        action_description: '写入文件 /home/user/old.txt',
        action: {},
        risk_level: 'High',
        requested_at: '2026-08-05T09:55:00Z',
        timeout_secs: 300,
      },
      response: {
        request_id: 'req-0',
        decision: 'Approve',
        reason: null,
        responded_at: '2026-08-05T09:56:00Z',
        approved_by: 'user',
      },
      execution_result: true,
    },
  ],
  confirmed_action_count: 3,
};

// ========== Insights mock ==========

export const mockSchedulerStatus: SchedulerStatus = {
  running: true,
  tasks: [
    {
      id: 'summary_generation',
      name: '摘要生成',
      priority: 'Medium',
      cron_expression: '0 */5 * * * *',
      run_count: 12,
      last_run_ago_secs: 183,
    },
    {
      id: 'memory_extraction',
      name: '记忆提取',
      priority: 'Medium',
      cron_expression: '0 */10 * * * *',
      run_count: 6,
      last_run_ago_secs: null,
    },
    {
      id: 'garbage_collection',
      name: '垃圾回收',
      priority: 'Low',
      cron_expression: '0 0 */6 * * *',
      run_count: 2,
      last_run_ago_secs: 3600,
    },
  ],
};

export const mockUsageStats: UsageStatsSummary = {
  total_skills_tracked: 7,
  total_skill_calls: 124,
  total_docs_tracked: 45,
  total_searches: 230,
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
    const lastContent = msgs.length > 0 ? (msgs[msgs.length - 1].content ?? '') : '';

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
    return HttpResponse.json({ entries: mockKnowledgeEntries, path: '' });
  }),

  // Knowledge entry delete（后端为 POST /knowledge/entries/delete，body: { uri }）
  http.post(`${API_BASE}/knowledge/entries/delete`, async ({ request }) => {
    const body = (await request.json()) as { uri?: string };
    const uri = body.uri ?? '';
    mockKnowledgeDeleteCalls.push({ uri });
    const index = mockKnowledgeEntries.findIndex((entry) => entry.uri === uri);
    if (index >= 0) {
      mockKnowledgeEntries.splice(index, 1);
    }
    return HttpResponse.json({ uri, success: true });
  }),

  // Memory（后端为 GET /memory）
  http.get(`${API_BASE}/memory`, () => {
    const response: MemoryListResponse = {
      memories: mockMemories,
      total: mockMemories.length,
    };
    return HttpResponse.json(response);
  }),

  // Retrieval traces（后端为 GET /retrieval/traces）
  http.get(`${API_BASE}/retrieval/traces`, () => {
    const response: RetrievalTracesResponse = {
      traces: mockRetrievalTraces,
      total: mockRetrievalTraces.length,
    };
    return HttpResponse.json(response);
  }),

  // Approval（后端为 GET /approval/status）
  http.get(`${API_BASE}/approval/status`, () => {
    return HttpResponse.json(mockApprovalStatus);
  }),

  // Approval respond（后端为 POST /approval/respond）
  http.post(`${API_BASE}/approval/respond`, () => {
    return HttpResponse.json({ ok: true });
  }),

  // Scheduler status（后端为 GET /scheduler/status）
  http.get(`${API_BASE}/scheduler/status`, () => {
    return HttpResponse.json(mockSchedulerStatus);
  }),

  // Usage stats（后端为 GET /stats）
  http.get(`${API_BASE}/stats`, () => {
    return HttpResponse.json(mockUsageStats);
  }),

  // Workspace tree（后端为 GET /workspace/tree?path=&depth=）
  http.get(`${API_BASE}/workspace/tree`, ({ request }) => {
    const url = new URL(request.url);
    const path = url.searchParams.get('path') ?? '';
    const depth = Number(url.searchParams.get('depth') ?? 1);
    mockWorkspaceTreeCalls.push({ path: path || undefined, depth });
    const entries = mockWorkspaceTree[path] ?? [];
    const response: WorkspaceTreeResponse = { root: mockWorkspaceRoot, path, entries };
    return HttpResponse.json(response);
  }),

  // Workspace read（后端为 GET /workspace/read?path=&offset=&limit=）
  http.get(`${API_BASE}/workspace/read`, ({ request }) => {
    const url = new URL(request.url);
    const path = url.searchParams.get('path') ?? '';
    const offset = url.searchParams.get('offset');
    const limit = Number(url.searchParams.get('limit') ?? 2000);
    mockWorkspaceReadCalls.push({
      path,
      offset: offset === null ? undefined : Number(offset),
      limit,
    });
    if (path.endsWith('.bin')) {
      return HttpResponse.json(mockWorkspaceReadBinary);
    }
    // src/lib.rs 走分页 fixture；其余文件返回单页内容
    if (path === 'src/lib.rs' && offset !== null) {
      return HttpResponse.json(mockWorkspaceReadPage2);
    }
    if (path === 'src/lib.rs') {
      return HttpResponse.json(mockWorkspaceReadPage1);
    }
    const response: WorkspaceReadResponse = {
      path: `${mockWorkspaceRoot}\\${path}`,
      content: mockWorkspaceReadContent,
      truncated: false,
      total_lines: 3,
      binary: false,
    };
    return HttpResponse.json(response);
  }),

  // Workspace diff（后端为 GET /workspace/diff?path=&base=snapshot&session_id=&index=）
  // 与后端契约一致：base=snapshot 缺少 session_id 或 index 时返回 400。
  http.get(`${API_BASE}/workspace/diff`, ({ request }) => {
    const url = new URL(request.url);
    const path = url.searchParams.get('path');
    const sessionId = url.searchParams.get('session_id') ?? undefined;
    const indexRaw = url.searchParams.get('index');
    mockWorkspaceDiffCalls.push({
      path: path ?? undefined,
      sessionId,
      index: indexRaw === null ? undefined : Number(indexRaw),
    });
    if (sessionId === undefined || indexRaw === null) {
      return new HttpResponse(null, { status: 400 });
    }
    if (!path) {
      return HttpResponse.json(mockWorkspaceDiffList);
    }
    return HttpResponse.json(mockWorkspaceDiff);
  }),

  // Workspace apply-patch（后端为 POST /workspace/apply-patch，body: { patch }）
  // 记录调用并返回成功 fixture；测试失败场景用 server.use 覆盖。
  http.post(`${API_BASE}/workspace/apply-patch`, async ({ request }) => {
    const body = (await request.json()) as { patch?: string };
    mockWorkspaceApplyPatchCalls.push({ patch: body.patch ?? '' });
    return HttpResponse.json(mockWorkspaceApplyPatchResult);
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

  // MCP servers（后端为 GET/POST /config/mcp/servers）
  http.get(`${API_BASE}/config/mcp/servers`, () => {
    return HttpResponse.json(mockMcpServers);
  }),

  http.post(`${API_BASE}/config/mcp/servers`, () => {
    return HttpResponse.json({ success: true });
  }),

  http.put(`${API_BASE}/config/mcp/servers/:name`, () => {
    return HttpResponse.json({ success: true });
  }),

  http.delete(`${API_BASE}/config/mcp/servers/:name`, () => {
    return HttpResponse.json({ success: true });
  }),

  http.post(`${API_BASE}/config/mcp/servers/:name/test`, () => {
    return HttpResponse.json(mockMcpTestResult);
  }),

  // Ollama（后端为 POST /config/ollama/scan、/config/ollama/test）
  http.post(`${API_BASE}/config/ollama/scan`, () => {
    return HttpResponse.json(mockOllamaScanResult);
  }),

  http.post(`${API_BASE}/config/ollama/test`, () => {
    return HttpResponse.json(mockOllamaTestResult);
  }),

  // Soul（后端为 GET/PUT /config/soul，GET /config/soul/default）
  http.get(`${API_BASE}/config/soul`, () => {
    return HttpResponse.json(mockSoulContent);
  }),

  http.put(`${API_BASE}/config/soul`, () => {
    return HttpResponse.json({ success: true, message: '已保存' });
  }),

  http.get(`${API_BASE}/config/soul/default`, () => {
    return HttpResponse.json(mockDefaultSoulContent);
  }),

  // Test connection（后端为 POST /config/test-connection）
  http.post(`${API_BASE}/config/test-connection`, () => {
    return HttpResponse.json(mockTestConnectionResult);
  }),

  http.get(`${API_BASE}/config/models`, () => {
    return HttpResponse.json(mockModelsResponse);
  }),
];
