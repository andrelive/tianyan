import { getApiBase } from '@/hooks/use-api-base';
import type {
  ApprovalDecision,
  ApprovalStatusSnapshot,
  BackgroundTask,
  CancelTaskResponse,
  ChatMessage,
  CompressSessionResponse,
  McpServerEntry,
  McpTestResponse,
  MemoryListResponse,
  ProviderProtocol,
  ProviderScanResponse,
  RetrievalTracesResponse,
  SchedulerStatus,
  SessionMessagesResponse,
  UsageStatsSummary,
  WorkspaceDiffListResponse,
  WorkspaceDiffResponse,
  WorkspaceReadResponse,
  WorkspaceTreeResponse,
  WorkspaceApplyPatchRequest,
  WorkspaceApplyPatchResponse,
} from './types';

const DEFAULT_TIMEOUT = 60000;

export class ApiError extends Error {
  code?: string;
  constructor(message: string, code?: string) {
    super(message);
    this.name = 'ApiError';
    this.code = code;
  }
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
  signal?: AbortSignal,
): Promise<T> {
  const url = `${getApiBase()}${path}`;
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    Accept: 'application/json',
  };

  const controller = new AbortController();
  const timeoutId = setTimeout(() => controller.abort(), DEFAULT_TIMEOUT);
  const finalSignal = signal || controller.signal;

  try {
    const response = await fetch(url, {
      method,
      headers,
      body: body ? JSON.stringify(body) : undefined,
      signal: finalSignal,
    });

    if (!response.ok) {
      const text = await response.text();
      let message = `HTTP ${response.status}: ${text}`;
      try {
        const err = JSON.parse(text);
        message = err.error || err.message || message;
      } catch {
        /* use raw text */
      }
      throw new ApiError(message, String(response.status));
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

export async function apiDelete<T>(path: string): Promise<T> {
  return request<T>('DELETE', path);
}

export async function apiPostMultipart<T>(path: string, formData: FormData): Promise<T> {
  const url = `${getApiBase()}${path}`;
  const response = await fetch(url, {
    method: 'POST',
    body: formData,
  });
  if (!response.ok) {
    const text = await response.text();
    throw new ApiError(`HTTP ${response.status}: ${text}`, String(response.status));
  }
  return response.json();
}

// ========== Session messages ==========

export interface DeleteMessageRequest {
  message_index: number;
}

/** 删除指定索引的消息及其后的所有消息，返回剩余消息。 */
export async function deleteSessionMessage(
  sessionId: string,
  messageIndex: number,
): Promise<SessionMessagesResponse> {
  return apiPost<SessionMessagesResponse>(
    `/sessions/${encodeURIComponent(sessionId)}/messages/delete`,
    { message_index: messageIndex } satisfies DeleteMessageRequest,
  );
}

export interface RedoRequest {
  message_index: number;
}

/** 重做被回退的消息与工作区文件，返回恢复后的消息。 */
export async function redoSessionMessage(
  sessionId: string,
  messageIndex: number,
): Promise<SessionMessagesResponse> {
  return apiPost<SessionMessagesResponse>(
    `/sessions/${encodeURIComponent(sessionId)}/messages/redo`,
    { message_index: messageIndex } satisfies RedoRequest,
  );
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

// ========== Chat clarification ==========

export interface ClarifyRequest {
  session_id: string;
  answer: string;
}

export interface ClarifyResponse {
  id: string;
  session_id: string;
  message: ChatMessage;
  usage: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
}

/** 提交对 Agent 追问的回答，返回继续处理的结果（非流式）。 */
export async function clarifyChat(sessionId: string, answer: string): Promise<ClarifyResponse> {
  return apiPost<ClarifyResponse>('/chat/clarify', {
    session_id: sessionId,
    answer,
  } satisfies ClarifyRequest);
}

export { getApiBase };

// ========== Provider Discovery API ==========

export async function scanProviderModels(
  endpoint: string,
  protocol: ProviderProtocol = 'openai',
): Promise<ProviderScanResponse> {
  return apiPost<ProviderScanResponse>('/config/providers/scan', { endpoint, protocol });
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

// ========== Memory API ==========

/** 列出 VFS memory 命名空间全部条目（含 L0/L1/L2 内容）。 */
export async function fetchMemories(): Promise<MemoryListResponse> {
  return apiGet<MemoryListResponse>('/memory');
}

// ========== Retrieval traces API ==========

/** 获取最近检索轨迹（单次检索完整过程，limit 默认 20 上限 100）。 */
export async function fetchRetrievalTraces(limit = 20): Promise<RetrievalTracesResponse> {
  const params = new URLSearchParams();
  params.set('limit', String(Math.min(limit, 100)));
  return apiGet<RetrievalTracesResponse>(`/retrieval/traces?${params}`);
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

/** 列出工作区目录下的直接子条目（dir 深度 1）。 */
export async function fetchWorkspaceTree(path?: string, depth = 1): Promise<WorkspaceTreeResponse> {
  const params = new URLSearchParams();
  if (path) params.set('path', path);
  params.set('depth', String(depth));
  return apiGet<WorkspaceTreeResponse>(`/workspace/tree?${params}`);
}

/** 分页读取工作区文件内容（offset 为起始行号，limit 默认 2000）。 */
export async function fetchWorkspaceRead(
  path: string,
  offset?: number,
  limit = 2000,
): Promise<WorkspaceReadResponse> {
  const params = new URLSearchParams({ path });
  if (offset !== undefined) params.set('offset', String(offset));
  params.set('limit', String(limit));
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

/** 应用工作区补丁（保存前 diff 确认后的写入，body: { patch }）。 */
export async function fetchWorkspaceApplyPatch(
  patch: string,
): Promise<WorkspaceApplyPatchResponse> {
  return apiPost<WorkspaceApplyPatchResponse>('/workspace/apply-patch', {
    patch,
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
