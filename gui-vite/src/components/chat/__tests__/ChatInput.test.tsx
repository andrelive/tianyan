import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import ChatInput from '../ChatInput';

// jsdom 的 FileReader 不支持 readAsDataURL 结果断言——用受控 mock 驱动图片管线
class MockFileReader {
  result: string | null = null;
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  readAsDataURL() {
    this.result = 'data:image/png;base64,dGVzdA==';
    this.onload?.();
  }
}

function makeImageFile(name: string, size = 1024): File {
  return new File([new Uint8Array(size)], name, { type: 'image/png' });
}

function renderChatInput(overrides: Partial<Parameters<typeof ChatInput>[0]> = {}) {
  const props = {
    onSend: vi.fn(),
    onStop: vi.fn(),
    isStreaming: false,
    usage: null,
    sessionUsage: null,
    onCompress: vi.fn(),
    ...overrides,
  };
  const utils = render(
    <MemoryRouter>
      <ChatInput {...props} />
    </MemoryRouter>,
  );
  return { ...utils, props };
}

function fileInput(): HTMLInputElement {
  const el = document.querySelector('input[type="file"]');
  if (!el) throw new Error('file input not found');
  return el as HTMLInputElement;
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
  vi.stubGlobal('FileReader', MockFileReader);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('ChatInput', () => {
  it('disables send when empty and sends typed content on click', async () => {
    const user = userEvent.setup();
    const { props } = renderChatInput();

    expect(screen.getByRole('button', { name: '发送消息' })).toBeDisabled();

    await user.type(screen.getByLabelText('输入消息'), '你好');
    expect(screen.getByRole('button', { name: '发送消息' })).toBeEnabled();

    await user.click(screen.getByRole('button', { name: '发送消息' }));
    expect(props.onSend).toHaveBeenCalledWith('你好', []);
    expect(screen.getByLabelText('输入消息')).toHaveValue('');
  });

  it('sends on Enter and keeps newline on Shift+Enter', async () => {
    const { props } = renderChatInput();
    const textarea = screen.getByLabelText('输入消息');

    fireEvent.change(textarea, { target: { value: '第一行' } });
    fireEvent.keyDown(textarea, { key: 'Enter', shiftKey: true });
    expect(props.onSend).not.toHaveBeenCalled();

    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(props.onSend).toHaveBeenCalledWith('第一行', []);
  });

  it('switches to stop button while streaming', async () => {
    const user = userEvent.setup();
    const { props } = renderChatInput({ isStreaming: true });

    expect(screen.getByRole('button', { name: '停止生成' })).toBeInTheDocument();
    expect(screen.getByLabelText('输入消息')).toBeDisabled();
    expect(screen.queryByRole('button', { name: '发送消息' })).not.toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '停止生成' }));
    expect(props.onStop).toHaveBeenCalled();
  });

  it('disables send and textarea while compressing', async () => {
    const user = userEvent.setup();
    const { props, rerender } = renderChatInput();

    // 先输入内容（compressing=false 时可输可发）
    await user.type(screen.getByLabelText('输入消息'), '压缩期间的消息');
    expect(screen.getByRole('button', { name: '发送消息' })).toBeEnabled();

    // 压缩开始：rerender 同一输入内容下，发送按钮与 textarea 禁用
    rerender(
      <MemoryRouter>
        <ChatInput
          onSend={props.onSend}
          onStop={props.onStop}
          isStreaming={false}
          usage={null}
          sessionUsage={null}
          onCompress={props.onCompress}
          compressing={true}
        />
      </MemoryRouter>,
    );
    expect(screen.getByRole('button', { name: '发送消息' })).toBeDisabled();
    expect(screen.getByLabelText('输入消息')).toBeDisabled();

    // Enter 也不触发发送
    fireEvent.keyDown(screen.getByLabelText('输入消息'), { key: 'Enter' });
    expect(props.onSend).not.toHaveBeenCalled();

    // 压缩结束：恢复可发
    rerender(
      <MemoryRouter>
        <ChatInput
          onSend={props.onSend}
          onStop={props.onStop}
          isStreaming={false}
          usage={null}
          sessionUsage={null}
          onCompress={props.onCompress}
          compressing={false}
        />
      </MemoryRouter>,
    );
    expect(screen.getByRole('button', { name: '发送消息' })).toBeEnabled();
    expect(screen.getByLabelText('输入消息')).toBeEnabled();
  });

  it('adds image via file input and sends it with content', async () => {
    const user = userEvent.setup();
    const { props } = renderChatInput();

    fireEvent.change(fileInput(), { target: { files: [makeImageFile('a.png')] } });
    await waitFor(() => {
      expect(screen.getByAltText('待发送图片 1')).toBeInTheDocument();
    });

    await user.type(screen.getByLabelText('输入消息'), '看图');
    await user.click(screen.getByRole('button', { name: '发送消息' }));
    expect(props.onSend).toHaveBeenCalledWith('看图', ['data:image/png;base64,dGVzdA==']);
  });

  it('rejects oversized images (>4MB) with an alert and no preview', async () => {
    renderChatInput();

    fireEvent.change(fileInput(), {
      target: { files: [makeImageFile('big.png', 5 * 1024 * 1024)] },
    });
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent('图片过大（超过 4MB）');
    });
    expect(screen.queryByAltText('待发送图片 1')).not.toBeInTheDocument();
  });

  it('caps at 4 images and reports the rest', async () => {
    renderChatInput();

    fireEvent.change(fileInput(), {
      target: {
        files: [
          makeImageFile('1.png'),
          makeImageFile('2.png'),
          makeImageFile('3.png'),
          makeImageFile('4.png'),
          makeImageFile('5.png'),
        ],
      },
    });
    await waitFor(() => {
      expect(screen.getAllByAltText(/待发送图片/)).toHaveLength(4);
    });
    expect(screen.getByRole('alert')).toHaveTextContent('最多上传 4 张图片');
  });

  it('renders session token summary with cache hit rate', () => {
    renderChatInput({
      sessionUsage: { uncachedInput: 1200, cachedInput: 800, completion: 500 },
    });

    expect(screen.getByText('未命中输入')).toBeInTheDocument();
    expect(screen.getByText('1,200')).toBeInTheDocument();
    expect(screen.getByText('800')).toBeInTheDocument();
    expect(screen.getByText('500')).toBeInTheDocument();
    // 800 / (1200+800) = 40%
    expect(screen.getByText('40%')).toBeInTheDocument();
  });
});
