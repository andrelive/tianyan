/**
 * 后端契约快照测试（C5：DTO 漂移防线）。
 *
 * fixture 目录 src/test/fixtures/contract/ 存放**真实后端**（127.0.0.1:3000）
 * 抓取的响应快照（脱敏：api_key 已 REDACTED）。本测试用快照驱动
 * config-transform 的映射，防三类漂移：
 * 1. 字段改名（曾发生：learned_rules_top_k ↔ loaded_rules_top_k 导致
 *    保存丢设置——toBackendConfig 写错字段名，serde 静默忽略）；
 * 2. 后端新增字段被前端丢弃（曾发生：shortlist_tools / safety_mode 等
 *    在「保存设置」时整块重建配置被抹掉）；
 * 3. 往返不稳定（from→to 后值漂移）。
 *
 * 刷新快照：启动后端后运行
 *   scripts/capture-contract-fixtures.ps1
 */

import { describe, it, expect } from 'vitest';
import rawConfig from '../../test/fixtures/contract/config.json';
import {
  fromBackendConfig,
  toBackendConfig,
  type BackendConfigResponse,
} from '@/lib/config-transform';

const response = rawConfig as unknown as BackendConfigResponse;

describe('backend config contract snapshot', () => {
  it('maps the captured backend response without loss', () => {
    const state = fromBackendConfig(response);
    // 字段名契约：后端发 learned_rules_top_k（曾漂移为 loaded_rules_top_k）
    expect(response.config.agent.learned_rules_top_k).toBe(state.learned_rules_top_k);
    // 后端新增字段透传（不丢）
    expect(state.shortlist_tools).toBe(response.config.agent.shortlist_tools ?? true);
    expect(state.background_self_review).toBe(
      response.config.agent.background_self_review ?? false,
    );
    expect(state.storage_backend).toBe(response.config.storage.backend ?? 'sqlite');
    expect(state.safety_mode).toBe(response.config.security.safety_mode ?? 'strict');
  });

  it('round-trips the captured config without data loss (save preserves unknown fields)', () => {
    const state = fromBackendConfig(response);
    const emitted = toBackendConfig(state).config;

    // 保存时后端字段名必须正确（曾错写 loaded_rules_top_k 被 serde 静默丢弃）
    expect(emitted.agent.learned_rules_top_k).toBe(state.learned_rules_top_k);
    expect(emitted.agent.shortlist_tools).toBe(state.shortlist_tools);
    expect(emitted.agent.background_self_review).toBe(state.background_self_review);
    expect(emitted.storage.backend).toBe(state.storage_backend);
    expect(emitted.security.safety_mode).toBe(state.safety_mode);
  });

  it('is stable after a second from/to cycle', () => {
    const first = toBackendConfig(fromBackendConfig(response)).config;
    const second = toBackendConfig(fromBackendConfig({ config: first })).config;
    expect(second).toEqual(first);
  });
});
