// ========== Chat Types ==========

export type MessageRole = 'system' | 'user' | 'assistant';

export type StreamChunkType =
  'answer' | 'thought' | 'tool_call' | 'observation' | 'error' | 'message';

export interface ChatMessage {
  id?: string;
  role: MessageRole;
  content: string;
  /** 思考过程文本（模型 reasoning；正文在 content，前端折叠展示） */
  thinking?: string;
  /** 消息时间线段（服务端权威，ADR-019）：历史加载与流式边界事件均携带，
   * 按 StructuredMessage.parts 真实到达顺序渲染思考/正文/工具调用 */
  segments?: MessageSegment[];
  /** 图片 data URL 列表（仅用户消息），如 data:image/png;base64,... */
  images?: string[];
  timestamp?: string;
  skill_calls?: SkillCallInfo[];
  /** 工具调用卡片（A2 展示契约；流式 chunk 累积 / 历史消息由后端按
   * tool_call_id 合并调用与结果，result 完整内容不截断） */
  tool_calls?: ToolCallWithResult[];
  chunk_type?: StreamChunkType;
  /** 流结束事件 finish_reason === 'length'：输出达到 token 上限被截断（前端本地标记） */
  truncated_by_length?: boolean;
  /** 流式中断（finish=interrupted：网络/服务中断保留部分输出；区别于 token 上限截断） */
  interrupted?: boolean;
  /** 本条消息的 token 用量（历史加载/完成 chunk 携带；前端按会话独立计算上下文占用） */
  usage?: TokenUsage | null;
}

export interface ChatRequest {
  session_id?: string | null;
  /** 本轮输入消息（单条）——历史由服务端会话持久化提供，请求不携带全量历史 */
  message: ChatMessage;
  stream: boolean;
  temperature: number;
  max_tokens: number;
  model?: string | null;
  /** 本会话思考强度档位（会话时选择；值为当前模型声明的档位，如 "high"/"max"，"off" 关闭） */
  thinking?: string;
  /** 新会话绑定的工作目录（仅新建会话时生效） */
  working_directory?: string | null;
}

export interface ChatResponse {
  id: string;
  session_id: string;
  message: ChatMessage;
  usage: TokenUsage;
}

export interface TokenUsage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
  /** 缓存命中（读取）token 数（提供商返回缓存明细时才有意义） */
  cache_read?: number;
  /** 缓存写入 token 数 */
  cache_write?: number;
}

/** 流式事件携带的 token 用量（完成 chunk 附带；上下文占用 / 缓存命中展示） */
export interface StreamUsage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
  cache_read: number;
  cache_write: number;
  /** 当前聊天模型上下文窗口（token） */
  context_window: number;
}

export interface ChatStreamEvent {
  id: string;
  session_id: string;
  /** 消息边界载荷（chunk_type=message）：流开始携带用户消息、流结束携带
   * assistant 消息的完整结构（与历史加载 ChatMessage 同构）——本地消息
   * id/内容直接来自服务端统一结构，不做两套形态的补丁同步 */
  message?: ChatMessage | null;
  delta: string;
  /** 思考过程增量（Thought chunk 携带；正文在 delta，分开渲染） */
  thinking?: string | null;
  finish_reason?: string | null;
  skill_calls?: SkillCallInfo[] | null;
  chunk_type: StreamChunkType;
  /** A2：结构化工具调用事件（tool_call chunk 携带） */
  tool_call?: ToolCallEvent | null;
  /** 工具执行结果事件（observation chunk 携带；耗时/成败结构化下发） */
  tool_result?: ToolResultEvent | null;
  /** 本轮 token 用量（完成 chunk 携带；上下文占用 / 缓存命中展示用） */
  usage?: StreamUsage | null;
}

export interface SkillCallInfo {
  skill_id: string;
  skill_name: string;
  success: boolean;
  execution_time_ms: number;
  error?: string | null;
}

/** 工具调用事件（A2 展示契约）：工具名 + 参数 + 展示意图 */
export interface ToolCallEvent {
  /** 工具调用 ID（关联 ToolResultEvent.tool_call_id；流式卡片据此挂结果） */
  id?: string;
  name: string;
  arguments: string;
  /** generic/read/write/terminal/diff/search/web/skill/knowledge/delegate/code */
  presentation: string;
  /** 执行耗时（毫秒；observation 事件到达后填充；历史消息由后端透传） */
  duration_ms?: number;
  /** 执行是否成功（observation 事件到达后填充；缺失 = 未知） */
  success?: boolean;
  /** 执行失败原因（成功/未知时为 null/undefined） */
  error?: string | null;
}

/** 工具执行结果事件（observation chunk 携带） */
export interface ToolResultEvent {
  tool_call_id: string;
  /** 执行耗时（毫秒） */
  duration_ms: number;
  /** 是否成功 */
  success: boolean;
  /** 失败原因（成功时为 null/undefined） */
  error?: string | null;
  /** 工具执行结果内容（observation delta；前端挂到对应卡片 result） */
  content?: string | null;
}

/** 工具调用卡片（历史消息：调用信息与对应执行结果合并渲染） */
export interface ToolCallWithResult extends ToolCallEvent {
  /** 工具调用 ID（关联工具结果） */
  id?: string;
  /** 对应执行结果（完整内容不截断；无结果时为 null/undefined） */
  result?: string | null;
  /** 执行耗时（毫秒；Part::ToolResult.time 差值，历史缺失时为 undefined） */
  duration_ms?: number;
  /** 执行失败原因（Part::ToolResult.error 透传；None = 成功） */
  error?: string | null;
}

/**
 * 消息时间线段（流式事件按到达顺序累积，前端据此按序轮番渲染
 * 思考/文本/工具调用——不再把同类内容挤在一起）。
 */
export type MessageSegment =
  | { type: 'thinking'; text: string }
  | { type: 'text'; text: string }
  | { type: 'tool'; tool_call: ToolCallEvent };

/** 工具展示意图 → 卡片标签（A2） */
export const TOOL_PRESENTATION_LABELS: Record<string, string> = {
  generic: '工具',
  read: '读取文件',
  write: '写入文件',
  terminal: '执行命令',
  diff: '编辑文件',
  search: '搜索',
  web: '浏览网页',
  skill: '调用技能',
  knowledge: '导入知识',
  delegate: '委托子代理',
  code: '代码分析',
};

// ========== Session Types ==========

export interface Session {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  message_count: number;
  /** 会话绑定的工作目录（工作区归属；None = 全局配置兜底） */
  working_directory?: string | null;
}

export interface ListSessionsResponse {
  sessions: Session[];
  total: number;
}

export interface SessionMessagesResponse {
  session_id: string;
  messages: ChatMessage[];
}

// ========== Skill Types ==========

export interface SkillParameter {
  name: string;
  type: string;
  description: string;
  required: boolean;
  default_value?: unknown;
}

export interface Skill {
  id: string;
  name: string;
  description: string;
  parameters?: SkillParameter[];
  category: string;
  version: string;
  enabled: boolean;
  /** 创建时间（RFC3339；内置技能无） */
  created_at?: string;
  /** 更新时间（RFC3339；内置技能无） */
  updated_at?: string;
}

/** 技能详情（完整内容 + 时间元数据） */
export interface SkillDetail {
  id: string;
  name: string;
  description: string;
  category: string;
  version: string;
  enabled: boolean;
  /** 完整内容（markdown；仅 VFS 存储的技能有） */
  content?: string;
  created_at?: string;
  updated_at?: string;
}

// ========== Tool Types ==========

/** 系统工具信息（工具目录：与 LLM tools 列表同源） */
export interface ToolInfo {
  name: string;
  description: string;
  /** 参数 JSON Schema */
  parameters: Record<string, unknown>;
}

export interface ListToolsResponse {
  tools: ToolInfo[];
  total: number;
}

export interface SkillExecutionStatus {
  job_id?: string;
  skill_id?: string;
  status: string;
  progress?: number;
  result?: unknown;
  error?: string;
}

export interface SkillListResponse {
  skills: Skill[];
  total?: number;
}

export interface SkillStatsEntry {
  skill_id: string;
  total_calls: number;
  success_calls: number;
  success_rate: number;
  avg_time_ms: number;
  last_called_at: string;
}

export interface SkillsStatsResponse {
  skills: SkillStatsEntry[];
  reviews: SkillReviewEntry[];
  total_calls: number;
  total_success: number;
  success_rate: number;
}

export interface SkillReviewEntry {
  skill_id: string;
  score: number;
  verdict: string;
  user_feedback: string;
  reason: string;
  ts: number;
}

// ── 子智能体角色（ADR-016）────────────────────────────────────────────

export interface RoleUsageSummary {
  calls: number;
  success: number;
  failed: number;
  success_rate: number;
  last_used: number;
}

export interface RoleSummary {
  name: string;
  source: 'builtin' | 'user' | 'learned';
  status: 'active' | 'experimental';
  version: number;
  lineage?: string | null;
  /** 职责一句话 */
  purpose: string;
  /** 工具白名单数量（null = 不限制） */
  tool_count?: number | null;
  model?: string | null;
  max_turns?: number | null;
  /** 使用统计（无记录时为 null） */
  usage?: RoleUsageSummary | null;
}

export interface RoleDetail extends RoleSummary {
  /** 完整系统提示 */
  system_prompt?: string | null;
  /** 工具白名单（null = 不限制） */
  tools?: string[] | null;
}

export interface ListRolesResponse {
  roles: RoleSummary[];
}

export interface DelegationRecord {
  ts: number;
  role: string;
  task: string;
  success: boolean;
  mode: string;
  duration_ms: number;
  tokens: number;
}

export interface RoleUsageEntry {
  name: string;
  calls: number;
  success: number;
  failed: number;
  success_rate: number;
  last_used: number;
}

export interface RolesStatsResponse {
  total_calls: number;
  total_success: number;
  success_rate: number;
  by_role: RoleUsageEntry[];
  by_task_type: [string, number][];
  recent: DelegationRecord[];
}

export interface RoleActionResponse {
  name: string;
  status: string;
}

export interface SkillExecuteResponse {
  success: boolean;
  job_id: string;
  skill_id: string;
  message: string;
  result?: unknown;
  error?: string;
}

// ========== Knowledge Types ==========

export interface KnowledgeSearchResult {
  id: string;
  content: string;
  score: number;
  source: string;
  metadata?: {
    title?: string;
    url?: string;
    timestamp?: string;
    tags?: string[];
  };
}

export interface KnowledgeSearchResponse {
  query: string;
  results: KnowledgeSearchResult[];
  total: number;
  limit: number;
  offset: number;
}

/** 搜索建议响应（GET /knowledge/search/suggestions）。 */
export interface SearchSuggestionsResponse {
  query: string;
  suggestions: string[];
}

// ========== Memory Types ==========

export interface MemoryUri {
  uri: string;
  namespace: string;
  path: string[];
}

export interface MemoryEntryMetadata {
  uri: MemoryUri;
  is_directory: boolean;
  content_type: string;
  category: string | null;
  source: string;
  original_name: string | null;
  file_size: number | null;
  importance: number;
  tags: string[];
  created_at: string;
  updated_at: string;
  custom: Record<string, unknown>;
}

export interface MemoryEntry {
  uri: string;
  is_directory: boolean;
  name: string | null;
  metadata: MemoryEntryMetadata;
  abstract: string | null;
  overview: string | null;
  detail: string | null;
}

export interface MemoryListResponse {
  memories: MemoryEntry[];
  total: number;
}

// ========== Retrieval Trace Types (matches backend retrieval trace DTO) ==========

export type RetrievalStepType =
  'intent_analysis' | 'l0_search' | 'l1_search' | 'content_load' | 'aggregation';

export interface RetrievalStep {
  step_type: RetrievalStepType;
  target_uri: MemoryUri;
  score: number | null;
  tokens_used: number;
  timestamp: string;
}

export interface RetrievalTrace {
  query: string;
  steps: RetrievalStep[];
  results: MemoryUri[];
  total_tokens: number;
  total_time_ms: number;
  timestamp: string;
}

export interface RetrievalTracesResponse {
  traces: RetrievalTrace[];
  total: number;
}

// ========== Approval Types (matches backend approval DTO) ==========

/** 审批决策（API 语义：snake_case 字符串）。 */
export type ApprovalDecision = 'approve' | 'deny' | 'request_more_info';

/** 待处理审批请求。 */
export interface ApprovalRequest {
  request_id: string;
  session_id: string;
  action_description: string;
  action: unknown;
  risk_level: string;
  requested_at: string;
  timeout_secs: number;
}

/** 审批响应记录（审计）。 */
export interface ApprovalResponse {
  request_id: string;
  decision: string;
  reason: string | null;
  responded_at: string;
  approved_by: string;
  /** 用户审批时编辑后的命令（纠正/改写场景；仅审计展示，不影响实际执行）。 */
  edited_command?: string | null;
}

/** 审批审计记录。 */
export interface ApprovalRecord {
  request: ApprovalRequest;
  response: ApprovalResponse | null;
  execution_result: boolean | null;
  /** 用户审批时编辑后的命令（纠正/改写场景；仅审计展示，不影响实际执行）。 */
  edited_command?: string | null;
}

/** 审批工作流配置。 */
export interface ApprovalWorkflowConfig {
  default_timeout_secs: number;
  enable_auto_approval: boolean;
  persist_records: boolean;
  max_pending_approvals: number;
  unattended_mode: boolean;
  wait_for_approval: boolean;
  /** "总是询问"命令列表：命中命令强制走人工审批（不被自动放行）。 */
  prompt_commands?: string[];
}

/** 审批状态快照（GET /approval/status）。 */
export interface ApprovalStatusSnapshot {
  config: ApprovalWorkflowConfig;
  pending_approvals: ApprovalRequest[];
  pending_confirmations: string[];
  recent_records: ApprovalRecord[];
  confirmed_action_count: number;
}

/** 知识库导入响应（POST /knowledge/ingest，multipart）。 */
export interface IngestFileResult {
  filename: string;
  /** 后端契约：completed | failed。 */
  status: string;
  /** 失败原因（status = failed 时存在）。 */
  error?: string;
  document_id?: string;
}

export interface IngestResponse {
  success: boolean;
  job_id: string;
  message: string;
  files: IngestFileResult[];
}

// ========== Background Task Types (matches backend background_task DTO) ==========

/** 后台任务状态。 */
export type TaskStatus = 'pending' | 'running' | 'completed' | 'failed' | 'cancelled';

/** 后台任务（GET /tasks 返回 `Vec<BackgroundTask>`）。 */
export interface BackgroundTask {
  id: string;
  description: string;
  status: TaskStatus;
  /** 归属会话 id（未归属时为 null）。 */
  parent_session_id: string | null;
  result: string | null;
  error: string | null;
  /** 创建时间（epoch 毫秒）。 */
  created_at: number;
  /** 完成时间（epoch 毫秒，未完成时为 null）。 */
  completed_at: number | null;
  seq: number;
}

/** 取消后台任务响应（POST /tasks/{id}/cancel，404 任务不存在）。 */
export interface CancelTaskResponse {
  task_id: string;
  status: TaskStatus;
}

/** 手动压缩会话响应（POST /sessions/{id}/compress）。 */
export interface CompressSessionResponse {
  /** true=已执行压缩；false=无需压缩。 */
  compressed: boolean;
}

// ========== Insights Types (matches backend scheduler/stats DTO) ==========

/** 定时任务状态（GET /scheduler/status）。 */
export interface SchedulerTaskStatus {
  id: string;
  name: string;
  priority: string;
  cron_expression: string;
  run_count: number;
  /** 距上次执行秒数（从未执行时为 null）。 */
  last_run_ago_secs: number | null;
}

export interface SchedulerStatus {
  running: boolean;
  tasks: SchedulerTaskStatus[];
}

/** 使用统计摘要（GET /stats）。 */
export interface UsageStatsSummary {
  /** 真技能（call_skill 调用）去重数。 */
  total_skills_tracked: number;
  /** 真技能调用总次数。 */
  total_skill_calls: number;
  /** 普通工具（read_file 等）去重数。 */
  total_tools_tracked: number;
  /** 普通工具调用总次数。 */
  total_tool_calls: number;
  total_docs_tracked: number;
  total_searches: number;
}

// ========== Config Types (aligned with backend TianyanConfig) ==========

export interface ConfigStatus {
  configured: boolean;
}

// ── Model Capability (matches backend ModelCapability enum) ──

export const MODEL_CAPABILITIES = [
  'chat',
  'vision',
  'text-embedding',
  'multimodal-embedding',
] as const;

export type ModelCapability = (typeof MODEL_CAPABILITIES)[number];

/** 能力 → 中文标签（全前端唯一词汇表；曾散落 ModelsTab/ConfirmStep/ModelStep 三处）。 */
export const MODEL_CAPABILITY_LABELS: Record<ModelCapability, string> = {
  chat: '对话 (Chat)',
  vision: '视觉 (Vision)',
  'text-embedding': '文本嵌入',
  'multimodal-embedding': '多模态嵌入',
};

// ── Model Entry (matches backend ModelEntry) ──

export interface ProviderModelEntry {
  name: string;
  capabilities: ModelCapability[];
  /** 该模型支持的思考强度档位值（每个模型自己声明的，如 ["low","high","max"]；缺省查后端内置模型表） */
  reasoning_efforts?: string[];
  /** 上下文窗口长度（token）。未配置时由后端内置模型表自动匹配。 */
  context_length?: number;
  /** 最大输出 token 数。未配置时走内置默认。 */
  max_output_tokens?: number;
  /** 嵌入模型最大输入 token 数。未配置时走内置默认。 */
  max_input_tokens?: number;
}

// ── Provider Config (matches backend ProviderConfig) ──

export interface ProviderConfigState {
  name: string;
  endpoint: string;
  api_key: string;
  models: ProviderModelEntry[];
  timeout: number;
  enabled: boolean;
  is_local: boolean;
  headers: Record<string, string>;
}

// ── Model Reference & Preferences (matches backend ModelRef / ModelPreferences) ──

export interface ModelRef {
  provider: string;
  model: string;
}

/** 后端解析后的模型生效规格（显式字段 > 内置模型表 > 默认），只读展示用。 */
export interface ResolvedModelSpec {
  /** 上下文窗口长度（token）。 */
  context_length: number;
  /** 最大输出 token 数。 */
  max_output_tokens: number;
  /** 嵌入模型最大输入 token 数。 */
  max_input_tokens: number;
}

export interface ModelPreferencesState {
  chat?: ModelRef | null;
  embedding?: ModelRef | null;
  vision?: ModelRef | null;
}

// ── Top-level ConfigState (internal form model, flat-ish for ease of editing) ──
// This is the form model used by SettingsPanel. It is converted to/from
// the backend's nested TianyanConfig via config-transform.ts.

export interface ConfigState {
  // -- Model config (replaces old model_services) --
  providers: ProviderConfigState[];
  preferences: ModelPreferencesState;
  /** 后端解析出的模型生效规格，key = "{provider}/{model}"。旧后端无该字段时为空对象。 */
  resolvedSpecs: Record<string, ResolvedModelSpec>;
  /** 后端解析出的内置目录命中（advisory），key = "{provider}/{model}"。旧后端无该字段时为空对象。 */
  modelCatalog: Record<string, ModelCatalogInfo>;

  // -- Agent config (agent.*) --
  default_top_k: number;
  max_turns: number;
  learned_rules_top_k: number;
  working_directory: string;
  /** 后台自审（后端字段透传） */
  background_self_review: boolean;

  // -- Storage config (storage.*) --
  data_dir: string;
  /** 存储后端类型（后端字段透传：sqlite 等） */
  storage_backend: string;
  /** SQLite 路径（null = 默认位置；后端字段透传） */
  sqlite_path: string | null;
  collection_name: string;
  vector_dimension: number;
  max_storage_size: number;
  auto_cleanup: boolean;
  cleanup_days: number;

  // -- Logging config (logging.*) --
  log_level: string;
  log_format: string;
  log_max_file_size: number;
  log_max_files: number;
  log_include_timestamp: boolean;
  log_include_location: boolean;

  // -- Security config (security.*) --
  security_enabled: boolean;
  confirm_commands: boolean;
  audit_logging: boolean;
  allow_all_operations: boolean;
  /** 安全模式（strict 等；后端字段透传） */
  safety_mode: string;
  /** 回收站目录（后端字段透传；空串 = 后端默认） */
  trash_directory: string;
  max_file_size: number;
  allowed_directories: string;
  blocked_directories: string;
  allowed_commands: string;
  blocked_commands: string;

  // -- Clipboard config (clipboard.*；复制即记忆，默认关闭 opt-in) --
  clipboard_enabled: boolean;
  clipboard_auto_capture: boolean;
  clipboard_prompt_confirm: boolean;

  // -- Memory config (memory.*) --
  max_session_memory: number;
  max_long_term_memory: number;
  importance_threshold: number;
  auto_consolidation: boolean;
  consolidation_interval: number;
  decay_rate: number;

  // -- Retrieval config (retrieval.*) --
  retrieval_top_k: number;
  min_score: number;
  two_stage_retrieval: boolean;
  l0_multiplier: number;
  max_context_tokens: number;
  enable_cache: boolean;
  cache_ttl: number;

  // -- MCP config --
  mcpServers: McpServerConfigState[];
}

// ========== API Response Types (matches backend api_types) ==========

export interface ProviderInfo {
  name: string;
  endpoint: string;
  enabled: boolean;
  model_count: number;
}

export interface ModelInfo {
  name: string;
  provider: string;
  capabilities: string[];
  /** 该模型支持的思考强度档位值（每个模型自己声明的；缺省/空 = 不支持思考） */
  reasoning_efforts?: string[] | null;
  /** 上下文窗口长度（token；配置了该字段时下发，前端计算上下文占用百分比） */
  context_length?: number;
  /** 单次最大输出 token 数（配置了该字段时下发） */
  max_output_tokens?: number;
}

export interface ModelsResponse {
  providers: ProviderInfo[];
  models: ModelInfo[];
  preferences: PreferencesInfo;
}

export interface PreferencesInfo {
  chat?: ModelRef | null;
  embedding?: ModelRef | null;
  vision?: ModelRef | null;
}

// ========== Workspace Types (matches backend workspace DTO) ==========

/** 工作区条目（GET /workspace/tree 中的单个条目）。 */
export interface WorkspaceEntry {
  name: string;
  type: 'dir' | 'file';
  /** 相对工作区根的路径（如 "src/main.rs"）。 */
  path: string;
  /** 文件大小（字节），目录条目无此字段。 */
  size?: number;
  /** 修改时间（epoch 毫秒），目录条目无此字段。 */
  mtime?: number;
}

/** 工作区目录树响应（GET /workspace/tree）。 */
export interface WorkspaceTreeResponse {
  /** 工作区根目录绝对路径。 */
  root: string;
  /** 当前目录相对路径（根为空字符串）。 */
  path: string;
  entries: WorkspaceEntry[];
}

/** 目录选择器响应（GET /workspace/dirs；工作区选择：逐级浏览任意目录）。 */
export interface WorkspaceDirsResponse {
  /** 当前浏览目录（浏览根时可能为哨兵文案如"浏览根"）。 */
  current: string;
  /** 上级目录（已到浏览根时为 null）。 */
  parent: string | null;
  /** 当前目录下的子目录列表。 */
  entries: WorkspaceDirEntry[];
}

/** 目录选择器条目。 */
export interface WorkspaceDirEntry {
  /** 目录名称。 */
  name: string;
  /** 完整路径（进入该目录时传给 path 参数）。 */
  path: string;
  /** 是否为浏览根条目（盘符/家目录）。 */
  is_root?: boolean;
}

/** 工作区文件读取响应（GET /workspace/read）。 */
export interface WorkspaceReadResponse {
  /** 文件绝对路径。 */
  path: string;
  /** hashline 前缀内容（"N#ID|content"），二进制文件无此字段。 */
  content?: string;
  /** 内容是否被截断（还有更多行可分页读取）。 */
  truncated?: boolean;
  /** 文件总行数。 */
  total_lines?: number;
  /** 本窗口实际覆盖的行区间（offset 1 起始；limit 为请求的窗口行数）。 */
  showing?: {
    /** 窗口起始行号（1 起始）。 */
    offset: number;
    /** 请求的窗口行数。 */
    limit: number;
  };
  /** 是否为二进制文件。 */
  binary?: boolean;
  /** 二进制文件大小（字节）。 */
  size?: number;
  /** 二进制文件预览文本。 */
  preview?: string;
}

/** 工作区 diff 的单个 hunk（GET /workspace/diff）。 */
export interface WorkspaceDiffHunk {
  old_start: number;
  old_len: number;
  new_start: number;
  new_len: number;
}

/** 工作区文件 diff 响应（GET /workspace/diff）。 */
export interface WorkspaceDiffResponse {
  /** 相对路径（省略时响应整体文件列表）。 */
  path?: string;
  status: 'modified' | 'added' | 'removed' | 'binary' | 'unchanged';
  hunks: WorkspaceDiffHunk[];
  /** 统一 diff 文本。 */
  unified?: string;
  old_lines?: number;
  new_lines?: number;
}

/** 工作区整体 diff 响应（GET /workspace/diff 不带 path）。 */
export interface WorkspaceDiffListResponse {
  files: WorkspaceDiffResponse[];
}

/** 应用工作区补丁请求（POST /workspace/apply-patch，body: { patch }）。 */
export interface WorkspaceApplyPatchRequest {
  /** unified diff patch 文本（`--- a/path` / `+++ b/path` 头）。 */
  patch: string;
}

/** 工作区补丁应用后的单个文件结果（POST /workspace/apply-patch）。 */
export interface WorkspaceApplyPatchFile {
  /** 相对工作区根的路径（如 "src/main.rs"）。 */
  path: string;
  /** 成功应用的 hunk 数量。 */
  hunks_applied: number;
  /** 变更行数。 */
  lines_changed: number;
}

/** 工作区补丁应用响应（POST /workspace/apply-patch）。 */
export interface WorkspaceApplyPatchResponse {
  files: WorkspaceApplyPatchFile[];
  total_files: number;
}

// ========== App State Types ==========

export type View =
  | 'chat'
  | 'skills'
  | 'roles'
  | 'tools'
  | 'knowledge'
  | 'workspace'
  | 'settings'
  | 'memory'
  | 'traces'
  | 'approval'
  | 'tasks'
  | 'insights';

export type StreamStatus = 'idle' | 'streaming' | 'error';

export type Theme = 'light' | 'dark' | 'system';

export type FontSize = 'small' | 'medium' | 'large';

export type ToastType = 'error' | 'success' | 'info';

export interface ToastMessage {
  message: string;
  type: ToastType;
}

// ========== Provider Discovery Types ==========
/** 内置目录命中信息（advisory：显示名 + 默认档位），来自后端 model_catalog。 */
export interface ModelCatalogInfo {
  display_name?: string;
  reasoning_efforts?: string[];
}

export type ProviderProtocol = 'openai' | 'ollama';
export interface DiscoveredModelInfo {
  name: string;
  size?: string;
  capabilities: string[];
  /** 显示名（端点/目录提供时下发；缺省以 name 兜底展示）。 */
  display_name?: string;
  /** 上下文窗口长度（token；端点/目录提供时下发，adopt 时预填模型配置）。 */
  context_length?: number;
  /** 单次最大输出 token 数（端点/目录提供时下发，adopt 时预填模型配置）。 */
  max_output_tokens?: number;
  /** 内置目录默认思考档位（仅目录扫描返回；advisory，UI 自动附加 "off"）。 */
  reasoning_efforts?: string[];
}
export interface ProviderScanResponse {
  success: boolean;
  models: DiscoveredModelInfo[];
  error?: string;
}
export interface ProviderTestResponse {
  success: boolean;
  version?: string;
  error?: string;
}

export interface McpServerEntry {
  name: string;
  command: string;
  args: string[];
  enabled: boolean;
  description?: string;
  env?: Record<string, string>;
}

export interface McpServerConfigState {
  name: string;
  command: string;
  args: string[];
  env?: Record<string, string>;
  enabled: boolean;
  description: string;
}

export interface McpTestResponse {
  success: boolean;
  tools: number;
  error?: string;
}

/** 单条用量统计（总计或按 provider/model 分组；/usage/stats 响应）。 */
export interface UsageStat {
  group: string;
  calls: number;
  uncached_input: number;
  cached_input: number;
  completion_tokens: number;
  total_tokens: number;
  cache_hit_rate: number;
}

export interface UsageStatsResponse {
  days: number;
  start_ts: number | null;
  end_ts: number | null;
  total: UsageStat | null;
  grouped: UsageStat[];
}
