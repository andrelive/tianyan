import { getApiBase } from '@/hooks/use-api-base';
import type {
  ApprovalDecision,
  ApprovalStatusSnapshot,
  ChatMessage,
  McpServerEntry,
  McpTestResponse,
  MemoryListResponse,
  OllamaScanResponse,
  OllamaTestResponse,
  RetrievalTracesResponse,
  SessionMessagesResponse,
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

// ========== Ollama API ==========

export async function scanOllamaModels(endpoint?: string): Promise<OllamaScanResponse> {
  return apiPost<OllamaScanResponse>('/config/ollama/scan', { endpoint });
}

export async function testOllamaConnection(endpoint?: string): Promise<OllamaTestResponse> {
  return apiPost<OllamaTestResponse>('/config/ollama/test', { endpoint });
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
): Promise<{ ok: boolean }> {
  return apiPost<{ ok: boolean }>('/approval/respond', {
    request_id: requestId,
    decision,
    ...(reason ? { reason } : {}),
  } satisfies RespondApprovalRequest);
}

interface RespondApprovalRequest {
  request_id: string;
  decision: ApprovalDecision;
  reason?: string;
}
