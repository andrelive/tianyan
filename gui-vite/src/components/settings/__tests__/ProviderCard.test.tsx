/**
 * ProviderCard 预置徽章测试（ADR-046）：命中预置的 provider 展示"内置预置"
 * 只读徽章（含自动注入的动态头 / 端点来源），未命中不展示——回答
 * 「用户怎么知道这个是内置的」的可视性诉求。
 */
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import ProviderCard from '../tabs/ProviderCard';
import type { ProviderConfigState, ProviderPresetHit } from '@/lib/types';

const noop = () => undefined;

function makeProvider(overrides: Partial<ProviderConfigState> = {}): ProviderConfigState {
  return {
    name: 'opencode-go',
    endpoint: 'https://opencode.ai/zen/go/v1',
    api_key: 'sk-test',
    models: [],
    timeout: 60,
    enabled: true,
    is_local: false,
    headers: {},
    ...overrides,
  };
}

function renderCard(presets: Record<string, ProviderPresetHit>, provider = makeProvider()) {
  return render(
    <ProviderCard
      provider={provider}
      collapsed={true}
      onToggleCollapsed={noop}
      testStatus="idle"
      scanState={{ protocol: 'openai', scanning: false, models: [], picked: [], error: null }}
      resolvedSpecs={{}}
      modelCatalog={{}}
      resolvedEndpoints={{}}
      providerPresets={presets}
      onUpdateProvider={noop}
      onRemoveProvider={noop}
      onTestConnection={noop}
      onScan={noop}
      onSetScanProtocol={noop}
      onTogglePicked={noop}
      onAdoptPicked={noop}
      onAddModel={noop}
      onRemoveModel={noop}
      onUpdateModel={noop}
      onToggleModelCapability={noop}
    />,
  );
}

describe('ProviderCard preset badge (ADR-046)', () => {
  it('shows 内置预置 badge with auto-injected headers when preset is hit', () => {
    renderCard({
      'opencode-go': {
        preset_id: 'opencode-go',
        display_name: 'OpenCode Go',
        dynamic_headers: ['x-opencode-session'],
        endpoint_from_preset: false,
      },
    });
    expect(screen.getByText(/内置预置：OpenCode Go/)).toBeInTheDocument();
    expect(screen.getByText(/自动注入 x-opencode-session/)).toBeInTheDocument();
  });

  it('shows no badge for a plain custom provider', () => {
    renderCard({}, makeProvider({ name: 'my-gateway' }));
    expect(screen.queryByText(/内置预置/)).not.toBeInTheDocument();
  });

  it('annotates endpoint-from-preset when configured endpoint is empty', () => {
    renderCard(
      {
        'opencode-go': {
          preset_id: 'opencode-go',
          display_name: 'OpenCode Go',
          dynamic_headers: [],
          endpoint_from_preset: true,
        },
      },
      makeProvider({ endpoint: '' }),
    );
    expect(screen.getByText(/端点来自预置/)).toBeInTheDocument();
  });
});
