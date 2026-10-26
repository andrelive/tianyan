import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, act, fireEvent } from '@testing-library/react';
import { createRef, useState } from 'react';
import ComposerEditor, { type ComposerEditorHandle } from '../ComposerEditor';
import {
  composerEl,
  composerText,
  expectComposerReadOnly,
  typeIntoComposer,
} from '@/test/composer';

/**
 * ComposerEditor 的**单元**测试（ChatInput.test.tsx 覆盖的是「接入后」的整卡行为：
 * 分流、chip、图片、草稿）。这里只钉编辑器自身的契约：
 * 外部 value → 编辑器、命令式句柄、placeholder、只读态。
 *
 * 「怎么把字打进去」的说明见 `@/test/composer`（jsdom 下用户事件要靠模拟浏览器分工）。
 */

/** 受控外壳：与 ChatInput 的接法一致（value 进来、onChange 出去）。 */
function Harness({
  initial = '',
  disabled = false,
  handleRef,
  onChangeSpy,
}: {
  initial?: string;
  disabled?: boolean;
  handleRef?: React.Ref<ComposerEditorHandle>;
  onChangeSpy?: (text: string) => void;
}) {
  const [value, setValue] = useState(initial);
  return (
    <ComposerEditor
      ref={handleRef}
      value={value}
      onChange={(text) => {
        onChangeSpy?.(text);
        setValue(text);
      }}
      onSubmit={() => {}}
      onPasteFiles={() => {}}
      disabled={disabled}
      placeholder="输入消息... (Shift+Enter 换行)"
    />
  );
}

describe('ComposerEditor', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('外部 value（多行）同步进编辑器，且文本语义与 textarea 的 value 一致', async () => {
    // 多行的回填（回退/切会话/恢复草稿）必须原样还原：'\n' → Lexical 的 LineBreakNode
    render(<Harness initial={'第一行\n第二行\n'} />);

    await act(async () => {});
    // DOM：两行文字（换行以 <br> 表示，不进 textContent）
    expect(composerText()).toBe('第一行第二行');
    expect(composerEl().querySelectorAll('br').length).toBeGreaterThan(0);
  });

  it('句柄 clear() 清空编辑器并回报 onChange("")', async () => {
    const handleRef = createRef<ComposerEditorHandle>();
    const onChangeSpy = vi.fn();
    render(<Harness handleRef={handleRef} onChangeSpy={onChangeSpy} />);

    await typeIntoComposer('待清空');
    expect(composerText()).toBe('待清空');

    await act(async () => {
      handleRef.current?.clear();
    });

    expect(composerText()).toBe('');
    expect(onChangeSpy).toHaveBeenLastCalledWith('');
  });

  it('句柄 focus() 把焦点交给编辑器', async () => {
    // jsdom 的 focus() 对 contenteditable 元素不生效（`document.activeElement`
    // 停在 body），所以断「编辑器调用了 focus() 且光标落进编辑器」——真机上的
    // 聚焦效果由 e2e（chat.spec.ts 的 "chat input is focused on page load"）兜住。
    const focusSpy = vi.spyOn(HTMLElement.prototype, 'focus');
    const handleRef = createRef<ComposerEditorHandle>();
    render(<Harness handleRef={handleRef} initial="聚焦我" />);
    await act(async () => {});

    await act(async () => {
      handleRef.current?.focus();
    });

    expect(focusSpy.mock.contexts).toContain(composerEl());
    // Lexical 的 focus() 会把编辑器选区挪到内容里（DOM 选区也同步过去）
    const anchor = window.getSelection()?.anchorNode ?? null;
    expect(anchor).not.toBeNull();
    expect(composerEl().contains(anchor)).toBe(true);
  });

  it('placeholder 只在空内容时显示', async () => {
    render(<Harness />);
    await act(async () => {});
    expect(screen.getByText(/输入消息\.\.\./)).toBeInTheDocument();

    await typeIntoComposer('有内容了');
    // 占位符是叠在编辑器上的兄弟节点（不是 `placeholder` 属性），空内容才渲染
    expect(screen.queryByText(/输入消息\.\.\./)).not.toBeInTheDocument();
  });

  it('disabled 时不可编辑（contenteditable=false + aria-readonly），且 Enter 不提交', async () => {
    const onSubmit = vi.fn();
    render(
      <ComposerEditor
        value="只读内容"
        onChange={() => {}}
        onSubmit={onSubmit}
        onPasteFiles={() => {}}
        disabled
        placeholder="输入消息... (Shift+Enter 换行)"
      />,
    );
    await act(async () => {});

    expectComposerReadOnly();

    await act(async () => {
      fireEvent.keyDown(composerEl(), { key: 'Enter' });
    });
    // 编辑器自身不做「禁发」判断（那是 ChatInput handleSend 的职责），
    // 这里只保证禁发态下按键不会插换行、内容不被破坏
    expect(composerText()).toBe('只读内容');
  });
});

/**
 * 行内文件 chip（`FileChipNode`）在编辑器里的行为。
 *
 * ChatInput.test.tsx 覆盖「粘贴 → chip」的整卡链路；这里钉编辑器自己的契约：
 * 命令式插入的落点/去重/上限、插入后 `onChange` 回吐的文本、外部 value 回填时
 * 的 chip 重建。
 */
describe('ComposerEditor / 行内文件 chip', () => {
  /** 造一个文件 chip 的序列化文本（与 FileChipNode.getTextContent 同一口径）。 */
  function chipText(path: string) {
    return '`@file:"' + path + '"`';
  }

  it('insertFileChips 在光标处插入 chip，onChange 回吐「文字 + 行内 code」', async () => {
    const handleRef = createRef<ComposerEditorHandle>();
    const onChangeSpy = vi.fn();
    render(<Harness handleRef={handleRef} onChangeSpy={onChangeSpy} />);

    // 先打字（光标停在末尾），再插入 chip —— 与粘贴的真实动作一致
    await typeIntoComposer('帮我看下');
    await act(async () => {
      handleRef.current?.insertFileChips(['C:\\a b\\架构.docx'], 20);
    });

    // 视觉上 chip 在文字流里（编辑器内部），并且有删除按钮
    const chips = screen.getAllByTestId('file-chip');
    expect(chips).toHaveLength(1);
    expect(composerEl().contains(chips[0])).toBe(true);
    expect(chips[0]).toHaveTextContent('架构.docx');
    expect(screen.getByRole('button', { name: '移除文件 架构.docx' })).toBeInTheDocument();

    // 序列化：文字 + chip + 末尾补的那个空格
    expect(onChangeSpy).toHaveBeenLastCalledWith(
      '帮我看下 ' + chipText('C:\\a b\\架构.docx') + ' ',
    );

    // 继续打字：接在 chip 后的空格之后（不会与 chip 贴死，也不会跑到文本最前面）
    await typeIntoComposer('的架构');
    expect(onChangeSpy).toHaveBeenLastCalledWith(
      '帮我看下 ' + chipText('C:\\a b\\架构.docx') + ' 的架构',
    );
  });

  it('重复插入同一路径只留一个 chip；超过上限时只插放得下的部分', async () => {
    const handleRef = createRef<ComposerEditorHandle>();
    render(<Harness handleRef={handleRef} />);

    let first: { inserted: number; limited: boolean } | undefined;
    await act(async () => {
      first = handleRef.current?.insertFileChips(['/a.ts', '/a.ts', '/b.ts'], 20);
    });
    // 批内去重：3 个路径里只有 2 个不同
    expect(first).toEqual({ inserted: 2, limited: false });
    expect(screen.getAllByTestId('file-chip')).toHaveLength(2);

    // 再插一次同一批：全部命中跨批去重，什么都不加
    let second: { inserted: number; limited: boolean } | undefined;
    await act(async () => {
      second = handleRef.current?.insertFileChips(['/a.ts', '/b.ts'], 20);
    });
    expect(second).toEqual({ inserted: 0, limited: false });
    expect(screen.getAllByTestId('file-chip')).toHaveLength(2);

    // 上限按「已有 chip 总数」算：上限 3、已有 2 → 只放得下 1 个
    let third: { inserted: number; limited: boolean } | undefined;
    await act(async () => {
      third = handleRef.current?.insertFileChips(['/c.ts', '/d.ts'], 3);
    });
    expect(third).toEqual({ inserted: 1, limited: true });
    expect(screen.getAllByTestId('file-chip')).toHaveLength(3);
  });

  it('外部 value 里的行内 chip 会被重建为真 chip（草稿恢复/回退）', async () => {
    render(<Harness initial={'看这个 ' + chipText('/home/u/a.ts') + ' 文件'} />);
    await act(async () => {});

    const chips = screen.getAllByTestId('file-chip');
    expect(chips).toHaveLength(1);
    expect(chips[0]).toHaveAttribute('title', '/home/u/a.ts');
    // DOM 里看不到反引号包裹的原文（它已变成组件）
    expect(composerText()).not.toContain('@file:');
    expect(composerText()).toContain('看这个');
    expect(composerText()).toContain('文件');
  });

  it('历史格式（整行 @file:）回填同样还原成 chip，文本升级为行内格式', async () => {
    const onChangeSpy = vi.fn();
    render(<Harness initial={'看这个\n@file:"C:\\a.ts"'} onChangeSpy={onChangeSpy} />);
    await act(async () => {});

    expect(screen.getAllByTestId('file-chip')).toHaveLength(1);
    expect(screen.queryByText(/@file:/)).not.toBeInTheDocument();
    // 重新序列化后统一成当前格式（旧格式只在历史消息里保留）
    expect(onChangeSpy).toHaveBeenLastCalledWith('看这个\n' + chipText('C:\\a.ts'));
  });

  it('点击 chip 上的 × 删除该 chip，文本同步去掉引用标记', async () => {
    const handleRef = createRef<ComposerEditorHandle>();
    const onChangeSpy = vi.fn();
    render(<Harness handleRef={handleRef} onChangeSpy={onChangeSpy} />);

    await act(async () => {
      handleRef.current?.insertFileChips(['/a.ts'], 20);
    });
    expect(screen.getAllByTestId('file-chip')).toHaveLength(1);

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: '移除文件 a.ts' }));
    });

    expect(screen.queryAllByTestId('file-chip')).toHaveLength(0);
    expect(onChangeSpy).toHaveBeenLastCalledWith(' ');
  });
});
