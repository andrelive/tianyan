import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  type Ref,
} from 'react';
import { LexicalComposer } from '@lexical/react/LexicalComposer';
import { PlainTextPlugin } from '@lexical/react/LexicalPlainTextPlugin';
import { ContentEditable } from '@lexical/react/LexicalContentEditable';
import { HistoryPlugin } from '@lexical/react/LexicalHistoryPlugin';
import { OnChangePlugin } from '@lexical/react/LexicalOnChangePlugin';
import { LexicalErrorBoundary } from '@lexical/react/LexicalErrorBoundary';
import { useLexicalComposerContext } from '@lexical/react/LexicalComposerContext';
import {
  $createLineBreakNode,
  $createParagraphNode,
  $createTextNode,
  $getRoot,
  $getSelection,
  $isRangeSelection,
  COMMAND_PRIORITY_HIGH,
  KEY_ENTER_COMMAND,
  PASTE_COMMAND,
  type EditorState,
  type LexicalEditor,
  type PasteCommandType,
} from 'lexical';
import { cn } from '@/lib/utils';

/**
 * 输入框富文本编辑器（Lexical）。
 *
 * ## 为什么不用 `<textarea>`
 * 下一步要把文件 chip **内联**在输入文字中间（像 Trae/WorkBuddy）：chip 是
 * 文本流里的一个 React 组件，而 `<textarea>` 只认纯文本。所以先换成
 * `contenteditable` 富文本编辑器——选 Lexical 的理由是 `DecoratorNode` 能把
 * React 组件嵌进文本流（下一步的 chip 就用它），且 IME 处理成熟（Meta 生产在用）。
 *
 * **本步只做「功能完全等价」验证**：纯文本 + Enter 发送 + 自动高度 + 粘贴文件分流，
 * 不做 chip 内联（图片行 / 附件 chip 行 / 底部控件行全部不动，仍在 ChatInput）。
 *
 * ## 三个必须写对的地方
 * 1. **IME 组合期间的 Enter 不能发送**（见 `SubmitOnEnterPlugin`）：中文/日文输入法
 *    用 Enter 确认候选词，若当成发送就会「拼音被截断 + 半句话被发出去」。
 * 2. **受控同步不能打断正在进行的输入**（见 `ExternalValuePlugin`）：`onChange`
 *    写 store → 外部 `value` 变 → 又同步回编辑器。必须「编辑器文本已等于 value
 *    就 skip」，否则每次按键都重建节点，光标/组合串会被清掉。
 * 3. **粘贴文件必须同步取 `File[]`**（见 `PasteFilesPlugin`）：`clipboardData`
 *    只在事件分发期间有效，一进 `await` 就被回收——取文件与 `preventDefault`
 *    都必须在事件同步阶段完成，分流逻辑本身留在 ChatInput（见 handlePasteFiles）。
 */

export interface ComposerEditorProps {
  /** 外部受控文本（= store 里的草稿 text）；换会话/回填/清空都由它驱动 */
  value: string;
  /** 每次编辑同步到 store（每次 update 一次，含换行） */
  onChange: (text: string) => void;
  /** Enter（无 Shift）与点击发送按钮之外的键盘提交入口 */
  onSubmit: () => void;
  /**
   * 剪贴板里存在文件项时回调（`File[]` 已在事件同步阶段构造好）。
   *
   * 只负责「把文件交出去」：图片通道优先 / 原生路径兜底 / CF_HDROP 空壳处理
   * 这套分流逻辑**逐字保留**在 ChatInput（那是不变量，别搬进来）。
   */
  onPasteFiles: (files: File[]) => void;
  /** 只读态（streaming / 压缩中 / 自动轮运行 / 控制面操作中）：不可输入、不可发送 */
  disabled?: boolean;
  placeholder?: string;
  /** 挂载后自动聚焦（与旧 textarea 行为一致） */
  autoFocus?: boolean;
}

/** 命令式句柄（发送后清空、外部需要重新聚焦时用） */
export interface ComposerEditorHandle {
  focus: () => void;
  clear: () => void;
}

/** 自动高度上限（与旧 textarea 的 200px 一致；超出后编辑器内部滚动） */
const MAX_HEIGHT_PX = 200;

/**
 * 是否为「输入法组合中」的按键。
 *
 * 三重判定（缺一就会漏）：`event.isComposing` 是标准字段；`keyCode === 229`
 * 是 WebKit/旧 Chromium 在组合期间的遗留标记（此时 `isComposing` 可能为 false）；
 * `editor.isComposing()` 是 Lexical 自己维护的组合态（compositionstart 之后为真，
 * 覆盖「Enter 到来时组合事件还没结束」的场景，WebView2 上实测需要）。
 */
function isComposingKeyEvent(event: KeyboardEvent | null, editor: { isComposing: () => boolean }) {
  if (editor.isComposing()) return true;
  if (event === null) return false;
  // keyCode 已废弃，但 IME 组合期的 229 是唯一可靠的跨浏览器信号
  return event.isComposing === true || event.keyCode === 229;
}

/**
 * 从剪贴板事件里**同步**取出文件项。
 *
 * `ClipboardEvent.clipboardData` 只在事件分发期间有效：`await` 之后 `items`
 * 会清空、`getAsFile()` 返回 null。所以这里必须在同步阶段取完并交出去。
 */
function readClipboardFiles(event: PasteCommandType): File[] {
  const clipboardData = 'clipboardData' in event ? event.clipboardData : null;
  if (clipboardData === null || clipboardData === undefined) return [];
  const items = Array.from(clipboardData.items ?? []);
  return items
    .filter((item) => item.kind === 'file')
    .map((item) => item.getAsFile())
    .filter((file): file is File => file !== null);
}

/**
 * 剪贴板里是否有文件项（`kind === 'file'`）。
 *
 * 注意与 `readClipboardFiles` 分开：壳环境可能给「有 file 项但 getAsFile()
 * 返回 null」的剪贴板（CF_HDROP 空壳），此时仍要拦截默认粘贴（否则浏览器
 * 会把文件名当文本插进正文），但交出去的文件数组为空——ChatInput 的原逻辑
 * 对空文件数组同样会去问原生要路径，行为不变。
 */
function clipboardHasFileItem(event: PasteCommandType): boolean {
  const clipboardData = 'clipboardData' in event ? event.clipboardData : null;
  if (clipboardData === null || clipboardData === undefined) return false;
  return Array.from(clipboardData.items ?? []).some((item) => item.kind === 'file');
}

/**
 * 把焦点交给编辑器——**两步都做**：
 * 1. `rootElement.focus()`：真正的 DOM 焦点（`editor.focus()` 只负责「编辑器选区」，
 *    DOM 焦点是 Lexical 在 reconcile 阶段按需补的，依赖更新时序）；
 * 2. `editor.focus()`：把选区落到内容里并同步到 DOM。
 *
 * 只做第 2 步时，某些路径下（选区已经就位、DOM 焦点却不在编辑器）光标看着在、
 * 键却打不进去；这正是「发送后光标还在输入框里」最容易踩的坑。
 */
function focusComposer(editor: LexicalEditor) {
  const rootElement = editor.getRootElement();
  rootElement?.focus({ preventScroll: true });
  editor.focus();
}

/** Enter 发送（无 Shift）+ Shift+Enter 换行 + IME 组合期一律不发送。 */
function SubmitOnEnterPlugin({ onSubmit }: { onSubmit: () => void }) {
  const [editor] = useLexicalComposerContext();
  // 用 ref 存最新回调：命令注册只做一次（不随父组件重渲染反复注册/注销）
  const submitRef = useRef(onSubmit);
  useEffect(() => {
    submitRef.current = onSubmit;
  }, [onSubmit]);

  useEffect(
    () =>
      editor.registerCommand<KeyboardEvent | null>(
        KEY_ENTER_COMMAND,
        (event) => {
          // ① IME 组合期间：既不发送、也不插换行——Enter 在这里只负责「确认候选词」，
          //    由输入法自己处理。返回 true 表示「已处理」以阻止 PlainTextPlugin 的
          //    默认换行逻辑，但**不** preventDefault（否则可能吞掉组合确认）。
          if (isComposingKeyEvent(event, editor)) return true;
          // ② 程序化派发（无事件对象）不当作发送：交回默认换行逻辑
          if (event === null) return false;
          // ③ Shift+Enter：换行（交回 PlainTextPlugin 插 LineBreakNode）
          if (event.shiftKey) return false;
          // ④ Enter：发送
          event.preventDefault();
          submitRef.current();
          return true;
        },
        COMMAND_PRIORITY_HIGH,
      ),
    [editor],
  );

  return null;
}

/** 粘贴分流：剪贴板里有文件项才拦截，纯文本粘贴交给 PlainTextPlugin。 */
function PasteFilesPlugin({ onPasteFiles }: { onPasteFiles: (files: File[]) => void }) {
  const [editor] = useLexicalComposerContext();
  const pasteRef = useRef(onPasteFiles);
  useEffect(() => {
    pasteRef.current = onPasteFiles;
  }, [onPasteFiles]);

  useEffect(
    () =>
      editor.registerCommand<PasteCommandType>(
        PASTE_COMMAND,
        (event) => {
          // 没有文件项 = 普通文本粘贴：不拦（走 PlainTextPlugin 的纯文本插入）
          if (!clipboardHasFileItem(event)) return false;
          // 【同步阶段】取文件 + 阻止默认插入：异步之后 clipboardData 已回收，
          // 而默认粘贴一旦发生就会把文件名/图片占位插进正文。
          const files = readClipboardFiles(event);
          event.preventDefault();
          pasteRef.current(files);
          return true;
        },
        COMMAND_PRIORITY_HIGH,
      ),
    [editor],
  );

  return null;
}

/**
 * 外部 `value` → 编辑器的单向同步（受控组件的回写通道）。
 *
 * 防「update 循环 / 打断输入」的关键：**编辑器当前文本已等于 value 就什么都不做**。
 * 否则每次按键都会 `onChange` → store → value → 重建全部节点，中文输入法的
 * 组合串会被清空（表现为「拼音打一半消失」）。
 */
function ExternalValuePlugin({ value }: { value: string }) {
  const [editor] = useLexicalComposerContext();

  useEffect(() => {
    editor.update(
      () => {
        // 值相同 → skip（这一条是 IME 安全的全部秘密）
        if ($getRoot().getTextContent() === value) return;

        const root = $getRoot();
        const selection = $getSelection();
        root.clear();

        // 纯文本重建：'\n' → LineBreakNode，其余按行切 TextNode
        // （与 textarea 的 value 语义一一对应，`getTextContent()` 读回来一模一样）
        const paragraph = $createParagraphNode();
        const lines = value.split('\n');
        for (let i = 0; i < lines.length; i += 1) {
          if (i > 0) paragraph.append($createLineBreakNode());
          if (lines[i].length > 0) paragraph.append($createTextNode(lines[i]));
        }
        root.append(paragraph);

        // 只有编辑器当前持有焦点时才挪光标（否则别抢焦点，也别留一个假选区）
        const rootElement = editor.getRootElement();
        const focused =
          rootElement !== null && rootElement.contains(rootElement.ownerDocument.activeElement);
        if (focused && $isRangeSelection(selection)) {
          paragraph.selectEnd();
        }
      },
      // 独立 tag：让 @lexical/history 把外部同步当作独立的历史边界，
      // 撤销不会把「换会话/清空」和用户输入混成一条记录
      { tag: 'external-value-sync' },
    );
  }, [editor, value]);

  return null;
}

/** `disabled` → `editor.setEditable()`（Lexical 的可编辑态只能在挂载后动态设置）。 */
function EditablePlugin({ disabled }: { disabled: boolean }) {
  const [editor] = useLexicalComposerContext();

  useEffect(() => {
    // setEditable 会把 contentEditable / aria-readonly / tabIndex 一起同步到 DOM
    editor.setEditable(!disabled);
  }, [editor, disabled]);

  return null;
}

/** 命令式句柄 + 挂载聚焦（两者都必须在 composer 上下文里执行）。 */
function HandlePlugin({
  handleRef,
  autoFocus,
  disabled,
}: {
  handleRef: Ref<ComposerEditorHandle>;
  autoFocus: boolean;
  disabled: boolean;
}) {
  const [editor] = useLexicalComposerContext();

  useImperativeHandle(
    handleRef,
    () => ({
      focus: () => focusComposer(editor),
      clear: () =>
        editor.update(() => {
          const root = $getRoot();
          root.clear();
          root.append($createParagraphNode());
        }),
    }),
    [editor],
  );

  // 只聚焦一次（与旧 textarea 的「挂载后 focus」等价；后续 disabled 切换不再抢焦点）
  const focusedRef = useRef(false);
  useEffect(() => {
    if (focusedRef.current) return;
    if (!autoFocus || disabled) return;
    focusedRef.current = true;
    focusComposer(editor);
  }, [editor, autoFocus, disabled]);

  return null;
}

/**
 * 聊天输入框的富文本编辑器（Lexical + 纯文本插件）。
 *
 * 对外仍是「受控 + 命令式句柄」：`value` 进来、`onChange` 出去，Enter 提交、
 * 粘贴文件交回调用方。视觉与旧 `<textarea>` 等价（同样 2 行起步、200px 封顶）。
 */
const ComposerEditor = forwardRef<ComposerEditorHandle, ComposerEditorProps>(
  function ComposerEditor(
    { value, onChange, onSubmit, onPasteFiles, disabled = false, placeholder, autoFocus = false },
    ref,
  ) {
    // initialConfig 只在首次挂载时读取：必须以 ref/常量形式稳定住，
    // 否则每次渲染新建对象会让 Lexical 重启编辑器
    const initialConfig = useMemo(
      () => ({
        namespace: 'tianyan-composer',
        // 初始可编辑态；后续变化由 EditablePlugin 动态 setEditable
        editable: !disabled,
        theme: {},
        onError: (error: Error) => {
          // 不抛：输入框崩掉不该拖垮整个会话页（Lexical 的 onError 是「最后一道网」）
          console.error('[ComposerEditor]', error);
        },
      }),
      // 只需在挂载时求值一次（eslint 无法静态看出这一点）
      // eslint-disable-next-line react-hooks/exhaustive-deps
      [],
    );

    const handleChange = useCallback(
      (editorState: EditorState) => {
        // 在 editorState 的只读快照里取纯文本：与 textarea 的 value 语义一致
        const text = editorState.read(() => $getRoot().getTextContent());
        onChange(text);
      },
      [onChange],
    );

    return (
      <LexicalComposer initialConfig={initialConfig}>
        {/* 相对定位容器：placeholder 是编辑器的兄弟节点，用绝对定位叠在文字位置上 */}
        <div className="relative">
          <PlainTextPlugin
            contentEditable={
              <ContentEditable
                aria-label="输入消息"
                aria-multiline="true"
                className={cn(
                  'w-full resize-none bg-transparent px-3.5 pt-3 pb-1',
                  'text-sm text-[var(--color-text-primary)]',
                  'whitespace-pre-wrap break-words [overflow-wrap:anywhere]',
                  'focus:outline-none',
                  // 自动高度：CSS 天然实现（内容撑高 + 封顶后内部滚动），
                  // 比旧 textarea 的 scrollHeight 手算更稳（无需在每次输入后测一次布局）
                  'min-h-[3.5rem] overflow-y-auto',
                  disabled && 'opacity-50 cursor-not-allowed',
                )}
                style={{ maxHeight: MAX_HEIGHT_PX }}
              />
            }
            placeholder={
              <div
                className={cn(
                  'pointer-events-none absolute top-0 left-0 px-3.5 pt-3',
                  'text-sm text-[var(--color-text-tertiary)]',
                  'whitespace-pre-wrap',
                  disabled && 'opacity-50',
                )}
              >
                {placeholder}
              </div>
            }
            ErrorBoundary={LexicalErrorBoundary}
          />
        </div>
        <HistoryPlugin />
        <OnChangePlugin
          onChange={handleChange}
          // 只关心文本变化：纯选区移动（点一下、方向键）不必写 store
          ignoreSelectionChange={true}
        />
        <SubmitOnEnterPlugin onSubmit={onSubmit} />
        <PasteFilesPlugin onPasteFiles={onPasteFiles} />
        <ExternalValuePlugin value={value} />
        <EditablePlugin disabled={disabled} />
        <HandlePlugin handleRef={ref} autoFocus={autoFocus} disabled={disabled} />
      </LexicalComposer>
    );
  },
);

export default ComposerEditor;
