/**
 * Config transform utilities.
 *
 * Converts between the frontend's flat-ish ConfigState form model and the
 * backend's nested TianyanConfig JSON structure.
 *
 * Backend expects:
 *   GET  /api/v1/config       → { config: TianyanConfig }
 *   PUT  /api/v1/config       ← { config: TianyanConfig }
 *
 * Frontend form model:
 *   ConfigState (flat-ish, designed for easy form editing)
 */

import type {
  ConfigState,
  ProviderConfigState,
  ProviderModelEntry,
  ModelCapability,
  ModelCatalogInfo,
  ModelPreferencesState,
  ModelRef,
  ResolvedModelSpec,
} from '@/lib/types';

/* ─────── Default values ─────── */

export function emptyProvider(): ProviderConfigState {
  return {
    name: '',
    endpoint: '',
    api_key: '',
    models: [],
    timeout: 60,
    enabled: true,
    is_local: false,
    headers: {},
  };
}

export function emptyModelEntry(): ProviderModelEntry {
  return {
    name: '',
    capabilities: [] as ModelCapability[],
  };
}

export function emptyPreferences(): ModelPreferencesState {
  return {
    chat: null,
    embedding: null,
    vision: null,
  };
}

export function emptyConfigState(): ConfigState {
  return {
    providers: [],
    preferences: emptyPreferences(),
    resolvedSpecs: {},
    modelCatalog: {},
    mcpServers: [],
    default_top_k: 5,
    max_turns: 20,
    learned_rules_top_k: 5,
    working_directory: '',
    data_dir: '',
    collection_name: 'tianyan_data',
    vector_dimension: 1536,
    max_storage_size: 0,
    auto_cleanup: true,
    cleanup_days: 365,
    log_level: 'info',
    log_format: 'text',
    log_max_file_size: 10,
    log_max_files: 5,
    log_include_timestamp: true,
    log_include_location: false,
    security_enabled: true,
    confirm_commands: true,
    audit_logging: true,
    allow_all_operations: false,
    max_file_size: 10485760,
    allowed_directories: '',
    blocked_directories: '',
    allowed_commands: '',
    blocked_commands: '',
    clipboard_enabled: false,
    clipboard_auto_capture: false,
    clipboard_prompt_confirm: true,
    max_session_memory: 8000,
    max_long_term_memory: 10000,
    importance_threshold: 0.5,
    auto_consolidation: true,
    consolidation_interval: 3600,
    decay_rate: 0.01,
    retrieval_top_k: 10,
    min_score: 0.5,
    two_stage_retrieval: true,
    l0_multiplier: 3,
    max_context_tokens: 4000,
    enable_cache: true,
    cache_ttl: 300,
  };
}

/* ─────── Backend TianyanConfig shape (for serialization) ─────── */

interface BackendAgentConfig {
  default_top_k: number;
  loaded_rules_top_k: number;
  working_directory?: string | null;
  max_turns: number;
}

interface BackendModelEntry {
  name: string;
  capabilities: string[];
  /** 思考强度档位值（每个模型自己声明的，如 ["low","high","max"]；缺省查内置模型表） */
  reasoning_efforts?: string[];
  context_length?: number;
  max_output_tokens?: number;
  max_input_tokens?: number;
}

interface BackendProviderConfig {
  name: string;
  endpoint: string;
  api_key: string | null;
  models: BackendModelEntry[];
  timeout: number;
  enabled: boolean;
  is_local: boolean;
  headers: Record<string, string>;
}

interface BackendModelRef {
  provider: string;
  model: string;
}

interface BackendModelPreferences {
  chat: BackendModelRef | null;
  embedding: BackendModelRef | null;
  vision: BackendModelRef | null;
}

interface BackendModelsConfig {
  providers: BackendProviderConfig[];
  preferences: BackendModelPreferences;
}

interface BackendStorageConfig {
  data_dir: string;
  max_storage_size: number;
  auto_cleanup: boolean;
  cleanup_days: number;
  vector: {
    collection_name: string;
    vector_dimension: number;
  };
}

interface BackendLoggingConfig {
  level: string;
  format: string;
  max_file_size: number;
  max_files: number;
  include_timestamp: boolean;
  include_location: boolean;
}

interface BackendSecurityConfig {
  enabled: boolean;
  confirm_commands: boolean;
  audit_logging: boolean;
  allow_all_operations?: boolean;
  max_file_size: number;
  allowed_directories: string[];
  blocked_directories: string[];
  allowed_commands: string[];
  blocked_commands: string[];
}

interface BackendClipboardConfig {
  enabled: boolean;
  auto_capture: boolean;
  prompt_confirm: boolean;
}

interface BackendMemoryConfig {
  max_session_memory: number;
  max_long_term_memory: number;
  importance_threshold: number;
  auto_consolidation: boolean;
  consolidation_interval: number;
  decay_rate: number;
}

interface BackendRetrievalConfig {
  default_top_k: number;
  min_score: number;
  two_stage_retrieval: boolean;
  l0_multiplier: number;
  max_context_tokens: number;
  enable_cache: boolean;
  cache_ttl: number;
}

interface BackendTianyanConfig {
  agent: BackendAgentConfig;
  models: BackendModelsConfig;
  storage: BackendStorageConfig;
  logging: BackendLoggingConfig;
  security: BackendSecurityConfig;
  clipboard: BackendClipboardConfig;
  memory: BackendMemoryConfig;
  retrieval: BackendRetrievalConfig;
  mcp: BackendMcpConfig;
}

interface BackendMcpConfig {
  servers: BackendMcpServerEntry[];
}

interface BackendMcpServerEntry {
  name: string;
  command: string;
  args: string[];
  env?: Record<string, string>;
  enabled: boolean;
  description?: string;
}

export interface BackendUpdateRequest {
  config: BackendTianyanConfig;
}

export interface BackendConfigResponse {
  config: BackendTianyanConfig;
  /** 后端解析出的模型生效规格（显式 > 内置表 > 默认），key = "{provider}/{model}"。旧后端可能缺省。 */
  model_specs?: Record<string, ResolvedModelSpec>;
  /** 后端解析出的内置目录命中（advisory），key = "{provider}/{model}"。旧后端可能缺省。 */
  model_catalog?: Record<string, ModelCatalogInfo>;
}

/* ─────── Form → Backend (ConfigState → { config: TianyanConfig }) ─────── */

function toModelRef(ref: ModelRef | null | undefined): BackendModelRef | null {
  if (!ref || !ref.provider || !ref.model) return null;
  return { provider: ref.provider, model: ref.model };
}

function splitLines(s: string): string[] {
  return s
    .split(/[\n,]/)
    .map((line) => line.trim())
    .filter(Boolean);
}

export function toBackendConfig(cs: ConfigState): BackendUpdateRequest {
  return {
    config: {
      agent: {
        default_top_k: cs.default_top_k,
        loaded_rules_top_k: cs.learned_rules_top_k,
        max_turns: cs.max_turns,
        working_directory: cs.working_directory || null,
      },
      models: {
        providers: cs.providers.map((p) => ({
          name: p.name,
          endpoint: p.endpoint,
          api_key: p.api_key || null,
          models: p.models.map((m) => ({
            name: m.name,
            capabilities: m.capabilities as string[],
            ...(m.reasoning_efforts && m.reasoning_efforts.length > 0
              ? { reasoning_efforts: m.reasoning_efforts }
              : {}),
            ...(m.context_length !== undefined ? { context_length: m.context_length } : {}),
            ...(m.max_output_tokens !== undefined
              ? { max_output_tokens: m.max_output_tokens }
              : {}),
            ...(m.max_input_tokens !== undefined ? { max_input_tokens: m.max_input_tokens } : {}),
          })),
          timeout: p.timeout,
          enabled: p.enabled,
          is_local: p.is_local || false,
          headers: p.headers || {},
        })),
        preferences: {
          chat: toModelRef(cs.preferences.chat),
          embedding: toModelRef(cs.preferences.embedding),
          vision: toModelRef(cs.preferences.vision),
        },
      },
      storage: {
        data_dir: cs.data_dir,
        max_storage_size: cs.max_storage_size,
        auto_cleanup: cs.auto_cleanup,
        cleanup_days: cs.cleanup_days,
        vector: {
          collection_name: cs.collection_name,
          vector_dimension: cs.vector_dimension,
        },
      },
      logging: {
        level: cs.log_level,
        format: cs.log_format,
        max_file_size: cs.log_max_file_size,
        max_files: cs.log_max_files,
        include_timestamp: cs.log_include_timestamp,
        include_location: cs.log_include_location,
      },
      security: {
        enabled: cs.security_enabled,
        confirm_commands: cs.confirm_commands,
        audit_logging: cs.audit_logging,
        allow_all_operations: cs.allow_all_operations,
        max_file_size: cs.max_file_size,
        allowed_directories: splitLines(cs.allowed_directories),
        blocked_directories: splitLines(cs.blocked_directories),
        allowed_commands: splitLines(cs.allowed_commands),
        blocked_commands: splitLines(cs.blocked_commands),
      },
      clipboard: {
        enabled: cs.clipboard_enabled,
        auto_capture: cs.clipboard_auto_capture,
        prompt_confirm: cs.clipboard_prompt_confirm,
      },
      memory: {
        max_session_memory: cs.max_session_memory,
        max_long_term_memory: cs.max_long_term_memory,
        importance_threshold: cs.importance_threshold,
        auto_consolidation: cs.auto_consolidation,
        consolidation_interval: cs.consolidation_interval,
        decay_rate: cs.decay_rate,
      },
      retrieval: {
        default_top_k: cs.retrieval_top_k,
        min_score: cs.min_score,
        two_stage_retrieval: cs.two_stage_retrieval,
        l0_multiplier: cs.l0_multiplier,
        max_context_tokens: cs.max_context_tokens,
        enable_cache: cs.enable_cache,
        cache_ttl: cs.cache_ttl,
      },
      mcp: {
        servers: (cs.mcpServers || []).map((s) => ({
          name: s.name,
          command: s.command,
          args: s.args || [],
          env: s.env || undefined,
          enabled: s.enabled,
          description: s.description || undefined,
        })),
      },
    },
  };
}

/* ─────── Backend → Form (ConfigResponse → ConfigState) ─────── */

export function fromBackendConfig(response: BackendConfigResponse): ConfigState {
  const c = response.config;
  const defaults = emptyConfigState();

  const models = c.models || ({} as BackendModelsConfig);
  const storage = c.storage || ({} as BackendStorageConfig);
  const agent = c.agent || ({} as BackendAgentConfig);
  const logging = c.logging || ({} as BackendLoggingConfig);
  const security = c.security || ({} as BackendSecurityConfig);
  const memory = c.memory || ({} as BackendMemoryConfig);
  const retrieval = c.retrieval || ({} as BackendRetrievalConfig);

  return {
    // Models
    providers: (models.providers || []).map((p) => ({
      name: p.name ?? '',
      endpoint: p.endpoint ?? '',
      api_key: p.api_key ?? '',
      models: (p.models || []).map((m) => ({
        name: m.name ?? '',
        capabilities: (m.capabilities || []) as ModelCapability[],
        reasoning_efforts: m.reasoning_efforts && m.reasoning_efforts.length > 0
          ? m.reasoning_efforts
          : undefined,
        context_length: m.context_length ?? undefined,
        max_output_tokens: m.max_output_tokens ?? undefined,
        max_input_tokens: m.max_input_tokens ?? undefined,
      })),
      timeout: p.timeout ?? defaults.providers[0]?.timeout ?? 60,
      enabled: p.enabled ?? true,
      is_local: p.is_local ?? false,
      headers: p.headers ?? {},
    })),
    resolvedSpecs: response.model_specs ?? {},
    modelCatalog: response.model_catalog ?? {},
    preferences: {
      chat: models.preferences?.chat ?? null,
      embedding: models.preferences?.embedding ?? null,
      vision: models.preferences?.vision ?? null,
    },

    // Agent
    default_top_k: agent.default_top_k ?? defaults.default_top_k,
    max_turns: agent.max_turns ?? defaults.max_turns,
    learned_rules_top_k: (() => {
      const agentRecord = agent as unknown as Record<string, unknown>;
      return typeof agentRecord.loaded_rules_top_k === 'number'
        ? agentRecord.loaded_rules_top_k
        : defaults.learned_rules_top_k;
    })(),
    working_directory: agent.working_directory ?? defaults.working_directory,

    // Storage
    data_dir:
      (typeof storage.data_dir === 'string' ? storage.data_dir : undefined) ?? defaults.data_dir,
    collection_name: storage.vector?.collection_name ?? defaults.collection_name,
    vector_dimension: storage.vector?.vector_dimension ?? defaults.vector_dimension,
    max_storage_size: storage.max_storage_size ?? defaults.max_storage_size,
    auto_cleanup: storage.auto_cleanup ?? defaults.auto_cleanup,
    cleanup_days: storage.cleanup_days ?? defaults.cleanup_days,

    // Logging
    log_level: logging.level ?? defaults.log_level,
    log_format: logging.format ?? defaults.log_format,
    log_max_file_size: logging.max_file_size ?? defaults.log_max_file_size,
    log_max_files: logging.max_files ?? defaults.log_max_files,
    log_include_timestamp: logging.include_timestamp ?? defaults.log_include_timestamp,
    log_include_location: logging.include_location ?? defaults.log_include_location,

    // Security
    security_enabled: security.enabled ?? defaults.security_enabled,
    confirm_commands: security.confirm_commands ?? defaults.confirm_commands,
    audit_logging: security.audit_logging ?? defaults.audit_logging,
    allow_all_operations: security.allow_all_operations ?? defaults.allow_all_operations,
    max_file_size: security.max_file_size ?? defaults.max_file_size,
    allowed_directories: (security.allowed_directories || []).join('\n'),
    blocked_directories: (security.blocked_directories || []).join('\n'),
    allowed_commands: (security.allowed_commands || []).join('\n'),
    blocked_commands: (security.blocked_commands || []).join('\n'),

    // Clipboard（复制即记忆；默认关闭 opt-in）
    clipboard_enabled: c.clipboard?.enabled ?? defaults.clipboard_enabled,
    clipboard_auto_capture: c.clipboard?.auto_capture ?? defaults.clipboard_auto_capture,
    clipboard_prompt_confirm: c.clipboard?.prompt_confirm ?? defaults.clipboard_prompt_confirm,

    // Memory
    max_session_memory: memory.max_session_memory ?? defaults.max_session_memory,
    max_long_term_memory: memory.max_long_term_memory ?? defaults.max_long_term_memory,
    importance_threshold: memory.importance_threshold ?? defaults.importance_threshold,
    auto_consolidation: memory.auto_consolidation ?? defaults.auto_consolidation,
    consolidation_interval: memory.consolidation_interval ?? defaults.consolidation_interval,
    decay_rate: memory.decay_rate ?? defaults.decay_rate,

    // Retrieval
    retrieval_top_k: retrieval.default_top_k ?? defaults.retrieval_top_k,
    min_score: retrieval.min_score ?? defaults.min_score,
    two_stage_retrieval: retrieval.two_stage_retrieval ?? defaults.two_stage_retrieval,
    l0_multiplier: retrieval.l0_multiplier ?? defaults.l0_multiplier,
    max_context_tokens: retrieval.max_context_tokens ?? defaults.max_context_tokens,
    enable_cache: retrieval.enable_cache ?? defaults.enable_cache,
    cache_ttl: retrieval.cache_ttl ?? defaults.cache_ttl,

    // MCP
    mcpServers: (c.mcp?.servers || []).map((s) => ({
      name: s.name,
      command: s.command,
      args: s.args || [],
      env: s.env || {},
      enabled: s.enabled,
      description: s.description || '',
    })),
  };
}
