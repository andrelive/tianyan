import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import ChatInput from '../ChatInput';

/**
 * 桌面壳桥接的可控替身：默认浏览器语义（拿不到绝对路径、无原生拖放）；
 * 用例内切到桌面语义即可验证文件引用通道的交互。
 * （用 vi.hoisted：vi.mock 工厂会被提升到 import 之前执行。）
 */
const tauri = vi.hoisted(() => ({
  desktop: false,
  dropCallbacks: [] as Array<(paths: string[]) => void>,
  pickFiles: vi.fn<() => Promise<string[]>>(async () => []),
  // 剪贴板文件通道（粘贴 = 资源管理器 Ctrl+C 的文件）：默认「原生没看到文件」
  readClipboardFiles: vi.fn<() => Promise<string[]>>(async () => [] as string[]),
}));
vi.mock('@/lib/tauri', () => ({
  isTauri: () => tauri.desktop,
  onFileDrop: async (cb: (paths: string[]) => void) => {
    tauri.dropCallbacks.push(cb);
    return () => {
      const i = tauri.dropCallbacks.indexOf(cb);
      if (i >= 0) tauri.dropCallbacks.splice(i, 1);
    };
  },
  pickFiles: () => tauri.pickFiles(),
  readClipboardFiles: () => tauri.readClipboardFiles(),
  pickDirectory: async () => null,
  openExternal: async () => false,
}));

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
  tauri.desktop = false;
  tauri.dropCallbacks.length = 0;
  tauri.pickFiles.mockReset();
  tauri.pickFiles.mockResolvedValue([]);
  tauri.readClipboardFiles.mockReset();
  tauri.readClipboardFiles.mockResolvedValue([]);
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
    // 第三个参数 = 文件引用通道（本用例无附件 → 空数组）
    expect(props.onSend).toHaveBeenCalledWith('你好', [], []);
    expect(screen.getByLabelText('输入消息')).toHaveValue('');
  });

  it('sends on Enter and keeps newline on Shift+Enter', async () => {
    const { props } = renderChatInput();
    const textarea = screen.getByLabelText('输入消息');

    fireEvent.change(textarea, { target: { value: '第一行' } });
    fireEvent.keyDown(textarea, { key: 'Enter', shiftKey: true });
    expect(props.onSend).not.toHaveBeenCalled();

    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(props.onSend).toHaveBeenCalledWith('第一行', [], []);
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
    expect(props.onSend).toHaveBeenCalledWith('看图', ['data:image/png;base64,dGVzdA=='], []);
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

/**
 * 文件引用通道（批次 1）：非图片文件 → 带图标的文件名 chip，发给后端的是
 * 绝对路径；图片通道（位图 data URL）行为不变，两条通道互不干扰。
 */
describe('ChatInput / 文件引用通道', () => {
  /** 等组件挂上原生拖放订阅（effect 内是异步的）。 */
  async function waitForDropSubscription() {
    await waitFor(() => expect(tauri.dropCallbacks.length).toBeGreaterThan(0));
    return tauri.dropCallbacks[tauri.dropCallbacks.length - 1];
  }

  it('桌面版：原生拖放的绝对路径渲染成文件名 chip（完整路径只在 title）', async () => {
    tauri.desktop = true;
    const { props } = renderChatInput();
    const drop = await waitForDropSubscription();

    act(() => drop(['C:\\repo\\src\\main.ts', '/home/u/notes.md']));

    const chips = await screen.findAllByTestId('file-chip');
    expect(chips).toHaveLength(2);
    // chip 文本只有文件名（长路径会把输入框撑开）
    expect(chips[0]).toHaveTextContent('main.ts');
    expect(chips[1]).toHaveTextContent('notes.md');
    expect(chips[0]).not.toHaveTextContent('C:\\repo');
    // 完整路径在悬停提示里
    expect(chips[0]).toHaveAttribute('title', 'C:\\repo\\src\\main.ts');

    // 只有附件、没有文字也能发（附件并入正文由 ChatPanel 负责）
    await userEvent.setup().click(screen.getByRole('button', { name: '发送消息' }));
    expect(props.onSend).toHaveBeenCalledWith(
      '',
      [],
      [
        { path: 'C:\\repo\\src\\main.ts', name: 'main.ts' },
        { path: '/home/u/notes.md', name: 'notes.md' },
      ],
    );
  });

  it('重复拖放同一文件不堆 chip；× 逐个删除后草稿随之清空', async () => {
    tauri.desktop = true;
    renderChatInput();
    const drop = await waitForDropSubscription();

    act(() => drop(['C:\\a\\b.ts']));
    act(() => drop(['C:\\a\\b.ts']));

    expect(await screen.findAllByTestId('file-chip')).toHaveLength(1);

    fireEvent.click(screen.getByRole('button', { name: '移除文件 b.ts' }));
    await waitFor(() => expect(screen.queryAllByTestId('file-chip')).toHaveLength(0));
    // 空草稿不残留键（与文本/图片同一条删除规则）
    expect(useAppStore.getState().inputDrafts).toEqual({});
  });

  it('桌面版：「添加文件」走原生多选对话框并追加 chip', async () => {
    tauri.desktop = true;
    tauri.pickFiles.mockResolvedValue(['F:\\work\\tianyan\\README.md']);
    const user = userEvent.setup();
    renderChatInput();

    await user.click(screen.getByRole('button', { name: '添加文件' }));

    const chip = await screen.findByTestId('file-chip');
    expect(chip).toHaveTextContent('README.md');
    expect(chip).toHaveAttribute('title', 'F:\\work\\tianyan\\README.md');
  });

  it('浏览器环境：没有「添加文件」入口，非图片文件提示指向「贴路径」而非静默忽略', async () => {
    renderChatInput();
    expect(screen.queryByRole('button', { name: '添加文件' })).not.toBeInTheDocument();

    fireEvent.change(fileInput(), {
      target: { files: [makeTypedFile('源码.rs', 'text/plain')] },
    });

    await waitFor(() => {
      const alert = screen.getByRole('alert');
      expect(alert).toHaveTextContent('浏览器环境拿不到文件绝对路径');
      expect(alert).toHaveTextContent('直接把路径贴到输入框');
    });
  });

  it('文件与图片并存：两条通道各自独立传递（图片仍是 data URL）', async () => {
    tauri.desktop = true;
    const user = userEvent.setup();
    const { props } = renderChatInput();
    const drop = await waitForDropSubscription();

    fireEvent.change(fileInput(), { target: { files: [makeImageFile('a.png')] } });
    await waitFor(() => expect(screen.getByAltText('待发送图片 1')).toBeInTheDocument());
    act(() => drop(['C:\\a\\b.ts']));
    await waitFor(() => expect(screen.getAllByTestId('file-chip')).toHaveLength(1));

    await user.type(screen.getByLabelText('输入消息'), '看这两个');
    await user.click(screen.getByRole('button', { name: '发送消息' }));

    expect(props.onSend).toHaveBeenCalledWith(
      '看这两个',
      ['data:image/png;base64,dGVzdA=='],
      [{ path: 'C:\\a\\b.ts', name: 'b.ts' }],
    );
  });
});

/**
 * 粘贴分流（本功能主路径）：资源管理器 Ctrl+C 复制文件 → 聊天框 Ctrl+V。
 *
 * 关键前提：截图与「复制的文件」在 Web 层同形（都是 `kind: 'file'`，
 * Win+Shift+S 的 MIME 也是 `image/png`），前端**不能**按 MIME 分流，只能
 * 「先问原生有没有文件路径（CF_HDROP），没有才回退图片通道」。
 */
describe('ChatInput / 粘贴分流（文件 chip vs 截图）', () => {
  /** 构造一个「粘贴事件里有文件项」的 payload（jsdom 无真实剪贴板）。 */
  function pasteWithFile(file: File | null, type = 'application/pdf') {
    const textarea = screen.getByLabelText('输入消息');
    return fireEvent.paste(textarea, {
      clipboardData: {
        items: [{ kind: 'file', type, getAsFile: () => file }],
      },
    });
  }

  it('原生返回文件路径 → 渲染文件 chip（title 含完整路径），不走图片通道', async () => {
    tauri.readClipboardFiles.mockResolvedValue(['C:\\work\\a.pdf']);
    const { props } = renderChatInput();

    pasteWithFile(makeTypedFile('a.pdf', 'application/pdf'));

    const chip = await screen.findByTestId('file-chip');
    expect(chip).toHaveTextContent('a.pdf');
    expect(chip).toHaveAttribute('title', 'C:\\work\\a.pdf');
    // 没走图片通道（addImages 未被调用 → 无图片预览）
    expect(screen.queryByAltText(/待发送图片/)).not.toBeInTheDocument();

    // 真正发给模型的是绝对路径（不是二进制）
    await userEvent.setup().click(screen.getByRole('button', { name: '发送消息' }));
    expect(props.onSend).toHaveBeenCalledWith('', [], [{ path: 'C:\\work\\a.pdf', name: 'a.pdf' }]);
  });

  it('原生返回空数组（截图场景）→ 回退图片通道，不出现文件 chip', async () => {
    tauri.readClipboardFiles.mockResolvedValue([]);
    renderChatInput();

    // 截图：MIME 同样是 image/png，与「复制的 png 文件」在 Web 层无法区分
    pasteWithFile(makeImageFile('image.png'), 'image/png');

    await waitFor(() => expect(screen.getByAltText('待发送图片 1')).toBeInTheDocument());
    expect(screen.queryByTestId('file-chip')).not.toBeInTheDocument();
  });

  it('原生读取失败（reject）→ 静默回退图片通道，不抛错也不丢粘贴', async () => {
    tauri.readClipboardFiles.mockRejectedValue(new Error('ACL denied'));
    renderChatInput();

    pasteWithFile(makeImageFile('image.png'), 'image/png');

    // 不能因为读剪贴板失败就让粘贴「完全没反应」
    await waitFor(() => expect(screen.getByAltText('待发送图片 1')).toBeInTheDocument());
    expect(screen.queryByTestId('file-chip')).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('纯文本粘贴：不查剪贴板文件、不拦截默认行为', () => {
    renderChatInput();

    const notCancelled = fireEvent.paste(screen.getByLabelText('输入消息'), {
      clipboardData: {
        items: [{ kind: 'string', type: 'text/plain', getAsFile: () => null }],
        getData: () => '普通文本',
      },
    });

    // fireEvent 返回 !defaultPrevented：true = 没拦截（走默认粘贴）
    expect(notCancelled).toBe(true);
    expect(tauri.readClipboardFiles).not.toHaveBeenCalled();
  });

  it('超过软上限：只保留 20 个文件并提示「最多添加 20 个文件」', async () => {
    const many = Array.from({ length: 25 }, (_, i) => `C:\\work\\f${i}.txt`);
    tauri.readClipboardFiles.mockResolvedValue(many);
    renderChatInput();

    pasteWithFile(makeTypedFile('f0.txt', 'text/plain'));

    await waitFor(() => expect(screen.getAllByTestId('file-chip')).toHaveLength(20));
    expect(screen.getByRole('alert')).toHaveTextContent('最多添加 20 个文件');
  });
});
