import { getApiBase } from '@/hooks/use-api-base';

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
  signal?: AbortSignal
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

export async function apiPostMultipart<T>(
  path: string,
  formData: FormData
): Promise<T> {
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

export { getApiBase };

// ========== Ollama API ==========

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

export async function scanOllamaModels(endpoint?: string): Promise<OllamaScanResponse> {
  return apiPost<OllamaScanResponse>('/config/ollama/scan', { endpoint });
}

export async function testOllamaConnection(endpoint?: string): Promise<OllamaTestResponse> {
  return apiPost<OllamaTestResponse>('/config/ollama/test', { endpoint });
}

// ========== MCP API ==========

export interface McpServerEntry {
  name: string;
  command: string;
  args: string[];
  env?: Record<string, string>;
  enabled: boolean;
  description?: string;
}

export interface McpTestResponse {
  success: boolean;
  tools: number;
  error?: string;
}

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

export async function updateSoulContent(content: string): Promise<{ success: boolean; message: string }> {
  return apiPut('/config/soul', { content });
}

export async function fetchDefaultSoul(): Promise<SoulResponse> {
  return apiGet<SoulResponse>('/config/soul/default');
}

// ========== VFS API ==========

export interface NamespaceInfo {
  name: string;
  display: string;
  uri: string;
}

export interface NamespaceListResponse {
  namespaces: NamespaceInfo[];
}

export interface EntryItem {
  uri: string;
  name: string;
  is_directory: boolean;
  has_abstract: boolean;
  has_overview: boolean;
  has_detail: boolean;
  updated_at?: string;
}

export interface ListEntriesResponse {
  entries: EntryItem[];
}

export interface ReadEntryResponse {
  uri: string;
  level: string;
  content: string;
  updated_at?: string;
}

export async function fetchNamespaces(): Promise<NamespaceListResponse> {
  return apiGet<NamespaceListResponse>('/vfs/namespaces');
}

export async function fetchVfsEntries(namespace?: string, path?: string): Promise<ListEntriesResponse> {
  const params = new URLSearchParams();
  if (namespace) params.set('namespace', namespace);
  if (path) params.set('path', path);
  return apiGet<ListEntriesResponse>(`/vfs/entries?${params}`);
}

export async function fetchVfsEntryContent(uri: string, level?: string): Promise<ReadEntryResponse> {
  const params = new URLSearchParams({ uri });
  if (level) params.set('level', level);
  return apiGet<ReadEntryResponse>(`/vfs/entries/read?${params}`);
}

export async function updateVfsEntry(
  uri: string,
  level: string,
  content: string,
): Promise<{ success: boolean }> {
  return apiPut('/vfs/entries', { uri, level, content });
}

export async function deleteVfsEntry(uri: string): Promise<{ success: boolean }> {
  return apiDelete(`/vfs/entries?uri=${encodeURIComponent(uri)}`);
}

export interface CreateEntryRequest {
  uri: string;
  is_directory?: boolean;
  abstract_content?: string;
  overview_content?: string;
  detail_content?: string;
}

export async function createVfsEntry(req: CreateEntryRequest): Promise<{ success: boolean; message: string; uri: string }> {
  return apiPost('/vfs/entries', req);
}
