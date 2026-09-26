import { getApiBase } from '@/lib/api-base';
import type {
  ApprovalDecision,
  ApprovalStatusSnapshot,
  BackgroundTask,
  CancelTaskResponse,
  CompressSessionResponse,
  IngestResponse,
  KnowledgeSearchResponse,
  ListRolesResponse,
  ListGoalsResponse,
  ListSessionsResponse,
  ListTodosResponse,
  ListToolsResponse,
  McpServerEntry,
  McpTestResponse,
  MemoryListResponse,
  ModelsResponse,
  ProviderProtocol,
  ProviderScanResponse,
  RoleActionResponse,
  RoleDetail,
  RolesStatsResponse,
  SchedulerStatus,
  SearchSuggestionsResponse,
  Session,
  SkillDetail,
  SkillListResponse,
  SkillsStatsResponse,
  SessionMessagesResponse,
  UsageStatsResponse,
  UsageStatsSummary,
  WorkspaceApplyPatchRequest,
  WorkspaceApplyPatchResponse,
  WorkspaceDiffListResponse,
  WorkspaceDiffResponse,
  WorkspaceDirsResponse,
  WorkspaceReadResponse,
  WorkspaceTreeResponse,
} from './types';
import type { Goal, GoalStatus, TodoItem, TodoPriority, TodoStatus } from './types';
import type { BackendConfigResponse, BackendUpdateRequest } from './config-transform';

import { fetchWithSignal } from './fetch-with-signal';

const DEFAULT_TIMEOUT = 60000;

/**
 * 错误分类契约（对齐后端 ADR-014 语义谓词）。
 *
 * 后端错误分类经 HTTP 状态码外泄（server 层用语义谓词映射状态码，
 * 见 core/src/common/error.rs + server/src/api/shared/error.rs）；前端在
 * **唯一一处**把状态码映射为语义 kind，消费端一律用谓词
 * （isNotFound / isConflict / ...）判断——禁止散落的状态码/字符串匹配。
 */
export type ApiErrorKind =
  'not_found' | 'conflict' | 'invalid_input' | 'permission' | 'timeout' | 'unknown';

/** 状态码 → 语义类别（唯一映射表；分类契约只此一处）。 */
function kindFromStatus(status: number): ApiErrorKind {
  switch (status) {
    case 404:
      return 'not_found';
    case 409:
      return 'conflict';
    case 400:
    case 422:
      return 'invalid_input';
    case 403:
      return 'permission';
    case 408:
    case 504:
      return 'timeout';
    default:
      return 'unknown';
  }
}

export class ApiError extends Error {
  /** HTTP 状态码（字符串；保留历史兼容）。 */
  code?: string;
  /** 语义错误类别（ADR-014；消费端用谓词判断，勿直接匹配）。 */
  readonly kind: ApiErrorKind;
  /** 结构化原因（ADR-041 忙语义等）：`stream_in_progress` /
   * `destructive_op_in_progress` / `same_operation_in_flight`。 */
  readonly reason?: string;
  constructor(message: string, status?: number | string, reason?: string) {
    super(message);
    this.name = 'ApiError';
    const num = typeof status === 'number' ? status : Number(status);
    this.code = num ? String(num) : undefined;
    this.kind = num ? kindFromStatus(num) : 'unknown';
    this.reason = reason;
  }
}

/** 语义谓词：消费端错误分类的唯一入口（对应后端 is_not_found / is_conflict / …）。 */
export function isNotFound(err: unknown): boolean {
  return err instanceof ApiError && err.kind === 'not_found';
}
export function isConflict(err: unknown): boolean {
  return err instanceof ApiError && err.kind === 'conflict';
}
export function isInvalidInput(err: unknown): boolean {
  return err instanceof ApiError && err.kind === 'invalid_input';
}
export function isPermission(err: unknown): boolean {
  return err instanceof ApiError && err.kind === 'permission';
}
export function isTimeout(err: unknown): boolean {
  return err instanceof ApiError && err.kind === 'timeout';
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
  signal?: AbortSignal,
): Promise<T> {
  const url = `${getApiBase()}${path}`;
  const headers: Record<string, string> = {
    Accept: 'application/json',
  };
  // FormData（multipart）不设 Content-Type——浏览器自动带 boundary；
  // 其余 JSON 请求统一 Content-Type。
  let payload: BodyInit | undefined;
  if (body instanceof FormData) {
    payload = body;
  } else if (body !== undefined) {
    headers['Content-Type'] = 'application/json';
    payload = JSON.stringify(body);
  }

  const controller = new AbortController();
  const timeoutId = setTimeout(() => controller.abort(), DEFAULT_TIMEOUT);
  const finalSignal = signal || controller.signal;

  try {
    const response = await fetchWithSignal(
      url,
      {
        method,
        headers,
        body: payload,
      },
      finalSignal,
    );

    if (!response.ok) {
      const text = await response.text();
      let message = `HTTP ${response.status}: ${text}`;
      let reason: string | undefined;
      try {
        const err = JSON.parse(text);
        message = err.error || err.message || message;
        reason = typeof err.reason === 'string' ? err.reason : undefined;
      } catch {
        /* use raw text */
      }
      throw new ApiError(message, response.status, reason);
    }

    return await response.json();
  } finally {
    clearTimeout(timeoutId);
  }
}

export async function apiGet<T>(path: string): Promise<T> {
  return request<T>('GET', path);
}

export async function apiPost<T>(path: string, body: unknown): Promise<T> {
  return request<T>('POST', path, body);
}

export async function apiPut<T>(path: string, body: unknown): Promise<T> {
  return request<T>('PUT', path, body);
}

export async function apiPatch<T>(path: string, body: unknown): Promise<T> {
  return request<T>('PATCH', path, body);
}

export async function apiDelete<T>(path: string): Promise<T> {
  return request<T>('DELETE', path);
}

export async function apiPostMultipart<T>(path: string, formData: FormData): Promise<T> {
  // 与 JSON 请求同一错误/超时路径（语义 kind + 60s 超时），不另起 fetch。
  return request<T>('POST', path, formData);
}

// ========== 类型化端点包装（F3：组件禁止裸 apiGet/apiPost 拼路径） ==========

/** 获取技能目录（全部技能；SkillsPanel 按 custom 类过滤展示）。 */
export async function getSkills(): Promise<SkillListResponse> {
  return apiGet<SkillListResponse>('/skills');
}

/** 语义搜索知识库（关键词查询，返回命中条目列表；offset 用于分页加载更多）。 */
export async function searchKnowledge(
  query: string,
  limit = 10,
  offset = 0,
): Promise<KnowledgeSearchResponse> {
  return apiGet<KnowledgeSearchResponse>(
    `/knowledge/search?q=${encodeURIComponent(query)}&limit=${limit}&offset=${offset}`,
  );
}

/** 知识摄入（multipart 文件上传；错误语义与 JSON 请求一致）。 */
export async function ingestKnowledge(formData: FormData): Promise<IngestResponse> {
  return apiPostMultipart<IngestResponse>('/knowledge/ingest', formData);
}

/** 获取当前模型目录（含思考档位规格）。 */
export async function getModels(): Promise<ModelsResponse> {
  return apiGet<ModelsResponse>('/config/models');
}

/** 列出全部会话（侧边栏）。 */
export async function listSessions(): Promise<ListSessionsResponse> {
  return apiGet<ListSessionsResponse>('/sessions');
}

/** 获取后端配置（BackendConfigResponse 原始形状）。 */
export async function getConfig(): Promise<BackendConfigResponse> {
  return apiGet<BackendConfigResponse>('/config');
}

/** 保存配置（后端校验 + 持久化 + 热重载）。 */
export async function saveConfig(
  payload: BackendUpdateRequest,
): Promise<{ success: boolean; message: string }> {
  return apiPut<{ success: boolean; message: string }>('/config', payload);
}

/** 测试提供商连接（endpoint / api_key / model）。 */
export async function testProviderConnection(
  endpoint: string,
  apiKey: string,
  model: string,
): Promise<{ success: boolean; message: string }> {
  return apiPost<{ success: boolean; message: string }>('/config/test-connection', {
    endpoint,
    api_key: apiKey,
    model,
  });
}

/** 使用统计（按时间范围查询；range 形如 days=N 或 start_ts=..&end_ts=..）。 */
export async function fetchUsageStatsRange(range: string): Promise<UsageStatsResponse> {
  return apiGet<UsageStatsResponse>(`/usage/stats?${range}`);
}

// ========== Data Migration API ==========

/** 数据目录搬迁（校验 + 写迁移请求 + 触发服务器关停；应用将自动重启）。 */
export async function migrateDataDir(
  newDir: string,
): Promise<{ success: boolean; message: string }> {
  return apiPost<{ success: boolean; message: string }>('/config/migrate-data-dir', {
    new_dir: newDir,
  });
}

// ========== Tools & Skills ==========

/** 列出全部系统工具（工具目录：与 LLM tools 列表同源） */
export async function getTools(): Promise<ListToolsResponse> {
  return apiGet<ListToolsResponse>('/tools');
}

/** 获取技能详情（完整内容 + 创建/更新时间） */
export async function getSkillDetail(skillId: string): Promise<SkillDetail> {
  return apiGet<SkillDetail>(`/skills/${encodeURIComponent(skillId)}`);
}

export async function getSkillsStats(): Promise<SkillsStatsResponse> {
  return apiGet<SkillsStatsResponse>('/skills/stats');
}

export async function getRoles(): Promise<ListRolesResponse> {
  return apiGet<ListRolesResponse>('/roles');
}

export async function getRoleDetail(name: string): Promise<RoleDetail> {
  return apiGet<RoleDetail>(`/roles/${encodeURIComponent(name)}`);
}

export async function getRolesStats(): Promise<RolesStatsResponse> {
  return apiGet<RolesStatsResponse>('/roles/stats');
}

export async function resetRole(name: string): Promise<RoleActionResponse> {
  return apiPost<RoleActionResponse>(`/roles/${encodeURIComponent(name)}/reset`, {});
}

export async function deleteRole(name: string): Promise<RoleActionResponse> {
  return apiDelete<RoleActionResponse>(`/roles/${encodeURIComponent(name)}`);
}

// ========== Session messages ==========

/** 获取会话全部消息（历史加载；与流式边界事件同构——均携带 segments 时间线）。
 * 前端展示列表经合并/过滤后索引与服务端错位，历史加载按服务端权威结构覆盖本地。 */
export async function fetchSessionMessages(sessionId: string): Promise<SessionMessagesResponse> {
  return apiGet<SessionMessagesResponse>(`/sessions/${encodeURIComponent(sessionId)}/messages`);
}
/** 分段加载会话消息（ADR-035 §8：上滚取更早历史；beforeSeq 省略 = 最近一页）。 */
export async function fetchSessionMessagesPage(
  sessionId: string,
  beforeSeq?: number,
  limit?: number,
): Promise<SessionMessagesResponse> {
  const params = new URLSearchParams();
  if (beforeSeq !== undefined) params.set('before_seq', String(beforeSeq));
  if (limit !== undefined) params.set('limit', String(limit));
  const qs = params.toString();
  return apiGet<SessionMessagesResponse>(
    `/sessions/${encodeURIComponent(sessionId)}/messages${qs ? `?${qs}` : ''}`,
  );
}

/** 删除会话（级联删除消息与工作区绑定）。 */
export async function deleteSession(sessionId: string): Promise<void> {
  return apiDelete<void>(`/sessions/${encodeURIComponent(sessionId)}`);
}

export interface DeleteMessageRequest {
  message_id: string;
  /** 客户端生成的幂等键（ADR-041）：双击/重试同一 opId 只执行一次。 */
  operation_id?: string;
}

/** 回退结果（ADR-040：核心回退事务的结构化回报）。 */
export interface RollbackOutcome {
  /** 被截断（丢弃）的消息数。 */
  truncated: number;
  /** 恢复 / 删除的工作区文件数。 */
  restored_files: number;
  /** 会话生效的工作目录；null = 未绑定工作区（**文件未回退**，须明示）。 */
  workdir: string | null;
  /** 可撤销回退（重做数据已保存）。 */
  redo_available: boolean;
  /** 被取消的时序锚点后任务数（委托 + 命令）。 */
  cancelled_tasks: number;
}

/** 重做结果（ADR-040）。 */
export interface RedoOutcome {
  /** 恢复的消息数。 */
  restored_messages: number;
  /** 恢复的工作区文件数。 */
  restored_files: number;
}

/** 删除指定消息及其后的所有消息（按消息 ID 定位，返回剩余消息）。
 * 前端展示列表经合并/过滤后索引与服务端错位，数字索引会删过头；
 * 消息 ID 是两端共享的稳定键。
 * 响应附带 `rollback`（ADR-040）：`workdir === null` 表示文件未回退。 */
export async function deleteSessionMessage(
  sessionId: string,
  messageId: string,
  operationId?: string,
): Promise<SessionMessagesResponse & { rollback?: RollbackOutcome }> {
  return apiPost<SessionMessagesResponse & { rollback?: RollbackOutcome }>(
    `/sessions/${encodeURIComponent(sessionId)}/messages/delete`,
    {
      message_id: messageId,
      ...(operationId ? { operation_id: operationId } : {}),
    } satisfies DeleteMessageRequest,
  );
}

export interface RedoRequest {
  message_id: string;
  /** 客户端生成的幂等键（ADR-041）：语义同 DeleteMessageRequest。 */
  operation_id?: string;
}

/** 重做被删除的消息与工作区文件，返回恢复后的消息（+ 重做结果 `redo`）。 */
export async function redoSessionMessage(
  sessionId: string,
  messageId: string,
  operationId?: string,
): Promise<SessionMessagesResponse & { redo?: RedoOutcome }> {
  return apiPost<SessionMessagesResponse & { redo?: RedoOutcome }>(
    `/sessions/${encodeURIComponent(sessionId)}/messages/redo`,
    {
      message_id: messageId,
      ...(operationId ? { operation_id: operationId } : {}),
    } satisfies RedoRequest,
  );
}

export interface AnswerRequest {
  session_id: string;
  answers: unknown;
}

/** 提交 ask_user 追问回答（同步工具语义）：回答提交到等待通道，
 * 工具执行恢复，结果经主对话流返回。无等待中的追问时幂等空操作。 */
export async function answerChat(sessionId: string, answers: unknown): Promise<void> {
  await apiPost<void>('/chat/answer', {
    session_id: sessionId,
    answers,
  } satisfies AnswerRequest);
}

/** 更新会话标题。 */
export async function updateSessionTitle(
  sessionId: string,
  title: string,
): Promise<{ success: boolean; message: string }> {
  return apiPost(`/sessions/${encodeURIComponent(sessionId)}/title`, {
    title,
  });
}

/** 更新会话绑定的工作目录（工作区归属；空串清除绑定）。 */
export async function updateSessionWorkspace(
  sessionId: string,
  workingDirectory: string,
): Promise<Session> {
  return apiPut<Session>(`/sessions/${encodeURIComponent(sessionId)}/workspace`, {
    working_directory: workingDirectory,
  });
}

export { getApiBase };

// ========== Provider Discovery API ==========

export async function scanProviderModels(
  endpoint: string,
  protocol: ProviderProtocol = 'openai',
  /** Provider 名称：命中内置目录时后端零网络直接返回目录模型。 */
  provider?: string,
): Promise<ProviderScanResponse> {
  return apiPost<ProviderScanResponse>('/config/providers/scan', {
    endpoint,
    protocol,
    ...(provider && provider.trim() ? { provider } : {}),
  });
}

// ========== Model switch API ==========

export interface SwitchModelRequest {
  model: string;
  /** 能力类型（默认 'chat'，持久化到 models.preferences.chat）。 */
  capability: 'chat';
  /** 目标提供商（同名模型跨提供商消歧；缺省后端按名匹配第一个已启用提供商）。 */
  provider?: string;
}

export interface SwitchModelResponse {
  success: boolean;
  message: string;
}

/** 切换默认模型（POST /config/models/switch），持久化 preferences.chat。 */
export async function switchModel(
  model: string,
  capability: 'chat' = 'chat',
  provider?: string,
): Promise<SwitchModelResponse> {
  return apiPost<SwitchModelResponse>('/config/models/switch', {
    model,
    capability,
    ...(provider && provider.trim() ? { provider } : {}),
  } satisfies SwitchModelRequest);
}

// ========== MCP API ==========

export async function listMcpServers(): Promise<McpServerEntry[]> {
  return apiGet<McpServerEntry[]>('/config/mcp/servers');
}

export async function addMcpServer(server: McpServerEntry): Promise<void> {
  return apiPost<void>('/config/mcp/servers', server);
}

export async function removeMcpServer(name: string): Promise<void> {
  return apiDelete<void>(`/config/mcp/servers/${encodeURIComponent(name)}`);
}

export async function toggleMcpServer(name: string, enabled: boolean): Promise<void> {
  return apiPut<void>(`/config/mcp/servers/${encodeURIComponent(name)}`, { enabled });
}

export async function testMcpServer(name: string): Promise<McpTestResponse> {
  return apiPost<McpTestResponse>(`/config/mcp/servers/${encodeURIComponent(name)}/test`, {});
}

// ========== 工作目录 API ==========

/** 设置工作目录（读完整配置 → 改 agent.working_directory → PUT；空串清除）。 */
export async function updateWorkingDirectory(path: string): Promise<void> {
  const resp = await apiGet<{ config: { agent?: { working_directory?: string | null } } }>(
    '/config',
  );
  const agent = resp.config.agent ?? {};
  const payload = {
    config: {
      ...(resp.config as object),
      agent: { ...agent, working_directory: path || null },
    },
  };
  await apiPut<{ success: boolean }>('/config', payload);
}

// ========== Soul API ==========

export interface SoulResponse {
  content: string;
}

export async function fetchSoulContent(): Promise<SoulResponse> {
  return apiGet<SoulResponse>('/config/soul');
}

export async function updateSoulContent(
  content: string,
): Promise<{ success: boolean; message: string }> {
  return apiPut('/config/soul', { content });
}

export async function fetchDefaultSoul(): Promise<SoulResponse> {
  return apiGet<SoulResponse>('/config/soul/default');
}

// ========== Knowledge entries (read-only browse) ==========

export interface KnowledgeEntryItem {
  uri: string;
  name: string;
  is_directory: boolean;
  has_abstract: boolean;
  has_overview: boolean;
  has_detail: boolean;
}

export interface KnowledgeEntriesResponse {
  entries: KnowledgeEntryItem[];
}

/** 列出知识库命名空间下指定路径的直接子条目。 */
export async function fetchKnowledgeEntries(path?: string): Promise<KnowledgeEntriesResponse> {
  const params = new URLSearchParams();
  if (path) params.set('path', path);
  return apiGet<KnowledgeEntriesResponse>(`/knowledge/entries?${params}`);
}

export interface KnowledgeReadResponse {
  uri: string;
  level: string;
  content: string;
}

/** 读取知识库条目的指定层级内容（abstract/overview/detail）。 */
export async function fetchKnowledgeEntryContent(
  uri: string,
  level?: string,
): Promise<KnowledgeReadResponse> {
  const params = new URLSearchParams({ uri });
  if (level) params.set('level', level);
  return apiGet<KnowledgeReadResponse>(`/knowledge/entries/read?${params}`);
}

/** 删除知识库条目（递归删除子条目 + 同步清理向量索引）。 */
export async function deleteKnowledgeEntry(
  uri: string,
): Promise<{ uri: string; success: boolean }> {
  return apiPost<{ uri: string; success: boolean }>('/knowledge/entries/delete', {
    uri,
  });
}

/** 获取知识搜索建议（GET /knowledge/search/suggestions?q=）。 */
export async function fetchKnowledgeSuggestions(q: string): Promise<SearchSuggestionsResponse> {
  return apiGet<SearchSuggestionsResponse>(
    `/knowledge/search/suggestions?q=${encodeURIComponent(q)}`,
  );
}

// ========== Memory API ==========

/** 列出 VFS memory 命名空间全部条目（含 L0/L1/L2 内容）。 */
export async function fetchMemories(): Promise<MemoryListResponse> {
  return apiGet<MemoryListResponse>('/memory');
}

// ========== Background Tasks API ==========

/** 列出全部后台任务（含 pending/running/completed/failed/cancelled）。 */
export async function fetchTasks(): Promise<BackgroundTask[]> {
  return apiGet<BackgroundTask[]>('/tasks');
}

/** 取消一个后台任务（仅 pending/running 有效；404 任务不存在）。 */
export async function cancelTask(taskId: string): Promise<CancelTaskResponse> {
  return apiPost<CancelTaskResponse>(`/tasks/${encodeURIComponent(taskId)}/cancel`, {});
}

/** 显式取消进行中的对话流（「停止」按钮；跑完再取语义下断线不取消，仅主动停止）。 */
export async function cancelChatStream(sessionId: string): Promise<{ status: string }> {
  return apiPost<{ status: string }>(`/chat/streams/${encodeURIComponent(sessionId)}/cancel`, {});
}

// ========== Scheduled Agent Tasks API ==========

/** 定时智能体任务（按执行间隔调用 agent 在指定工作区工作，ADR-024 间隔制）。 */
export interface ScheduledAgentTask {
  id: string;
  name: string;
  /** 执行间隔（秒）；距上次执行 ≥ 此值即触发，宕机超期任务重启后补跑一次。 */
  interval_secs: number;
  workspace: string;
  prompt: string;
  enabled: boolean;
  created_at: number;
  last_run_at: number | null;
  next_run_at: number | null;
  last_result: string | null;
}

/** 列出全部定时智能体任务。 */
export async function fetchScheduledTasks(): Promise<ScheduledAgentTask[]> {
  return apiGet<ScheduledAgentTask[]>('/scheduled-tasks');
}

/** 创建定时智能体任务。 */
export async function createScheduledTask(req: {
  name: string;
  interval_secs: number;
  workspace: string;
  prompt: string;
}): Promise<ScheduledAgentTask> {
  return apiPost<ScheduledAgentTask>('/scheduled-tasks', req);
}

/** 删除定时智能体任务。 */
export async function deleteScheduledTask(id: string): Promise<{ deleted: boolean }> {
  return apiDelete<{ deleted: boolean }>(`/scheduled-tasks/${encodeURIComponent(id)}`);
}

// ========== Todo & Goal API ==========

/** 列出待办（sessionId 传入时按归属会话过滤——会话面板数据源）。 */
export async function fetchTodos(sessionId?: string): Promise<ListTodosResponse> {
  const qs = sessionId ? `?session_id=${encodeURIComponent(sessionId)}` : '';
  return apiGet<ListTodosResponse>(`/todos${qs}`);
}

/** 创建待办。 */
export async function createTodo(req: {
  title: string;
  description?: string;
  priority?: TodoPriority;
  goal_id?: string;
  due_at?: number;
}): Promise<{ todo: TodoItem }> {
  return apiPost<{ todo: TodoItem }>('/todos', req);
}

/** 更新待办（部分字段；status 支持 pending/in_progress/completed）。 */
export async function updateTodo(
  id: string,
  req: {
    title?: string;
    description?: string;
    status?: TodoStatus;
    priority?: TodoPriority;
    goal_id?: string;
    due_at?: number;
  },
): Promise<{ todo: TodoItem }> {
  return apiPatch<{ todo: TodoItem }>(`/todos/${encodeURIComponent(id)}`, req);
}

/** 删除待办。 */
export async function deleteTodo(id: string): Promise<{ deleted: boolean }> {
  return apiDelete<{ deleted: boolean }>(`/todos/${encodeURIComponent(id)}`);
}

/** 列出目标（含进度；sessionId 传入时按归属会话过滤——会话面板数据源）。 */
export async function fetchGoals(sessionId?: string): Promise<ListGoalsResponse> {
  const qs = sessionId ? `?session_id=${encodeURIComponent(sessionId)}` : '';
  return apiGet<ListGoalsResponse>(`/goals${qs}`);
}

/** 创建目标。 */
export async function createGoal(req: {
  title: string;
  description?: string;
  target_date?: number;
}): Promise<{ goal: Goal }> {
  return apiPost<{ goal: Goal }>('/goals', req);
}

/** 更新目标（部分字段；status 支持 active/completed/archived）。 */
export async function updateGoal(
  id: string,
  req: {
    title?: string;
    description?: string;
    status?: GoalStatus;
    target_date?: number;
  },
): Promise<{ goal: Goal }> {
  return apiPatch<{ goal: Goal }>(`/goals/${encodeURIComponent(id)}`, req);
}

/** 删除目标。 */
export async function deleteGoal(id: string): Promise<{ deleted: boolean }> {
  return apiDelete<{ deleted: boolean }>(`/goals/${encodeURIComponent(id)}`);
}

// ========== Session compression API ==========

/** 手动压缩会话上下文（与自动压缩共用逻辑）。 */
export async function compressSession(sessionId: string): Promise<CompressSessionResponse> {
  return apiPost<CompressSessionResponse>(
    `/sessions/${encodeURIComponent(sessionId)}/compress`,
    {},
  );
}

// ========== Approval API ==========

/** 获取审批状态快照（配置 + 待处理请求 + 最近审计）。 */
export async function fetchApprovalStatus(): Promise<ApprovalStatusSnapshot> {
  return apiGet<ApprovalStatusSnapshot>('/approval/status');
}

/** 响应待处理审批请求（批准/拒绝/要求更多信息）。 */
export async function respondApproval(
  requestId: string,
  decision: ApprovalDecision,
  reason?: string,
  editedCommand?: string,
): Promise<{ ok: boolean }> {
  return apiPost<{ ok: boolean }>('/approval/respond', {
    request_id: requestId,
    decision,
    ...(reason ? { reason } : {}),
    ...(editedCommand ? { edited_command: editedCommand } : {}),
  } satisfies RespondApprovalRequest);
}

interface RespondApprovalRequest {
  request_id: string;
  decision: ApprovalDecision;
  reason?: string;
  /** 用户编辑后的命令（纠正/改写场景；仅 decision=approve 时生效，记入审计）。 */
  edited_command?: string;
}

// ========== Insights API ==========

/** 获取定时任务调度器状态（任务列表 / 执行次数 / 距上次执行）。 */
export async function fetchSchedulerStatus(): Promise<SchedulerStatus> {
  return apiGet<SchedulerStatus>('/scheduler/status');
}

/** 获取使用统计摘要（技能调用 / 文档访问 / 搜索热度）。 */
export async function fetchUsageStats(): Promise<UsageStatsSummary> {
  return apiGet<UsageStatsSummary>('/stats');
}

// ========== Workspace API (read-only, Phase 1) ==========

/** 目录选择器：列出指定路径下的子目录（path 缺省 = 浏览根：Windows 盘符 / 家目录）。 */
export async function fetchWorkspaceDirs(path?: string): Promise<WorkspaceDirsResponse> {
  const params = new URLSearchParams();
  if (path) params.set('path', path);
  const qs = params.toString();
  return apiGet<WorkspaceDirsResponse>(`/workspace/dirs${qs ? `?${qs}` : ''}`);
}

/** 列出工作区目录下的直接子条目（dir 深度 1；sessionId 解析会话绑定目录）。 */
export async function fetchWorkspaceTree(
  path?: string,
  depth = 1,
  sessionId?: string,
): Promise<WorkspaceTreeResponse> {
  const params = new URLSearchParams();
  if (path) params.set('path', path);
  params.set('depth', String(depth));
  if (sessionId) params.set('session_id', sessionId);
  return apiGet<WorkspaceTreeResponse>(`/workspace/tree?${params}`);
}

/** 分页读取工作区文件内容（offset 为起始行号，limit 默认 2000；sessionId 解析会话绑定目录）。 */
export async function fetchWorkspaceRead(
  path: string,
  offset?: number,
  limit = 2000,
  sessionId?: string,
): Promise<WorkspaceReadResponse> {
  const params = new URLSearchParams({ path });
  if (offset !== undefined) params.set('offset', String(offset));
  params.set('limit', String(limit));
  if (sessionId) params.set('session_id', sessionId);
  return apiGet<WorkspaceReadResponse>(`/workspace/read?${params}`);
}

/** 读取指定文件相对快照的 diff（sessionId/index 定位快照）。 */
export async function fetchWorkspaceDiff(
  path: string,
  sessionId?: string,
  index?: number,
): Promise<WorkspaceDiffResponse> {
  const params = new URLSearchParams({ path, base: 'snapshot' });
  if (sessionId) params.set('session_id', sessionId);
  if (index !== undefined) params.set('index', String(index));
  return apiGet<WorkspaceDiffResponse>(`/workspace/diff?${params}`);
}

/** 读取工作区整体 diff（不带 path 时返回文件列表）。 */
export async function fetchWorkspaceDiffList(
  sessionId?: string,
  index?: number,
): Promise<WorkspaceDiffListResponse> {
  const params = new URLSearchParams({ base: 'snapshot' });
  if (sessionId) params.set('session_id', sessionId);
  if (index !== undefined) params.set('index', String(index));
  return apiGet<WorkspaceDiffListResponse>(`/workspace/diff?${params}`);
}

/** 应用工作区补丁（保存前 diff 确认后的写入；sessionId 解析会话绑定目录）。 */
export async function fetchWorkspaceApplyPatch(
  patch: string,
  sessionId?: string,
): Promise<WorkspaceApplyPatchResponse> {
  return apiPost<WorkspaceApplyPatchResponse>('/workspace/apply-patch', {
    patch,
    ...(sessionId ? { session_id: sessionId } : {}),
  } satisfies WorkspaceApplyPatchRequest);
}

// ── 剪贴板 I/O（T1 路线：复制即记忆） ─────────────────────────────

/** 待确认的剪贴板捕获。 */
export interface ClipboardPendingCapture {
  id: string;
  text: string;
  captured_at: string;
}

/** 获取待确认的剪贴板捕获（无则 null）。 */
export async function fetchClipboardPending(): Promise<ClipboardPendingCapture | null> {
  const resp = await apiGet<{ pending: ClipboardPendingCapture | null }>('/clipboard/pending');
  return resp.pending;
}

/** 响应剪贴板捕获（沉淀/忽略）。 */
export async function respondClipboard(
  action: 'remember' | 'knowledge' | 'ignore',
  text?: string,
): Promise<{ status: string; uri?: string }> {
  return apiPost<{ status: string; uri?: string }>('/clipboard/respond', {
    action,
    ...(text ? { text } : {}),
  });
}
