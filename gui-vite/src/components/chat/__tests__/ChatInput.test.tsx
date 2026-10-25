import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';
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

/** 任意类型文件（用于验证类型白名单；内容不重要）。 */
function makeTypedFile(name: string, type: string, size = 128): File {
  return new File([new Uint8Array(size)], name, { type });
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

  it('keeps the unsent draft after unmount and restores it on remount (page switch)', () => {
    const { unmount } = renderChatInput();
    fireEvent.change(screen.getByLabelText('输入消息'), { target: { value: '未发送的草稿' } });
    expect(screen.getByLabelText('输入消息')).toHaveValue('未发送的草稿');

    // 模拟切走页面（组件卸载）再切回：草稿从 store 恢复
    unmount();
    renderChatInput();
    expect(screen.getByLabelText('输入消息')).toHaveValue('未发送的草稿');
  });

  it('isolates drafts per session and restores each on session switch', () => {
    useAppStore.setState({ currentSessionId: 'session-a' });
    renderChatInput();
    fireEvent.change(screen.getByLabelText('输入消息'), { target: { value: 'A 的草稿' } });

    // 切到会话 B：显示 B 自己的草稿（空）——A 的草稿不串台
    act(() => useAppStore.setState({ currentSessionId: 'session-b' }));
    expect(screen.getByLabelText('输入消息')).toHaveValue('');

    fireEvent.change(screen.getByLabelText('输入消息'), { target: { value: 'B 的草稿' } });

    // 切回会话 A：恢复 A 的草稿
    act(() => useAppStore.setState({ currentSessionId: 'session-a' }));
    expect(screen.getByLabelText('输入消息')).toHaveValue('A 的草稿');
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

  it('rejects SVG with an actionable hint instead of sending it as an image', async () => {
    // 前车之鉴：SVG 的 MIME 是 `image/svg+xml`，此前 `image/*` 过滤会放过它，
    // 读成 data URL 发给模型 → 三家 provider 全部 400。必须在入口拦下并指路。
    renderChatInput();

    fireEvent.change(fileInput(), {
      target: { files: [makeTypedFile('架构图.svg', 'image/svg+xml')] },
    });

    await waitFor(() => {
      const alert = screen.getByRole('alert');
      expect(alert).toHaveTextContent('不支持的文件类型');
      expect(alert).toHaveTextContent('架构图.svg');
      expect(alert).toHaveTextContent('read_file'); // 给出可操作出路
    });
    expect(screen.queryByAltText('待发送图片 1')).not.toBeInTheDocument();
  });

  it('keeps accepted images while reporting a rejected sibling (mixed batch)', async () => {
    // 混选（PNG + SVG）时不能整批丢掉：合法图片照常进预览，非法项单独提示。
    renderChatInput();

    fireEvent.change(fileInput(), {
      target: {
        files: [makeImageFile('ok.png'), makeTypedFile('note.html', 'text/html')],
      },
    });

    await waitFor(() => {
      expect(screen.getByAltText('待发送图片 1')).toBeInTheDocument();
    });
    expect(screen.getByRole('alert')).toHaveTextContent('note.html');
  });

  it('reports pasted non-image files instead of silently ignoring them', async () => {
    renderChatInput();

    const textarea = screen.getByLabelText('输入消息');
    fireEvent.paste(textarea, {
      clipboardData: {
        items: [
          {
            kind: 'file',
            type: 'image/svg+xml',
            getAsFile: () => makeTypedFile('logo.svg', 'image/svg+xml'),
          },
        ],
      },
    });

    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent('logo.svg');
    });
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

  it('renders the latest request perf (TTFT + tok/s) next to token summary', () => {
    // TTFT 1.2s、总耗时 3.2s、输出 100 token → 生成阶段 2.0s → 50 tok/s
    renderChatInput({
      sessionUsage: { uncachedInput: 1200, cachedInput: 800, completion: 500 },
      requestPerf: { ttftMs: 1200, durationMs: 3200, completionTokens: 100 },
    });

    expect(screen.getByText('TTFT')).toBeInTheDocument();
    expect(screen.getByText('1.2s')).toBeInTheDocument();
    expect(screen.getByText('50')).toBeInTheDocument();
    expect(screen.getByText('tok/s')).toBeInTheDocument();
  });

  it('renders ms TTFT and hides tok/s when there is no output token', () => {
    // 纯工具调用轮：有 TTFT（ms 级）+ 无输出 token → 不展示 tok/s
    renderChatInput({
      sessionUsage: null,
      requestPerf: { ttftMs: 820, durationMs: 2400, completionTokens: 0 },
    });

    expect(screen.getByText('820ms')).toBeInTheDocument();
    expect(screen.queryByText('tok/s')).not.toBeInTheDocument();
  });
});
