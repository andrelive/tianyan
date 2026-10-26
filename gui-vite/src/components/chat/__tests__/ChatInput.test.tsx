import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import ChatInput from '../ChatInput';
// 输入框交互工具（contenteditable 的「打字 / 只读态 / 文本断言」口径集中在这里，
// 与 ChatPanel.test.tsx 共用；理由见文件内注释）
import {
  caretToEnd,
  composerEl,
  composerText,
  draftText,
  expectComposerEditable,
  expectComposerReadOnly,
  pasteTextOntoComposer,
  pressEnter,
  typeIntoComposer,
} from '@/test/composer';

/**
 * 桌面壳桥接的可控替身：默认浏览器语义（拿不到绝对路径）；
 * 用例内切到桌面语义即可验证桌面专属的提示文案。
 * （用 vi.hoisted：vi.mock 工厂会被提升到 import 之前执行。）
 */
const tauri = vi.hoisted(() => ({
  desktop: false,
  // 剪贴板文件通道（粘贴 = 资源管理器 Ctrl+C 的文件）：默认「原生没看到文件」
  readClipboardFiles: vi.fn<() => Promise<string[]>>(async () => [] as string[]),
}));
vi.mock('@/lib/tauri', () => ({
  isTauri: () => tauri.desktop,
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

/**
 * 读不出字节的 FileReader 替身。
 *
 * 模拟 WebView2 对「从资源管理器复制（CF_HDROP）」的文件只给空壳 `File` 的情形：
 * name/size 都在，但 `readAsDataURL` 拿不到内容。jsdom 里只能这样近似——
 * 真实的 CF_HDROP 行为无法在单测中复现（见报告里的局限说明）。
 */
class FailingFileReader {
  result: string | null = null;
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  readAsDataURL() {
    this.result = null;
    this.onerror?.();
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

    await typeIntoComposer('你好');
    expect(screen.getByRole('button', { name: '发送消息' })).toBeEnabled();

    await user.click(screen.getByRole('button', { name: '发送消息' }));
    // 两个参数：content（文件 chip 已在里面）+ images（本用例无图片 → 空数组）
    expect(props.onSend).toHaveBeenCalledWith('你好', []);
    // 发送后编辑器清空（value 同步进 Lexical，文本从 DOM 消失）
    await waitFor(() => expect(composerText()).toBe(''));
    expect(draftText()).toBe('');
  });

  it('sends on Enter and keeps newline on Shift+Enter', async () => {
    const { props } = renderChatInput();

    await typeIntoComposer('第一行');
    expect(composerText()).toBe('第一行');

    // Shift+Enter：不发送，而是插入换行（旧 textarea 里 fireEvent.keyDown 走不到默认
    // 换行，所以旧断言只能验「没发送」；Lexical 下换行是真发生了，断言可以更严）
    await pressEnter({ shiftKey: true });
    expect(props.onSend).not.toHaveBeenCalled();
    expect(draftText()).toBe('第一行\n');

    // Enter：发送，内容含 Shift+Enter 留下的换行（与 textarea 的 value 语义一致）
    await pressEnter();
    expect(props.onSend).toHaveBeenCalledWith('第一行\n', []);
  });

  it('输入法组合期间（isComposing）的 Enter 不发送、也不插换行', async () => {
    // 本步最关键的验证点：contenteditable + 中文输入法的硬门槛。拼音打一半时
    // 按 Enter 是「确认候选词」——若当成发送，就是半句话被发出去 + 拼音被截断。
    const { props } = renderChatInput();
    await typeIntoComposer('nihao');

    // Chromium / WebView2 在确认候选词时派发的 keydown 自报 isComposing
    await pressEnter({ isComposing: true });
    expect(props.onSend).not.toHaveBeenCalled();
    // 也不该插换行（组合中的 Enter 只负责确认候选词）
    expect(draftText()).toBe('nihao');
    expect(draftText()).not.toContain('\n');

    // 组合结束后 Enter 恢复发送（同一段文本）
    await pressEnter();
    expect(props.onSend).toHaveBeenCalledWith('nihao', []);
  });

  it('Lexical 自身的组合态（compositionstart）期间 Enter 同样不发送', async () => {
    // 覆盖「Enter 到达时 DOM 事件没带 isComposing，但编辑器知道自己正在组合」
    // （旧 WebKit / 部分 IME 的形态）——靠 editor.isComposing() 兜住。
    const { props } = renderChatInput();
    await typeIntoComposer('nihao');

    await act(async () => {
      composerEl().dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
    });
    await pressEnter();

    expect(props.onSend).not.toHaveBeenCalled();
    // 只断言「没有插换行」：组合开始时 Lexical 会往文本里放一个占位零宽空格
    // （COMPOSITION_START_CHAR，真实浏览器在 compositionend 时清掉；jsdom 无法
    // 复现完整 IME 生命周期，所以这里不对文本做逐字断言）
    expect(draftText()).not.toContain('\n');
  });

  it('keeps the unsent draft after unmount and restores it on remount (page switch)', async () => {
    const { unmount } = renderChatInput();
    await typeIntoComposer('未发送的草稿');
    expect(composerText()).toBe('未发送的草稿');

    // 模拟切走页面（组件卸载）再切回：草稿从 store 恢复（外部 value 同步进编辑器）
    unmount();
    renderChatInput();
    await waitFor(() => expect(composerText()).toBe('未发送的草稿'));
    expect(draftText()).toBe('未发送的草稿');
  });

  it('isolates drafts per session and restores each on session switch', async () => {
    useAppStore.setState({ currentSessionId: 'session-a' });
    renderChatInput();
    await typeIntoComposer('A 的草稿');
    expect(draftText('session-a')).toBe('A 的草稿');

    // 切到会话 B：显示 B 自己的草稿（空）——A 的草稿不串台
    act(() => useAppStore.setState({ currentSessionId: 'session-b' }));
    await waitFor(() => expect(composerText()).toBe(''));

    await typeIntoComposer('B 的草稿');

    // 切回会话 A：恢复 A 的草稿
    act(() => useAppStore.setState({ currentSessionId: 'session-a' }));
    await waitFor(() => expect(composerText()).toBe('A 的草稿'));
  });

  it('switches to stop button while streaming', async () => {
    const user = userEvent.setup();
    const { props } = renderChatInput({ isStreaming: true });

    expect(screen.getByRole('button', { name: '停止生成' })).toBeInTheDocument();
    expectComposerReadOnly();
    expect(screen.queryByRole('button', { name: '发送消息' })).not.toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '停止生成' }));
    expect(props.onStop).toHaveBeenCalled();
  });

  it('disables send and composer while compressing', async () => {
    const { props, rerender } = renderChatInput();

    // 先输入内容（compressing=false 时可输可发）
    await typeIntoComposer('压缩期间的消息');
    expect(screen.getByRole('button', { name: '发送消息' })).toBeEnabled();

    // 压缩开始：rerender 同一输入内容下，发送按钮与编辑器禁用
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
    expectComposerReadOnly();

    // Enter 也不触发发送
    await pressEnter();
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
    expectComposerEditable();
  });

  it('adds image via file input and sends it with content', async () => {
    const user = userEvent.setup();
    const { props } = renderChatInput();

    fireEvent.change(fileInput(), { target: { files: [makeImageFile('a.png')] } });
    await waitFor(() => {
      expect(screen.getByAltText('待发送图片 1')).toBeInTheDocument();
    });

    await typeIntoComposer('看图');
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

    fireEvent.paste(composerEl(), {
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
 * 文件引用通道：非图片文件 → **内联在文字流里**的 chip（`FileChipNode`），
 * 发给后端的是它序列化后的文本 `` `@file:"<绝对路径>"` ``；图片通道（位图
 * data URL）行为不变，两条通道互不干扰。
 *
 * 入口只有粘贴（资源管理器 Ctrl+C → 输入框 Ctrl+V）：拖拽与文件选择对话框
 * 两个入口已按需求删除，chip 只能经 `readClipboardFiles` 通道产生。
 */
describe('ChatInput / 文件引用通道', () => {
  /**
   * 粘贴一个文件项（jsdom 无真实剪贴板，只能手工构造 payload）。
   * `types`/`files` 是 RichTextPlugin 的 `eventFiles()` 必读项（见 @/test/composer）。
   */
  function pasteFile(file: File) {
    fireEvent.paste(composerEl(), {
      clipboardData: {
        types: ['Files'],
        files: [file],
        items: [{ kind: 'file', type: file.type, getAsFile: () => file }],
      },
    });
  }

  it('粘贴得到绝对路径 → chip 出现在输入框内（不是顶部独立行），重复粘贴不堆 chip', async () => {
    tauri.readClipboardFiles.mockResolvedValue(['/home/u/b.ts']);
    const { props } = renderChatInput();

    pasteFile(makeTypedFile('b.ts', 'text/plain'));
    const chips = await screen.findAllByTestId('file-chip');
    expect(chips).toHaveLength(1);
    // chip 文本只有文件名（长路径会把输入框撑开），完整路径在悬停提示里
    expect(chips[0]).toHaveTextContent('b.ts');
    expect(chips[0]).not.toHaveTextContent('/home/u');
    expect(chips[0]).toHaveAttribute('title', '/home/u/b.ts');
    // ★ 核心诉求：chip 在**编辑器的文字流里**（contenteditable 内部），
    // 而不是草稿区顶部那一行独立 chip 行
    expect(composerEl().contains(chips[0])).toBe(true);

    // 再粘一次同一文件：跨批次按 path 去重，不会堆成两个 chip
    await act(async () => {
      pasteFile(makeTypedFile('b.ts', 'text/plain'));
    });
    expect(screen.getAllByTestId('file-chip')).toHaveLength(1);

    // 纯附件（没有文字）也能发：chip 的序列化文本就是 content
    await userEvent.setup().click(screen.getByRole('button', { name: '发送消息' }));
    // 末尾那个空格是插入 chip 时补的分隔空格（ChatPanel 发送前会 trim）
    expect(props.onSend).toHaveBeenCalledWith('`@file:"/home/u/b.ts"` ', []);
  });

  it('粘贴发生在**光标处**：文字夹着 chip，序列化后是行内 Markdown', async () => {
    tauri.readClipboardFiles.mockResolvedValue(['C:\\a.ts']);
    renderChatInput();

    // 先把光标打到句子里（末尾），再粘贴，然后继续打字
    await typeIntoComposer('看这个 ');
    await act(async () => {
      pasteFile(makeTypedFile('a.ts', 'text/plain'));
    });
    await waitFor(() => expect(screen.getAllByTestId('file-chip')).toHaveLength(1));
    await typeIntoComposer('文件');

    // 文本形态：`看这个 ` + `` `@file:"C:\a.ts"` `` + ` ` + `文件`
    expect(draftText()).toBe('看这个 `@file:"C:\\a.ts"` 文件');
  });

  it('× 删除 chip：chip 消失，文本里也不再残留引用标记', async () => {
    tauri.readClipboardFiles.mockResolvedValue(['/home/u/b.ts']);
    renderChatInput();

    pasteFile(makeTypedFile('b.ts', 'text/plain'));
    await waitFor(() => expect(screen.getAllByTestId('file-chip')).toHaveLength(1));
    fireEvent.click(screen.getByRole('button', { name: '移除文件 b.ts' }));
    await waitFor(() => expect(screen.queryAllByTestId('file-chip')).toHaveLength(0));
    // 只剩插入时补的那个分隔空格（草稿键因此不会被删——`' '` 不是空串）；
    // 关键契约是「引用标记彻底消失」，否则用户会发出一条指向已删文件的路径。
    expect(draftText()).not.toContain('@file:');
    expect(draftText().trim()).toBe('');
  });

  it('浏览器环境：非图片文件提示指向「贴路径」而非静默忽略', async () => {
    renderChatInput();

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
    tauri.readClipboardFiles.mockResolvedValue(['/home/u/b.ts']);
    const user = userEvent.setup();
    const { props } = renderChatInput();

    // 图片走「添加图片」（`<input type="file">`）；非位图文件走粘贴 → 路径 chip
    fireEvent.change(fileInput(), { target: { files: [makeImageFile('a.png')] } });
    await waitFor(() => expect(screen.getByAltText('待发送图片 1')).toBeInTheDocument());
    pasteFile(makeTypedFile('b.ts', 'text/plain'));
    await waitFor(() => expect(screen.getAllByTestId('file-chip')).toHaveLength(1));

    await typeIntoComposer('看这两个');
    await user.click(screen.getByRole('button', { name: '发送消息' }));

    // chip 先在（粘贴时编辑器还是空的），随后打的字接在它后面的空格之后
    expect(props.onSend).toHaveBeenCalledWith('`@file:"/home/u/b.ts"` 看这两个', [
      'data:image/png;base64,dGVzdA==',
    ]);
  });
});

/**
 * 粘贴分流（本功能主路径）：资源管理器 Ctrl+C 复制文件 → 聊天框 Ctrl+V。
 *
 * 关键设计（**别“优化”回“先问原生”**）：图片优先，原生路径只兜底。
 * 截图与「复制的位图文件」在 Web 层同形（都是 `kind: 'file'` + MIME `image/png`），
 * 而且**两者都该进图片通道**——所以先按 MIME 白名单分流，只有存在非位图文件时
 * 才去问原生要 CF_HDROP 路径。旧顺序（先问原生）会把复制的 `.png` 丢进 chip，
 * 模型只能拿到路径去 read_file 读二进制，用户看不到图。
 */
describe('ChatInput / 粘贴分流（图片优先，原生路径兜底）', () => {
  /**
   * 构造「粘贴事件里有 N 个文件项」的 payload（jsdom 无真实剪贴板）。
   * `types`/`files` 必给：RichTextPlugin 的 `eventFiles()` 先读它们判断
   * 「这批粘贴是文件还是文本」（真实浏览器的 DataTransfer 一定有）。
   */
  function pasteWithItems(items: Array<{ file: File | null; type?: string }>) {
    const realFiles = items.map(({ file }) => file).filter((file): file is File => file !== null);
    return fireEvent.paste(composerEl(), {
      clipboardData: {
        types: ['Files'],
        files: realFiles,
        items: items.map(({ file, type }) => ({
          kind: 'file',
          // 显式 type 优先（剪贴板项的 MIME 不一定等于 File 上的 type）
          type: type ?? file?.type ?? 'application/octet-stream',
          getAsFile: () => file,
        })),
      },
    });
  }

  /** 单文件项（大多数用例）。 */
  function pasteWithFile(file: File | null, type = 'application/pdf') {
    return pasteWithItems([{ file, type }]);
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

    // 真正发给模型的是绝对路径（不是二进制），且形态是行内 code
    await userEvent.setup().click(screen.getByRole('button', { name: '发送消息' }));
    expect(props.onSend).toHaveBeenCalledWith('`@file:"C:\\work\\a.pdf"` ', []);
  });

  it('原生返回空数组（截图场景）→ 图片通道，且整个过程不问原生', async () => {
    tauri.readClipboardFiles.mockResolvedValue([]);
    renderChatInput();

    // 截图：MIME 是 image/png，直接命中位图白名单
    pasteWithFile(makeImageFile('image.png'), 'image/png');

    await waitFor(() => expect(screen.getByAltText('待发送图片 1')).toBeInTheDocument());
    expect(screen.queryByTestId('file-chip')).not.toBeInTheDocument();
    // 全图片分支不问原生（省一次 IPC，也不靠它分辨截图与复制的位图）
    expect(tauri.readClipboardFiles).not.toHaveBeenCalled();
  });

  it('复制 .png 文件（剪贴板既有 CF_HDROP 又有 image/png）→ 走图片通道，不塞 chip', async () => {
    // 本次修正的核心用例：原生明明能给出 C:\pics\a.png，但用户的期望是“看到这张图”，
    // 而位图 MIME 已明确 → 必须先走图片通道（chip 只会让模型 read_file 读二进制失败）。
    tauri.readClipboardFiles.mockResolvedValue(['C:\\pics\\a.png']);
    renderChatInput();

    pasteWithFile(makeImageFile('a.png'), 'image/png');

    await waitFor(() => expect(screen.getByAltText('待发送图片 1')).toBeInTheDocument());
    expect(screen.queryByTestId('file-chip')).not.toBeInTheDocument();
    // 证明省了一次 IPC：图片分支压根不查剪贴板文件
    expect(tauri.readClipboardFiles).not.toHaveBeenCalled();
  });

  it('复制 .pdf 文件 → 走路径 chip 通道，无图片预览', async () => {
    tauri.readClipboardFiles.mockResolvedValue(['C:\\work\\b.pdf']);
    renderChatInput();

    pasteWithFile(makeTypedFile('b.pdf', 'application/pdf'));

    const chip = await screen.findByTestId('file-chip');
    expect(chip).toHaveTextContent('b.pdf');
    expect(chip).toHaveAttribute('title', 'C:\\work\\b.pdf');
    expect(screen.queryByAltText(/待发送图片/)).not.toBeInTheDocument();
    expect(tauri.readClipboardFiles).toHaveBeenCalledTimes(1);
  });

  it('混合（png + pdf 一起复制）→ 整批走 chip，不出现图片预览', async () => {
    // 取舍：宁可图片也走路径，也不要在一条消息里同时出现
    // 「同一张图的 base64 预览」+「指向它的 chip」（模型侧重复入上下文）。
    tauri.readClipboardFiles.mockResolvedValue(['C:\\pics\\a.png', 'C:\\work\\b.pdf']);
    renderChatInput();

    pasteWithItems([
      { file: makeImageFile('a.png'), type: 'image/png' },
      { file: makeTypedFile('b.pdf', 'application/pdf'), type: 'application/pdf' },
    ]);

    const chips = await screen.findAllByTestId('file-chip');
    expect(chips).toHaveLength(2);
    expect(chips[0]).toHaveTextContent('a.png');
    expect(chips[1]).toHaveTextContent('b.pdf');
    expect(screen.queryByAltText(/待发送图片/)).not.toBeInTheDocument();
  });

  it('复制的 png 读不出字节（CF_HDROP 空壳）→ 回退路径 chip，不停在“图片过大”', async () => {
    // 图片优先带来的新风险：File 只有 name/size、读不出内容。
    // 此时必须回退原生路径通道，否则用户会撞上误导性提示且无路可走。
    vi.stubGlobal('FileReader', FailingFileReader);
    tauri.readClipboardFiles.mockResolvedValue(['C:\\pics\\blank.png']);
    renderChatInput();

    pasteWithFile(makeImageFile('blank.png'), 'image/png');

    const chip = await screen.findByTestId('file-chip');
    expect(chip).toHaveAttribute('title', 'C:\\pics\\blank.png');
    expect(screen.queryByAltText(/待发送图片/)).not.toBeInTheDocument();
    // 回退成功后不应残留「图片读取失败 / 图片过大」这类提示
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
  });

  it('复制的 png 是 0 字节空壳 → 同样回退路径 chip，不弄出一张破图', async () => {
    // 空壳的另一种形状：FileReader 不报错，但 data URL 载荷为空。
    tauri.readClipboardFiles.mockResolvedValue(['C:\\pics\\empty.png']);
    renderChatInput();

    pasteWithFile(makeImageFile('empty.png', 0), 'image/png');

    const chip = await screen.findByTestId('file-chip');
    expect(chip).toHaveAttribute('title', 'C:\\pics\\empty.png');
    expect(screen.queryByAltText(/待发送图片/)).not.toBeInTheDocument();
  });

  it('复制的 png 超过 4MB → 保留“图片过大”提示，不回退成 chip（确定原因不靠路径通道救）', async () => {
    tauri.readClipboardFiles.mockResolvedValue(['C:\\pics\\big.png']);
    renderChatInput();

    pasteWithFile(makeImageFile('big.png', 5 * 1024 * 1024), 'image/png');

    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent('图片过大（超过 4MB）'),
    );
    // 过大是确定原因（压缩后再传），不应被兜底成“模型读不出二进制”的 chip
    expect(screen.queryByTestId('file-chip')).not.toBeInTheDocument();
  });

  it('图片读不出且原生也没有路径（真·截图）→ 保留提示，不静默', async () => {
    vi.stubGlobal('FileReader', FailingFileReader);
    tauri.readClipboardFiles.mockResolvedValue([]);
    renderChatInput();

    pasteWithFile(makeImageFile('image.png'), 'image/png');

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('图片读取失败'));
    expect(screen.queryByTestId('file-chip')).not.toBeInTheDocument();
    expect(screen.queryByAltText(/待发送图片/)).not.toBeInTheDocument();
    // 读失败时才会为了兜底去问原生
    expect(tauri.readClipboardFiles).toHaveBeenCalledTimes(1);
  });

  it('原生读取失败（reject）+ 非位图文件 → 不抛错，回退图片通道并给出可操作提示', async () => {
    tauri.readClipboardFiles.mockRejectedValue(new Error('ACL denied'));
    renderChatInput();

    pasteWithFile(makeTypedFile('b.pdf', 'application/pdf'));

    // 原生通道异常不能变成“粘贴没反应”：回退图片通道，pdf 由白名单拦下并指路
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('不支持的文件类型'));
    expect(screen.queryByTestId('file-chip')).not.toBeInTheDocument();
  });

  it('原生读取失败（reject）→ 纯位图粘贴不丢图，也不抛错', async () => {
    // 注：全图片分支根本不问原生（reject 不会发生），此用例保证「原生通道再怎么
    // 坏，粘贴的图也不能丢」——读剪贴板异常在 readClipboardPaths 里已降级成 []。
    tauri.readClipboardFiles.mockRejectedValue(new Error('ACL denied'));
    renderChatInput();

    pasteWithFile(makeImageFile('image.png'), 'image/png');

    // 不能因为读剪贴板失败就让粘贴「完全没反应」
    await waitFor(() => expect(screen.getByAltText('待发送图片 1')).toBeInTheDocument());
    expect(screen.queryByTestId('file-chip')).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('纯文本粘贴：不查剪贴板文件，文本照常进编辑器', async () => {
    renderChatInput();

    // 真实用户是「先把光标放进输入框，再 Ctrl+V」：编辑器要持有选区，粘贴才有落点
    await act(async () => {
      composerEl().focus();
      caretToEnd(composerEl());
    });

    // 剪贴板只带 text/plain（不带文件项）：RichText 的默认导入路径插文本
    await pasteTextOntoComposer('普通文本');

    // 文件通道完全没被碰：既不问原生要路径，也不产生 chip
    expect(tauri.readClipboardFiles).not.toHaveBeenCalled();
    expect(screen.queryByTestId('file-chip')).not.toBeInTheDocument();
    // 粘贴的文本进了编辑器。旧断言看的是 fireEvent 的 `!defaultPrevented`
    // （“没被拦截”）；富文本编辑器必须自己接管 paste 才能维护内部模型，事件层面
    // 调用 preventDefault 是**预期行为**，所以改为验证「粘贴结果本身」——
    // 语义（纯文本照常粘进来、文件通道不介入）不变，且比看事件标志更强。
    expect(composerText()).toBe('普通文本');
    expect(draftText()).toBe('普通文本');
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
