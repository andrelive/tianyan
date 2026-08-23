import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { useAppStore } from '@/lib/store';
import ThinkingSelect from '@/components/chat/ThinkingSelect';

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('ThinkingSelect', () => {
  it('renders nothing when the current model declares no reasoning efforts', () => {
    useAppStore.setState({
      selectedModel: 'plain-model',
      chatModels: [{ name: 'plain-model', provider: 'x', capabilities: ['chat'] }],
    });
    const { container } = render(<ThinkingSelect />);
    expect(container.firstChild).toBeNull();
  });

  it('lists 关闭 plus declared efforts and applies the selection to the store', () => {
    useAppStore.setState({
      selectedModel: 'thinking-model',
      chatModels: [
        {
          name: 'thinking-model',
          provider: 'x',
          capabilities: ['chat'],
          reasoning_efforts: ['low', 'high', 'max'],
        },
      ],
    });
    render(<ThinkingSelect />);
    fireEvent.click(screen.getByRole('button', { name: /思考强度/ }));
    expect(screen.getByRole('option', { name: '关闭' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'low' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'high' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'max' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('option', { name: 'high' }));
    expect(useAppStore.getState().thinkingEffort).toBe('high');
  });

  it('resets to 关闭 when the selected effort is not in the new model set', () => {
    useAppStore.setState({
      selectedModel: 'model-a',
      thinkingEffort: 'max',
      chatModels: [
        { name: 'model-a', provider: 'x', capabilities: ['chat'], reasoning_efforts: ['low'] },
      ],
    });
    render(<ThinkingSelect />);
    expect(useAppStore.getState().thinkingEffort).toBe('off');
  });
});
