// ========== Chat Types ==========

export type MessageRole = 'system' | 'user' | 'assistant';

export type StreamChunkType =
  'answer' | 'thought' | 'tool_call' | 'observation' | 'clarification' | 'error';

export interface ChatMessage {
  id?: string;
  role: MessageRole;
  content: string;
  timestamp?: string;
  skill_calls?: SkillCallInfo[];
  chunk_type?: StreamChunkType;
}

export interface ChatRequest {
  session_id?: string | null;
  messages: ChatMessage[];
  stream: boolean;
  temperature: number;
  max_tokens: number;
  model?: string | null;
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
}

export interface ChatStreamEvent {
  id: string;
  session_id: string;
  delta: string;
  finish_reason?: string | null;
  skill_calls?: SkillCallInfo[] | null;
  chunk_type: StreamChunkType;
}

export interface SkillCallInfo {
  skill_id: string;
  skill_name: string;
  success: boolean;
  execution_time_ms: number;
  error?: string | null;
}

// ========== Session Types ==========

export interface Session {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  message_count: number;
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
}

/** 审批审计记录。 */
export interface ApprovalRecord {
  request: ApprovalRequest;
  response: ApprovalResponse | null;
  execution_result: boolean | null;
}

/** 审批工作流配置。 */
export interface ApprovalWorkflowConfig {
  default_timeout_secs: number;
  enable_auto_approval: boolean;
  persist_records: boolean;
  max_pending_approvals: number;
  unattended_mode: boolean;
  wait_for_approval: boolean;
}

/** 审批状态快照（GET /approval/status）。 */
export interface ApprovalStatusSnapshot {
  config: ApprovalWorkflowConfig;
  pending_approvals: ApprovalRequest[];
  pending_confirmations: string[];
  recent_records: ApprovalRecord[];
  confirmed_action_count: number;
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

// ── Model Entry (matches backend ModelEntry) ──

export interface ProviderModelEntry {
  name: string;
  capabilities: ModelCapability[];
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

  // -- Agent config (agent.*) --
  enable_skills: boolean;
  enable_memory: boolean;
  stream_responses: boolean;
  enable_thinking: boolean;
  default_top_k: number;
  max_turns: number;
  learned_rules_top_k: number;
  learned_rules_max_tokens: number;

  // -- Storage config (storage.*) --
  data_dir: string;
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
  max_file_size: number;
  allowed_directories: string;
  blocked_directories: string;
  allowed_commands: string;
  blocked_commands: string;

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

// ========== App State Types ==========

export type View = 'chat' | 'skills' | 'knowledge' | 'settings' | 'memory' | 'traces' | 'approval';

export type StreamStatus = 'idle' | 'streaming' | 'error';

export type Theme = 'light' | 'dark' | 'system';

export type FontSize = 'small' | 'medium' | 'large';

export type ToastType = 'error' | 'success' | 'info';

export interface ToastMessage {
  message: string;
  type: ToastType;
}

// ========== Ollama / MCP Types ==========

export interface OllamaModelInfo {
  name: string;
  size: string;
  capabilities: string[];
}

export interface OllamaScanResponse {
  success: boolean;
  models: OllamaModelInfo[];
  error?: string;
}

export interface OllamaTestResponse {
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
