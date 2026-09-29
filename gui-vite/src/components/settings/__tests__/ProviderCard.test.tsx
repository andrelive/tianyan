/**
 * ProviderCard 预置徽章测试（ADR-046）：命中预置的 provider 在名称输入框
 * 左侧内嵌徽标（与输入框融合），悬浮显示预置名——回答「用户怎么知道这个
 * 是内置的」的可视性诉求；未命中不展示。
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
  it('fuses the preset badge into the name input (hover reveals the preset name)', () => {
    renderCard({
      'opencode-go': {
        preset_id: 'opencode-go',
        display_name: 'OpenCode Go',
        dynamic_headers: ['x-opencode-session'],
        endpoint_from_preset: true,
      },
    });
    // 内嵌徽标（名称输入框左侧），不是输入框下方独立一行
    expect(screen.getByLabelText('内置预置：OpenCode Go')).toBeInTheDocument();
    // 输入框加左内边距给徽标让位
    expect(screen.getByPlaceholderText('openai')).toHaveClass('pl-8');
    // 悬浮提示：默认隐藏（hover 经 CSS 变体显示）
    const tooltip = screen.getByRole('tooltip');
    expect(tooltip).toHaveClass('hidden');
    expect(tooltip).toHaveTextContent('内置预置：OpenCode Go');
    // 不暴露底层实现细节（会话头注入 / 端点来源）
    expect(screen.queryByText(/x-opencode-session/)).not.toBeInTheDocument();
    expect(screen.queryByText(/端点来自预置/)).not.toBeInTheDocument();
  });

  it('shows no badge for a plain custom provider', () => {
    renderCard({}, makeProvider({ name: 'my-gateway' }));
    expect(screen.queryByLabelText(/内置预置/)).not.toBeInTheDocument();
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
    expect(screen.getByPlaceholderText('openai')).not.toHaveClass('pl-8');
  });
});
