/**
 * config-transform 边界测试（F3）。
 *
 * contract.test.ts 已用真实后端快照覆盖主路径往返；本文件补齐
 * 边界/容错路径：缺失分节、空 provider、null 偏好、空 api_key、
 * 模型可选字段省略、MCP env/description、splitLines 分隔符。
 */

import { describe, it, expect } from 'vitest';
import {
  emptyConfigState,
  fromBackendConfig,
  toBackendConfig,
  type BackendConfigResponse,
  type BackendUpdateRequest,
} from '@/lib/config-transform';
import type { ConfigState } from '@/lib/types';

function makeResponse(partial: unknown): BackendConfigResponse {
  return { config: partial as BackendConfigResponse['config'] };
}

describe('fromBackendConfig edge cases', () => {
  it('tolerates a completely empty config object (all sections fall back to defaults)', () => {
    const state = fromBackendConfig(makeResponse({}));
    const defaults = emptyConfigState();
    expect(state.providers).toEqual([]);
    expect(state.preferences).toEqual(defaults.preferences);
    expect(state.working_directory).toBe(defaults.working_directory);
    expect(state.log_level).toBe('info');
    expect(state.safety_mode).toBe('strict');
    expect(state.mcpServers).toEqual([]);
    expect(state.resolvedSpecs).toEqual({});
  });

  it('falls back per-section when a section is missing', () => {
    const state = fromBackendConfig(makeResponse({ models: { providers: [], preferences: {} } }));
    expect(state.providers).toEqual([]);
    expect(state.default_top_k).toBe(5);
    expect(state.data_dir).toBe('');
    expect(state.retrieval_top_k).toBe(10);
  });

  it('normalizes null provider fields to empty strings', () => {
    const state = fromBackendConfig(
      makeResponse({
        models: {
          providers: [
            {
              name: null,
              endpoint: null,
              api_key: null,
              models: [{ name: null, capabilities: null }],
              timeout: null,
              enabled: null,
            },
          ],
          preferences: {},
        },
      }),
    );
    expect(state.providers[0].name).toBe('');
    expect(state.providers[0].endpoint).toBe('');
    expect(state.providers[0].api_key).toBe('');
    expect(state.providers[0].models[0].name).toBe('');
    expect(state.providers[0].models[0].capabilities).toEqual([]);
    expect(state.providers[0].timeout).toBe(60);
    expect(state.providers[0].enabled).toBe(true);
  });

  it('omits empty reasoning_efforts but keeps non-empty ones', () => {
    const withEfforts = fromBackendConfig(
      makeResponse({
        models: {
          providers: [
            {
              name: 'p',
              models: [
                { name: 'a', reasoning_efforts: ['low', 'high'] },
                { name: 'b', reasoning_efforts: [] },
                { name: 'c' },
              ],
            },
          ],
          preferences: {},
        },
      }),
    );
    expect(withEfforts.providers[0].models[0].reasoning_efforts).toEqual(['low', 'high']);
    expect(withEfforts.providers[0].models[1].reasoning_efforts).toBeUndefined();
    expect(withEfforts.providers[0].models[2].reasoning_efforts).toBeUndefined();
  });

  it('converts null preferences into null refs', () => {
    const state = fromBackendConfig(
      makeResponse({
        models: { providers: [], preferences: { chat: null, embedding: null, vision: null } },
      }),
    );
    expect(state.preferences.chat).toBeNull();
    expect(state.preferences.embedding).toBeNull();
    expect(state.preferences.vision).toBeNull();
  });

  it('joins security lists with newlines and keeps empty lists empty', () => {
    const state = fromBackendConfig(
      makeResponse({
        security: {
          allowed_directories: ['C:/a', 'C:/b'],
          blocked_directories: [],
          allowed_commands: ['git status'],
          blocked_commands: [],
        },
      }),
    );
    expect(state.allowed_directories).toBe('C:/a\nC:/b');
    expect(state.blocked_directories).toBe('');
    expect(state.allowed_commands).toBe('git status');
    expect(state.blocked_commands).toBe('');
  });

  it('maps MCP servers with env and description and defaults missing fields', () => {
    const state = fromBackendConfig(
      makeResponse({
        mcp: {
          servers: [
            { name: 's1', command: 'npx', args: ['-y'], env: { A: '1' }, enabled: true },
            { name: 's2', command: 'echo', enabled: false },
          ],
        },
      }),
    );
    expect(state.mcpServers).toHaveLength(2);
    expect(state.mcpServers[0].env).toEqual({ A: '1' });
    expect(state.mcpServers[0].description).toBe('');
    expect(state.mcpServers[1].args).toEqual([]);
    expect(state.mcpServers[1].env).toEqual({});
    expect(state.mcpServers[1].enabled).toBe(false);
  });

  it('keeps thinking_field (transport thinking dialect) through from→to', () => {
    // 回归锁定：thinking_field 是嗅探不到网关的逃生门，白名单映射漏掉它会
    // 在「保存设置」时被静默丢弃（PUT 全量替换配置）。
    const state = fromBackendConfig(
      makeResponse({
        models: {
          providers: [
            { name: 'gw', endpoint: 'https://gw.corp.example/v1', thinking_field: 'reasoning' },
            {
              name: 'ds',
              endpoint: 'https://api.deepseek.com/v1',
              thinking_field: 'reasoning_content',
            },
            { name: 'auto', endpoint: 'https://ollama.com/v1' },
          ],
          preferences: {},
        },
      }),
    );
    expect(state.providers[0].thinking_field).toBe('reasoning');
    expect(state.providers[1].thinking_field).toBe('reasoning_content');
    expect(state.providers[2].thinking_field).toBeUndefined();

    const req = toBackendConfig(state) as BackendUpdateRequest;
    const emitted = req.config.models.providers;
    expect(emitted[0].thinking_field).toBe('reasoning');
    expect(emitted[1].thinking_field).toBe('reasoning_content');
    // 缺省不序列化（不污染用户配置文件）
    expect('thinking_field' in emitted[2]).toBe(false);
  });

  it('keeps model_specs and model_catalog passthrough', () => {
    const state = fromBackendConfig({
      config: { models: { providers: [], preferences: {} } },
      model_specs: { 'p/m': { context_length: 100, max_output_tokens: 50, max_input_tokens: 10 } },
      model_catalog: { 'p/m': { provider: 'p', model: 'm' } as never },
    } as unknown as BackendConfigResponse);
    expect(state.resolvedSpecs['p/m'].context_length).toBe(100);
    expect(state.modelCatalog['p/m']).toBeDefined();
  });
});

describe('toBackendConfig edge cases', () => {
  function stateWith(overrides: Partial<ConfigState>): ConfigState {
    return { ...emptyConfigState(), ...overrides };
  }

  it('serializes empty api_key as null', () => {
    const req = toBackendConfig(
      stateWith({
        providers: [
          {
            name: 'p',
            endpoint: 'http://x',
            api_key: '',
            models: [],
            timeout: 60,
            enabled: true,
            is_local: false,
            headers: {},
          },
        ],
      }),
    ) as BackendUpdateRequest;
    expect(req.config.models.providers[0].api_key).toBeNull();
  });

  it('omits model optional fields when undefined and emits them when set', () => {
    const req = toBackendConfig(
      stateWith({
        providers: [
          {
            name: 'p',
            endpoint: 'http://x',
            api_key: 'k',
            models: [
              { name: 'a', capabilities: ['chat'] as never },
              {
                name: 'b',
                capabilities: ['chat'] as never,
                reasoning_efforts: ['low'],
                context_length: 100,
                max_output_tokens: 50,
                max_input_tokens: 10,
              },
            ],
            timeout: 60,
            enabled: true,
            is_local: false,
            headers: {},
          },
        ],
      }),
    ) as BackendUpdateRequest;
    const [a, b] = req.config.models.providers[0].models;
    expect('reasoning_efforts' in a).toBe(false);
    expect('context_length' in a).toBe(false);
    expect(b.reasoning_efforts).toEqual(['low']);
    expect(b.context_length).toBe(100);
    expect(b.max_output_tokens).toBe(50);
    expect(b.max_input_tokens).toBe(10);
  });

  it('maps null preference refs to null and partial refs to null', () => {
    const req = toBackendConfig(
      stateWith({
        preferences: {
          chat: null,
          embedding: { provider: '', model: 'm' },
          vision: { provider: 'p', model: 'm' },
        },
      }),
    ) as BackendUpdateRequest;
    expect(req.config.models.preferences.chat).toBeNull();
    expect(req.config.models.preferences.embedding).toBeNull();
    expect(req.config.models.preferences.vision).toEqual({ provider: 'p', model: 'm' });
  });

  it('splits textarea lists on newlines and commas, trimming empties', () => {
    const req = toBackendConfig(
      stateWith({
        allowed_directories: 'C:/a\nC:/b, C:/c\n\n',
        blocked_commands: 'rm -rf /,del /s',
      }),
    ) as BackendUpdateRequest;
    expect(req.config.security.allowed_directories).toEqual(['C:/a', 'C:/b', 'C:/c']);
    expect(req.config.security.blocked_commands).toEqual(['rm -rf /', 'del /s']);
  });

  it('serializes MCP servers with env and description only when present', () => {
    const req = toBackendConfig(
      stateWith({
        mcpServers: [
          {
            name: 's1',
            command: 'npx',
            args: ['-y'],
            env: { A: '1' },
            enabled: true,
            description: 'd',
          },
          { name: 's2', command: 'echo', args: [], env: {}, enabled: false, description: '' },
        ],
      }),
    ) as BackendUpdateRequest;
    const [s1, s2] = req.config.mcp.servers;
    expect(s1.env).toEqual({ A: '1' });
    expect(s1.description).toBe('d');
    expect(s2.env).toEqual({});
    expect(s2.description).toBeUndefined();
    expect(s2.args).toEqual([]);
  });

  it('round-trips edge state: from(empty) → to → from is stable', () => {
    const first = fromBackendConfig(makeResponse({}));
    const emitted = toBackendConfig(first).config;
    const second = fromBackendConfig({ config: emitted });
    expect(second).toEqual(first);
  });
});
