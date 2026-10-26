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
import { RichTextPlugin } from '@lexical/react/LexicalRichTextPlugin';
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
  $isElementNode,
  $isRangeSelection,
  $isTextNode,
  COMMAND_PRIORITY_HIGH,
  KEY_ENTER_COMMAND,
  PASTE_COMMAND,
  type BaseSelection,
  type EditorState,
  type LexicalEditor,
  type LexicalNode,
  type PasteCommandType,
} from 'lexical';
import { cn } from '@/lib/utils';
import { parseInlineChips, toFileRefs } from '@/lib/attachments';
import { $createFileChipNode, $isFileChipNode, FileChipNode } from './FileChipNode';

/**
 * 输入框富文本编辑器（Lexical）。
 *
 * ## 为什么不用 `<textarea>`
 * 文件 chip 要**内联**在输入文字中间（像 Trae/WorkBuddy）：chip 是文本流里的
 * 一个 React 组件，而 `<textarea>` 只认纯文本。Lexical 的 `DecoratorNode` 能把
 * React 组件嵌进文本流（见 `FileChipNode`），IME 处理也成熟（Meta 生产在用）。
 *
 * 对外契约仍是**纯文本受控组件**：`value` 进来、`onChange` 出去，chip 在文本里
 * 的形态就是序列化文本 `` `@file:"<路径>"` ``（`FileChipNode.getTextContent()`）。
 * 于是「草稿恢复 / 回退重发 / 发送」全都不需要知道 chip 的存在。
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

/** `insertFileChips` 的结果（供调用方决定是否提示「超过上限」）。 */
export interface InsertFileChipsResult {
  /** 真正插进文本流的 chip 数（去重后、上限内的） */
  inserted: number;
  /** 是否因为 `maxTotal` 上限丢弃了一部分（调用方据此提示） */
  limited: boolean;
}

/** 命令式句柄（发送后清空、外部需要重新聚焦时用） */
export interface ComposerEditorHandle {
  focus: () => void;
  clear: () => void;
  /**
   * 在**当前光标处**插入 N 个文件 chip（粘贴分流的落点）。
   *
   * 跨批按路径去重（同一个文件贴两次只留一个 chip），并遵守 `maxTotal` 的
   * 总量上限（chip 总数，不是本批数量）；插入后末尾补一个空格，用户接着打字
   * 不会与 chip 贴死。没有可用文本选区（未聚焦 / 选中了 chip 的节点选区）时
   * 退到内容末尾，绝不丢文件。
   */
  insertFileChips: (paths: string[], maxTotal: number) => InsertFileChipsResult;
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

/**
 * Enter 发送（无 Shift）+ Shift+Enter 换行 + IME 组合期一律不发送。
 *
 * 换插件（PlainText → RichText）后这里**一行没改**，因为它拦在
 * `COMMAND_PRIORITY_HIGH`：命令按优先级从高到低派发，本处理器先跑，跑完
 * `return true` 就等于「已处理」，`registerRichText`（`COMMAND_PRIORITY_EDITOR`，
 * 更低）注册的默认处理器根本不会被调用。
 *
 * 于是三条契约的落点：
 * - **Enter 发送**：本处理器 `preventDefault()` + 提交 + `return true`
 *   （RichText 的默认处理器不会去插 `<p>` / `<br>`）。
 * - **Shift+Enter 换行**：`return false` → 交回 RichText 的默认处理器 → 它看到
 *   `event.shiftKey` 会 `dispatchCommand(INSERT_LINE_BREAK_COMMAND)`，即插入
 *   `LineBreakNode`（渲染成 `<br>`，文本内容仍是 `'\n'`，与 textarea 语义一致）。
 * - **IME 组合期间**：`return true`（既不发送也不插换行）——注意这一条**不能**
 *   改成 `return false`：RichText 的默认处理器在组合态下会把 Enter 当成换行，
 *   而组合态的 Enter 只该由输入法负责确认候选词。
 */
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
          //    由输入法自己处理。返回 true 表示「已处理」以阻止 RichTextPlugin 的
          //    默认换行逻辑，但**不** preventDefault（否则可能吞掉组合确认）。
          if (isComposingKeyEvent(event, editor)) return true;
          // ② 程序化派发（无事件对象）不当作发送：交回默认换行逻辑
          if (event === null) return false;
          // ③ Shift+Enter：换行（交回 RichTextPlugin 插 LineBreakNode）
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

/**
 * 插入 chip 时要不要在前面补一个空格。
 *
 * 只在「光标处于某段文字中间/末尾、且前一个字符不是空白」时补：用户打了
 * 「帮我看下」就直接粘贴，得到的是「帮我看下 ␣[chip]」——而不是把 chip 直接
 * 糊在汉字上（chip 有内边距，看着还行；但序列化文本会粘成一坨，模型读起来也
 * 别扭）。
 * 行首（前一字符是空白 / 没有任何前置文本）不需要补——粘贴进空输入框时
 * 文本就是干净的 `` `@file:"…"` ``。
 */
function $needsLeadingSpace(selection: BaseSelection | null): boolean {
  if (!$isRangeSelection(selection) || !selection.isCollapsed()) return false;
  const { anchor } = selection;
  const node = anchor.getNode();
  if (!$isTextNode(node)) return false;
  const before = node.getTextContent().slice(0, anchor.offset);
  return before !== '' && !/\s$/.test(before);
}

/**
 * 当前文本里已有的 chip 路径（插入时的跨批去重与总量上限都基于它）。
 *
 * 深度遍历而不是只看 `getChildren()`：chip 可能落在段落里，也可能被将来加入的
 * 嵌套结构（引用块/列表项）包住。
 */
function $collectFileChipPaths(): string[] {
  const paths: string[] = [];
  const visit = (node: LexicalNode) => {
    if ($isFileChipNode(node)) {
      paths.push(node.getPath());
      return;
    }
    if ($isElementNode(node)) node.getChildren().forEach(visit);
  };
  visit($getRoot());
  return paths;
}

/** 粘贴分流：剪贴板里有文件项才拦截，纯文本粘贴交给 RichTextPlugin。 */
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
          // 没有文件项 = 普通文本粘贴：不拦（走 RichTextPlugin 的默认文本插入）
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
 *
 * 重建时 `parseInlineChips` 会把文本里的引用标记认回来变成真 chip：
 * - 行内 `` `@file:"…"` ``（当前格式）：草稿恢复/回退后 chip 不会退化成裸文本；
 * - 整行 `@file:"…"`（历史格式）：回退旧消息时同样还原成 chip（顺便把格式升级）。
 * 两种格式的序列化文本都是纯文本，所以「文本已等于 value」这个早退仍然成立。
 */
function ExternalValuePlugin({ value }: { value: string }) {
  const [editor] = useLexicalComposerContext();

  useEffect(() => {
    editor.update(
      () => {
        const root = $getRoot();
        // 值相同 → skip（这一条是 IME 安全的全部秘密）
        // 额外条件 `getChildrenSize() > 0`：首次挂载时 root 是空的（文本也是 ''），
        // 必须补一个空段落——RichText 的块状结构（Enter 插 `<p>`、块光标）依赖
        // 它有至少一个块级子节点。
        if (root.getTextContent() === value && root.getChildrenSize() > 0) return;

        const selection = $getSelection();
        // 纯文本重建：引用标记 → chip 节点，'\n' → LineBreakNode，其余按行切 TextNode
        // （与 textarea 的 value 语义一一对应，`getTextContent()` 读回来一模一样）
        const paragraph = $createParagraphNode();
        for (const segment of parseInlineChips(value)) {
          if (segment.type === 'chip') {
            paragraph.append($createFileChipNode(segment.path));
            continue;
          }
          const lines = segment.text.split('\n');
          for (let i = 0; i < lines.length; i += 1) {
            if (i > 0) paragraph.append($createLineBreakNode());
            if (lines[i].length > 0) paragraph.append($createTextNode(lines[i]));
          }
        }
        root.clear();
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
      insertFileChips: (paths: string[], maxTotal: number) => {
        // 返回值在 `editor.update` 的回调里被填（update 回调是同步执行的）
        const result: InsertFileChipsResult = { inserted: 0, limited: false };
        // 批内去重（同一次粘贴里的重复路径）在这里做，跨批去重在上面的 update 里做
        const refs = toFileRefs(paths);
        if (refs.length === 0) return result;

        editor.update(
          () => {
            const existing = new Set($collectFileChipPaths());
            const fresh = refs.filter((ref) => !existing.has(ref.path));
            if (fresh.length === 0) return;
            // 上限按「文本里已有的 chip 总数」算，而不是本批数量
            const room = Math.max(0, maxTotal - existing.size);
            if (fresh.length > room) result.limited = true;
            const accepted = fresh.slice(0, room);
            if (accepted.length === 0) return;

            const nodes: LexicalNode[] = [];
            // 前面紧挨着文字 → 先补一个空格（见 $needsLeadingSpace）
            if ($needsLeadingSpace($getSelection())) nodes.push($createTextNode(' '));
            accepted.forEach((ref, index) => {
              // chip 之间留一个空格：连续粘贴多个文件时不会挤成一串
              if (index > 0) nodes.push($createTextNode(' '));
              nodes.push($createFileChipNode(ref.path));
            });
            // 末尾补一个空格：chip 之后接着打字不会与 chip 贴死
            nodes.push($createTextNode(' '));

            // 光标在哪儿就插哪儿；没有可用的文本选区（未聚焦 / 选中了 chip 的节点
            // 选区）时退到内容末尾——宁可插在末尾，也不能把文件丢掉
            if (!$isRangeSelection($getSelection())) $getRoot().selectEnd();
            const selection = $getSelection();
            if (!$isRangeSelection(selection)) return;
            selection.insertNodes(nodes);
            result.inserted = accepted.length;
          },
          { tag: 'file-chip-insert' },
        );
        return result;
      },
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
 * 聊天输入框的富文本编辑器（Lexical + RichText 插件 + 行内文件 chip）。
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
        // 自定义节点必须登记：chip 的 decorate/importJSON 与文本序列化都靠它
        nodes: [FileChipNode],
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
          {/* RichTextPlugin：DecoratorNode（文件 chip）的前置——它负责把 decorate()
              返回的 React 元素 portal 进 chip 的宿主 span。只有块/行内结构能力，
              不引入 markdown 快捷输入（`@lexical/markdown` 没装也不用）。 */}
          <RichTextPlugin
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
