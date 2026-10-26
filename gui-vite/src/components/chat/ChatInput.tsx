import { useState, useRef, useCallback } from 'react';
import { Send, Square, ImagePlus, X } from 'lucide-react';
import { cn, formatNumber } from '@/lib/utils';
import type { RequestPerf, StreamUsage } from '@/lib/types';
import { useAppStore, PENDING_SESSION_KEY } from '@/lib/store';
import { isTauri, readClipboardFiles } from '@/lib/tauri';
import ModelSelector from './ModelSelector';
import ThinkingSelect from './ThinkingSelect';
import ContextRing from './ContextRing';
import ComposerEditor, { type ComposerEditorHandle } from './ComposerEditor';
import ZoomableImage from '@/components/ui/ImageLightbox';

interface Props {
  /** 发送：`content` 为输入框原文（文件 chip 已经在里面，序列化成
   * `` `@file:"<路径>"` `` 的行内 code），`images` 为图片通道（data URL）。 */
  onSend: (content: string, images: string[]) => void;
  onStop: () => void;
  isStreaming: boolean;
  /** 当前会话上下文占用（近一轮完成的 token 用量；切会话随之更新） */
  usage: StreamUsage | null;
  /** 当前会话 token 消耗汇总（跨全部消息累加；底部小字展示） */
  sessionUsage: { uncachedInput: number; cachedInput: number; completion: number } | null;
  /** 最近一次请求的性能指标（TTFT + 生成速度；无样本时不展示） */
  requestPerf?: RequestPerf | null;
  /** 压缩当前会话（圆环详情面板内的快捷操作） */
  onCompress: () => void;
  /** 压缩进行中（ContextRing 按钮禁用 + loading） */
  compressing?: boolean;
  /** 会话有轮在运行（后端起 turn_state 驱动，ADR-035 §9 / U10）：
   * auto 轮（唤醒轮/后台轮）运行中——输入禁用、发送按钮转为"停止"
   * （B 方案：不出现"打了字发不出去"的困惑态）。 */
  turnRunning?: boolean;
  /** 停止请求已发出、后端轮收尾中：按钮禁用并显示"正在停止…"。 */
  stopping?: boolean;
  /** 控制面操作进行中（回退/重做，ADR-041）：输入与发送禁用
   * （会话级操作是原子的——期间新消息会与回退/重做互斥而被服务端拒绝）。 */
  busy?: boolean;
}

/** 单张图片大小上限（4MB，data URL base64 膨胀约 1.33 倍后约 5.3MB 文本） */
const MAX_IMAGE_BYTES = 4 * 1024 * 1024;
/** 单次最多图片数 */
const MAX_IMAGES = 4;

/**
 * 单次最多文件引用数（软上限）。
 *
 * 每个引用都会变成模型侧一次 `read_file`——20 个已是「一次对话里说得清」的
 * 量级。上限按**文本里已有的 chip 总数**算，超出只丢弃多余项并提示，不阻断
 * 已有项（与图片的 `MAX_IMAGES` 同一套手感）。
 */
const MAX_FILES = 20;

/**
 * 图片通道的**类型白名单**（ADR-050 输入准入）。
 *
 * 只有 provider 真正接受的位图能进图片通道：SVG（`image/svg+xml`）、BMP 等
 * "看起来是图片"的类型会被上游直接拒（2026-10-24 实测：ollama / deepseek /
 * opencode 对 SVG 全部 400 `invalid image input` / `unsupported image`）。
 * 后端 `ChatRequest::validate` 与 core 的 `sniff_image_mime` 用同一份口径，
 * 前端这一层是**最早的用户可见提示**（比等上游 400 好）。
 */
const ACCEPTED_IMAGE_TYPES: readonly string[] = [
  'image/png',
  'image/jpeg',
  'image/webp',
  'image/gif',
];

/** 是否为图片通道接受的位图（大小写不敏感；空 type 不接受）。 */
function isAcceptedImage(file: File): boolean {
  return ACCEPTED_IMAGE_TYPES.includes(file.type.toLowerCase());
}

/**
 * 图片通道的入队结果。
 *
 * 常规调用方忽略返回值即可（`<input type="file">` 只关心副作用）；
 * 「粘贴分流」用它判断这一批图是不是**一张都没读出来**：WebView2 对从资源
 * 管理器复制（CF_HDROP）来的文件，可能只给一个「有 name/size、但读不出字节」
 * 的空壳 `File`——那时不能停在图片通道里报错，得回退到原生路径通道
 * （见 `handlePaste` 的兜底）。
 */
interface AddImagesResult {
  /** 成功读进图片通道的张数（被 `MAX_IMAGES` 上限挤掉的不计） */
  accepted: number;
  /**
   * 「大小在限内却读不出字节」的文件名。
   *
   * 刻意**不含**超 4MB 的那些：太大是有明确出路的确定原因（压缩后再传），
   * 不该被兜底成路径 chip（模型 `read_file` 读 5MB 二进制同样读不出图）。
   */
  unreadable: string[];
}

/** 空图片数组常量（草稿缺省值；引用稳定，避免每次渲染新建数组）。 */
const EMPTY_IMAGES: string[] = [];

/** 读取 File → data URL；超过大小限制返回 null 并提示 */
function fileToDataUrl(file: File): Promise<string | null> {
  return new Promise((resolve) => {
    if (file.size > MAX_IMAGE_BYTES) {
      resolve(null);
      return;
    }
    // 0 字节 = 没有图像数据：壳环境（WebView2 对 CF_HDROP 复制来的文件）可能给
    // 一个 size 为 0 的空壳，FileReader 读出来是 `data:image/png;base64,`
    // 这种**空载荷**——必须判失败，否则它会伪装成“读取成功”，把粘贴分流的
    // 路径兜底（见 handlePaste ①）挡在门外，用户只看到一张破图。
    if (file.size === 0) {
      resolve(null);
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      const url = typeof reader.result === 'string' ? reader.result : null;
      // 同理：载荷为空（末尾就是逗号）视为读不出字节
      resolve(url && !url.endsWith(',') ? url : null);
    };
    reader.onerror = () => resolve(null);
    reader.readAsDataURL(file);
  });
}

/** 毫秒 → 简短展示（不足 1 秒用 ms，否则一位小数的 s）。 */
function formatMs(ms: number): string {
  return ms < 1000 ? `${Math.round(ms)}ms` : `${(ms / 1000).toFixed(1)}s`;
}

/**
 * 生成速度（token/s）= 输出 token / 纯生成时长。
 *
 * 分母扣掉 TTFT：首包等待不属于生成阶段（否则慢首包会把速度也拖低，
 * 两个指标失去正交性）。无输出 / 时长非正 → null（不展示）。
 */
function perfTokensPerSec(perf: RequestPerf): number | null {
  const genMs = perf.durationMs - (perf.ttftMs ?? 0);
  if (perf.completionTokens <= 0 || genMs <= 0) return null;
  return Math.round(perf.completionTokens / (genMs / 1000));
}

export default function ChatInput({
  onSend,
  onStop,
  isStreaming,
  usage,
  sessionUsage,
  requestPerf,
  onCompress,
  compressing = false,
  turnRunning = false,
  stopping = false,
  busy = false,
}: Props) {
  // 输入草稿按会话缓存在 store（键 = 当前会话 ?? PENDING）：组件卸载
  // （路由切换）与切换会话都不丢；发送后清空（见 handleSend）。
  const currentSessionId = useAppStore((s) => s.currentSessionId);
  const draftKey = currentSessionId ?? PENDING_SESSION_KEY;
  const draft = useAppStore((s) => s.inputDrafts[draftKey]);
  const setInputDraft = useAppStore((s) => s.setInputDraft);
  const input = draft?.text ?? '';
  const images = draft?.images ?? EMPTY_IMAGES;
  const [rejected, setRejected] = useState<string | null>(null);
  // 富文本编辑器句柄（发送后清空 / 需要重新聚焦）：自动高度与按键逻辑已搬进
  // ComposerEditor（Lexical 用 CSS 的 max-height 自带「撑高 + 封顶滚动」）
  const composerRef = useRef<ComposerEditorHandle>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  // 桌面壳判定（渲染期常量：运行环境不会中途切换）；浏览器里拿不到文件
  // 绝对路径，非位图文件的提示文案随之分岔
  const desktop = isTauri();

  // 压缩期间禁发：压缩请求与发送并发会基于分叉状态组装上下文
  // （编辑器/send 一并禁用，与 streaming 语义一致）
  // 控制面操作（回退/重做）期间同理禁发（ADR-041）。
  const canSend =
    !isStreaming && !compressing && !busy && (input.trim().length > 0 || images.length > 0);

  /**
   * 在**光标处**插入文件 chip（粘贴分流的路径通道）。
   *
   * chip 是编辑器里的行内节点（不是草稿里的附件数组），所以这里只把路径交给
   * 编辑器——插入位置、去重、上限都在 `ComposerEditor.insertFileChips` 里做，
   * 落点就是用户 Ctrl+V 时的光标，不是「草稿顶部的一行」。
   */
  const insertFiles = useCallback((paths: string[]) => {
    const result = composerRef.current?.insertFileChips(paths, MAX_FILES);
    if (result === undefined) return;
    if (result.limited) {
      setRejected(`最多添加 ${MAX_FILES} 个文件`);
    } else if (result.inserted > 0) {
      // 干净插入成功：清掉上一次的提示（与 addImages 的「全部通过才清」一致）
      setRejected(null);
    }
  }, []);

  const addImages = useCallback(
    async (files: FileList | File[]): Promise<AddImagesResult> => {
      const all = Array.from(files);
      if (all.length === 0) return { accepted: 0, unreadable: [] };

      // 类型白名单：非位图（SVG / HTML / 其他二进制）在入口拦下并给可操作提示。
      // 不能只按 `image/*` 过滤——SVG 的 MIME 就是 `image/svg+xml`，会被上游 400。
      const accepted = all.filter(isAcceptedImage);
      const rejectedTypes = all.filter((f) => !isAcceptedImage(f));
      if (rejectedTypes.length > 0) {
        const names = rejectedTypes.map((f) => f.name || f.type || '未知文件').join('、');
        // 非位图的出路按环境分岔：桌面版能拿绝对路径（粘贴时原生读 CF_HDROP），
        // 浏览器只能用文本路径（`File.path` 是 Electron 扩展，Web 标准没有）。
        const hint = desktop
          ? '其他文件请先复制，再在输入框 Ctrl+V 粘贴（按绝对路径引用）'
          : '浏览器环境拿不到文件绝对路径，请用桌面版粘贴文件，或直接把路径贴到输入框（我会用 read_file 读取）';
        setRejected(`不支持的文件类型：${names}（图片通道仅支持 PNG/JPEG/WebP/GIF；${hint}）`);
      }
      if (accepted.length === 0) return { accepted: 0, unreadable: [] };
      // 全部通过时才清旧提示（否则刚设的类型提示会被立刻覆盖）
      if (rejectedTypes.length === 0) setRejected(null);

      // 读图（IO）在草稿更新之外完成；写入合成见下方函数式更新
      const urls: string[] = [];
      const tooLarge: string[] = [];
      const unreadable: string[] = [];
      for (const file of accepted) {
        const url = await fileToDataUrl(file);
        if (url) {
          urls.push(url);
        } else if (file.size > MAX_IMAGE_BYTES) {
          tooLarge.push(file.name);
        } else {
          // 大小在限内却读不出字节：壳环境没把图像数据交出来（CF_HDROP 空壳），
          // 不是「文件太大」，措辞必须区分——否则用户被指向错误的出路。
          unreadable.push(file.name);
        }
      }
      if (tooLarge.length > 0) {
        setRejected('图片过大（超过 4MB）：' + tooLarge.join('、'));
      } else if (unreadable.length > 0) {
        setRejected('图片读取失败（没有拿到图像数据）：' + unreadable.join('、'));
      }

      // 函数式更新：读图是异步的，基于最新草稿追加（不覆盖等待期间输入的文字）
      setInputDraft(draftKey, (prev) => {
        const room = MAX_IMAGES - prev.images.length;
        if (urls.length > room) {
          setRejected('最多上传 ' + MAX_IMAGES + ' 张图片');
        }
        return { ...prev, images: [...prev.images, ...urls.slice(0, Math.max(room, 0))] };
      });

      // accepted = 真正读出字节的张数（被上限挤掉的不算「读失败」，故不进 unreadable）
      return { accepted: urls.length, unreadable };
    },
    [desktop, draftKey, setInputDraft],
  );

  const handleSend = useCallback(() => {
    if (isStreaming || compressing || busy) return;
    if (input.trim().length === 0 && images.length === 0) return;
    // chip 已在 content 里（编辑器的序列化文本），不再有独立的 files 参数
    onSend(input, images);
    // 清空该会话草稿（键删除；已发送内容不再随页面/会话切换恢复）
    setInputDraft(draftKey, { text: '', images: [] });
    setRejected(null);
    // 发送后把焦点还给编辑器（旧 textarea 天然保持焦点，Lexical 也要显式要一次）
    composerRef.current?.focus();
  }, [input, images, isStreaming, compressing, busy, onSend, draftKey, setInputDraft]);

  /**
   * 读剪贴板里的文件绝对路径（CF_HDROP）。
   * 任何失败都降级成 `[]`：这条通道只是兜底，读不到也不能让粘贴「完全没反应」
   * （tauri 层已降级一次，这里再兜一层，防御 ACL / IPC 异常）。
   */
  const readClipboardPaths = useCallback(async (): Promise<string[]> => {
    try {
      return await readClipboardFiles();
    } catch {
      return [];
    }
  }, []);

  /**
   * 粘贴分流：**图片通道优先，原生路径通道只作兜底**。
   *
   * 为什么顺序是这个（本功能的**核心知识**，未来别“优化”回“先问原生”）：
   * 1. 截图（`Win+Shift+S`）和「资源管理器里复制一个 `.png` 文件」在 Web 层
   *    **同形**：都是 `kind: 'file'` 的项，MIME 都是 `image/png`。而且**两者
   *    本来就都该走图片通道**（用户的期望是“看到这张图”，`addImages` 对两者的
   *    处理完全一致——都是读成 data URL）。目的地相同，就不需要让原生侧去分辨
   *    它们，还能省掉一次 IPC。
   * 2. 真正需要绝对路径的只有**非位图文件**（pdf/docx/zip/svg…），而那时 MIME
   *    已经明确不在白名单里——原生只需在「存在非白名单文件」时出场。
   * 3. 反过来做（先问原生有没有 CF_HDROP，有路径就塞 chip）会把复制的 `.png`
   *    也送进路径通道：模型拿到 `C:\pics\a.png` 去 `read_file` 读二进制，图像
   *    内容进不了上下文，用户看不到图——这是旧实现的真实缺陷。
   *
   * 混合场景（一次复制 png+pdf）统一走路径通道（**明确的取舍**）：只要存在
   * 非白名单文件就整批 `insertFiles`。否则同一条消息里会出现「同一张图既有一份
   * base64 预览、又有一个指向它的 chip」，模型侧重复入上下文、费用翻倍。
   *
   * 兜底（图片优先带来的新风险）：复制来的文件那个 `File` 来自 CF_HDROP，
   * WebView2 可能给的是「有 name/size 但读不出字节」的空壳，`fileToDataUrl`
   * 返回 null。若整批都**读不出字节**，就在原生有路径时改走 chip 通道——否则
   * 用户只会撞上“图片太大”这种误导性死胡同。（「超过 4MB」**不**回退：那是
   * 确定原因，压缩后再传即可，塞成 chip 只会让模型去读一份读不出的二进制。）
   * 原生也没路径（真·截图但读不出）则保留提示，不静默吞掉。
   *
   * 同步 / 异步的分界：`ClipboardEvent.clipboardData` 只在事件分发期间有效，
   * 一进 `await` 就被回收（`items` 清空、`getAsFile()` 返回 null）。所以
   * `File[]` 必须在**同步阶段**构造好并让异步闭包捕获，`preventDefault()`
   * 也必须同步调用（否则默认粘贴已经发生了）。
   * 换 Lexical 后这两件事搬到了 `ComposerEditor.tsx` 的 `PasteFilesPlugin`
   * （在 `PASTE_COMMAND` 的同步阶段取文件 + 拦默认插入），**分流逻辑本身
   * 仍在这里逐字保留**——它是本功能的核心不变量，与编辑器实现无关。
   */
  const handlePasteFiles = useCallback(
    (files: File[]) => {
      const images = files.filter(isAcceptedImage);
      const others = files.filter((f) => !isAcceptedImage(f));

      void (async () => {
        // ① 整批都在位图白名单内（截图 / 复制的位图文件）→ 图片通道，不问原生
        if (others.length === 0 && images.length > 0) {
          const { accepted, unreadable } = await addImages(images);
          // 一张都没读出来（CF_HDROP 空壳）→ 原生若有路径就改走 chip 通道。
          // 只看 unreadable（读不出字节），不看「超 4MB」——后者提示已明确，
          // 转成 chip 反而让模型去 read_file 读一份读不出的二进制。
          if (accepted === 0 && unreadable.length > 0) {
            const paths = await readClipboardPaths();
            if (paths.length > 0) insertFiles(paths);
          }
          return;
        }

        // ② 存在非白名单文件（混合场景也走这里、整批按路径引用），
        //    或极端情况下连 File 都取不到 → 原生路径才是正确通道
        const paths = await readClipboardPaths();
        if (paths.length > 0) {
          insertFiles(paths);
          return;
        }

        // ③ 原生没路径（浏览器 / ACL 失败）→ 回退图片通道。整批交给 addImages
        //    让它自己按白名单挑，顺带给非位图文件一条可操作提示（不静默）
        if (files.length > 0) await addImages(files);
      })();
    },
    [insertFiles, addImages, readClipboardPaths],
  );

  const removeImage = useCallback(
    (idx: number) => {
      setInputDraft(draftKey, (prev) => ({
        ...prev,
        images: prev.images.filter((_, i) => i !== idx),
      }));
    },
    [draftKey, setInputDraft],
  );

  // 一体式输入卡片（DSH 布局）：字段、模型、思考、图片、圆环、发送都在同一个圆角矩形外框内，
  // 各组件不再独立戴框。
  return (
    <div className="border-t border-[var(--color-border)] bg-[var(--color-bg-primary)] px-4 pt-3 pb-3">
      <div className="mx-auto max-w-4xl rounded-2xl border border-[var(--color-border)] bg-[var(--color-bg-secondary)] transition-colors">
        {/* 图片预览 */}
        {images.length > 0 && (
          <div className="flex flex-wrap gap-2 px-3 pt-3 pb-1">
            {images.map((src, i) => (
              <ZoomableImage
                key={src.slice(0, 32) + '-' + i}
                src={src}
                alt={'待发送图片 ' + (i + 1)}
                wrapperClassName="group"
                className="w-16 h-16 rounded-lg object-cover border border-[var(--color-border)]"
              >
                <button
                  onClick={() => removeImage(i)}
                  className="absolute -top-1.5 -right-1.5 p-0.5 rounded-full bg-black/70 text-white opacity-0 group-hover:opacity-100 transition-opacity"
                  aria-label="移除图片"
                >
                  <X className="w-3 h-3" />
                </button>
              </ZoomableImage>
            ))}
          </div>
        )}

        {rejected && (
          <p className="text-xs text-red-500 px-3 pt-2" role="alert">
            {rejected}
          </p>
        )}

        {/* 富文本编辑器（Lexical）：Enter 发送 / Shift+Enter 换行 / 自动高度 /
            粘贴文件分流都在它内部，对外仍是「受控 value + onChange + onSubmit」。 */}
        <ComposerEditor
          ref={composerRef}
          value={input}
          onChange={(text) => setInputDraft(draftKey, (prev) => ({ ...prev, text }))}
          onSubmit={handleSend}
          onPasteFiles={handlePasteFiles}
          disabled={isStreaming || compressing || turnRunning || busy}
          placeholder="输入消息... (Shift+Enter 换行，可粘贴图片或文件)"
          autoFocus
        />

        {/* 底部控件行：左 = 图片；右 = 模型 / 思考 / 上下文圆环 / 发送 */}
        <div className="flex items-center justify-between gap-2 flex-wrap px-2 pb-2 pt-1">
          <div className="flex items-center gap-0.5 min-w-0">
            {!isStreaming && !turnRunning && (
              <>
                <input
                  ref={fileInputRef}
                  type="file"
                  accept={ACCEPTED_IMAGE_TYPES.join(',')}
                  multiple
                  className="hidden"
                  onChange={(e) => {
                    if (e.target.files?.length) void addImages(e.target.files);
                    e.target.value = '';
                  }}
                />
                <button
                  onClick={() => fileInputRef.current?.click()}
                  disabled={images.length >= MAX_IMAGES}
                  className="p-2 rounded-lg text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)] transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                  aria-label="添加图片"
                  title="添加图片（最多 4 张，单张 4MB）"
                >
                  <ImagePlus className="w-4 h-4" />
                </button>
              </>
            )}
          </div>

          <div className="flex items-center gap-1.5">
            <ModelSelector ghost />
            <ThinkingSelect ghost />
            <ContextRing usage={usage} onCompress={onCompress} compressing={compressing} />
            {isStreaming || turnRunning ? (
              <button
                onClick={onStop}
                disabled={stopping}
                className="flex items-center gap-2 px-3.5 py-2 rounded-xl bg-red-500 hover:bg-red-600 text-white text-sm font-medium transition-colors disabled:opacity-60 disabled:cursor-not-allowed"
                aria-label={stopping ? '正在停止' : '停止生成'}
                title={turnRunning && !isStreaming ? '停止自动轮（唤醒轮/后台轮）' : '停止生成'}
              >
                <Square className="w-4 h-4 fill-current" />
                {stopping ? '正在停止…' : '停止'}
              </button>
            ) : (
              <button
                onClick={handleSend}
                disabled={!canSend}
                aria-label="发送消息"
                className={cn(
                  'flex items-center gap-2 px-3.5 py-2 rounded-xl text-white text-sm font-medium transition-colors',
                  canSend
                    ? 'bg-blue-500 hover:bg-blue-600'
                    : 'bg-[var(--color-bg-tertiary)] text-[var(--color-text-tertiary)] cursor-not-allowed',
                )}
              >
                <Send className="w-4 h-4" />
                发送
              </button>
            )}
          </div>
        </div>
      </div>

      {/* 会话 token 统计小字（DSH 风格，水平居中）：
         未命中输入 / 缓存命中输入 / 输出 / 缓存命中率 */}
      {(sessionUsage || requestPerf) && (
        <div className="flex flex-wrap items-center justify-center gap-x-2 gap-y-0.5 px-4 pb-2 text-[11px] leading-relaxed text-[var(--color-text-tertiary)] select-none">
          {sessionUsage && (
            <>
              <span>
                未命中输入{' '}
                <span className="font-mono">{formatNumber(sessionUsage.uncachedInput)}</span>
              </span>
              <span className="opacity-40">·</span>
              <span>
                缓存命中输入{' '}
                <span className="font-mono">{formatNumber(sessionUsage.cachedInput)}</span>
              </span>
              <span className="opacity-40">·</span>
              <span>
                输出 <span className="font-mono">{formatNumber(sessionUsage.completion)}</span>
              </span>
              <span className="opacity-40">·</span>
              <span>
                缓存命中率
                <span className="font-mono">
                  {(() => {
                    const total = sessionUsage.uncachedInput + sessionUsage.cachedInput;
                    return total > 0
                      ? Math.round((sessionUsage.cachedInput / total) * 100) + '%'
                      : '0%';
                  })()}
                </span>
              </span>
            </>
          )}
          {requestPerf && (
            <>
              {sessionUsage && <span className="opacity-40">·</span>}
              <span title="最近一次请求：首 token 时间 / 生成速度">
                TTFT{' '}
                <span className="font-mono">
                  {requestPerf.ttftMs != null ? formatMs(requestPerf.ttftMs) : '—'}
                </span>
              </span>
              {(() => {
                const tps = perfTokensPerSec(requestPerf);
                if (tps == null) return null;
                return (
                  <>
                    <span className="opacity-40">·</span>
                    <span>
                      <span className="font-mono">{tps}</span> tok/s
                    </span>
                  </>
                );
              })()}
            </>
          )}
        </div>
      )}
    </div>
  );
}
