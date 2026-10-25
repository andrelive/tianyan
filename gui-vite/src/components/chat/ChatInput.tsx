import { useState, useRef, useCallback, useEffect } from 'react';
import { Send, Square, ImagePlus, X } from 'lucide-react';
import { cn, formatNumber } from '@/lib/utils';
import type { RequestPerf, StreamUsage } from '@/lib/types';
import { useAppStore, PENDING_SESSION_KEY } from '@/lib/store';
import ModelSelector from './ModelSelector';
import ThinkingSelect from './ThinkingSelect';
import ContextRing from './ContextRing';
import ZoomableImage from '@/components/ui/ImageLightbox';

interface Props {
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

/** 空图片数组常量（草稿缺省值；引用稳定，避免每次渲染新建数组）。 */
const EMPTY_IMAGES: string[] = [];

/** 读取 File → data URL；超过大小限制返回 null 并提示 */
function fileToDataUrl(file: File): Promise<string | null> {
  return new Promise((resolve) => {
    if (file.size > MAX_IMAGE_BYTES) {
      resolve(null);
      return;
    }
    const reader = new FileReader();
    reader.onload = () => resolve(typeof reader.result === 'string' ? reader.result : null);
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
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const dragDepthRef = useRef(0);
  const [dragOver, setDragOver] = useState(false);

  // Auto-resize textarea as content grows
  useEffect(() => {
    const el = textareaRef.current;
    if (el) {
      el.style.height = 'auto';
      el.style.height = Math.min(el.scrollHeight, 200) + 'px';
    }
  }, [input]);

  // Focus on mount
  useEffect(() => {
    textareaRef.current?.focus();
  }, []);

  // 压缩期间禁发：压缩请求与发送并发会基于分叉状态组装上下文
  // （textarea/send 一并禁用，与 streaming 语义一致）
  // 控制面操作（回退/重做）期间同理禁发（ADR-041）。
  const canSend =
    !isStreaming && !compressing && !busy && (input.trim().length > 0 || images.length > 0);

  const addImages = useCallback(
    async (files: FileList | File[]) => {
      const all = Array.from(files);
      if (all.length === 0) return;

      // 类型白名单：非位图（SVG / HTML / 其他二进制）在入口拦下并给可操作提示。
      // 不能只按 `image/*` 过滤——SVG 的 MIME 就是 `image/svg+xml`，会被上游 400。
      const accepted = all.filter(isAcceptedImage);
      const rejectedTypes = all.filter((f) => !isAcceptedImage(f));
      if (rejectedTypes.length > 0) {
        const names = rejectedTypes.map((f) => f.name || f.type || '未知文件').join('、');
        setRejected(
          `不支持的文件类型：${names}（图片通道仅支持 PNG/JPEG/WebP/GIF；` +
            `SVG / HTML 等可以：① 截图或导出成 PNG；② 把文件路径发给我，我用 read_file 读源码）`,
        );
      }
      if (accepted.length === 0) return;
      // 全部通过时才清旧提示（否则刚设的类型提示会被立刻覆盖）
      if (rejectedTypes.length === 0) setRejected(null);

      // 读图（IO）在草稿更新之外完成；写入合成见下方函数式更新
      const urls: string[] = [];
      const tooLarge: string[] = [];
      for (const file of accepted) {
        const url = await fileToDataUrl(file);
        if (url) {
          urls.push(url);
        } else {
          tooLarge.push(file.name);
        }
      }
      if (tooLarge.length > 0) {
        setRejected('图片过大（超过 4MB）：' + tooLarge.join('、'));
      }

      // 函数式更新：读图是异步的，基于最新草稿追加（不覆盖等待期间输入的文字）
      setInputDraft(draftKey, (prev) => {
        const room = MAX_IMAGES - prev.images.length;
        if (urls.length > room) {
          setRejected('最多上传 ' + MAX_IMAGES + ' 张图片');
        }
        return { ...prev, images: [...prev.images, ...urls.slice(0, Math.max(room, 0))] };
      });
    },
    [draftKey, setInputDraft],
  );

  const handleSend = useCallback(() => {
    if (isStreaming || compressing || busy || (input.trim().length === 0 && images.length === 0)) {
      return;
    }
    onSend(input, images);
    // 清空该会话草稿（键删除；已发送内容不再随页面/会话切换恢复）
    setInputDraft(draftKey, { text: '', images: [] });
    setRejected(null);
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto';
    }
  }, [input, images, isStreaming, compressing, busy, onSend, draftKey, setInputDraft]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    [handleSend],
  );

  const handlePaste = useCallback(
    (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
      // 剪贴板里的**文件项**一律交给 addImages（内部按白名单过滤并给提示）：
      // 过滤条件用 `kind === 'file'` 而不是 `type.startsWith('image/')`——
      // 后者会放过 SVG（MIME `image/svg+xml`），前者能接住 HTML 等非图片文件，
      // 让"粘贴了不支持的文件"给出明确提示而不是静默无反应。
      const files = Array.from(e.clipboardData?.items ?? [])
        .filter((it) => it.kind === 'file')
        .map((it) => it.getAsFile())
        .filter((f): f is File => f !== null);
      if (files.length > 0) {
        e.preventDefault();
        void addImages(files);
      }
    },
    [addImages],
  );

  const handleDragEnter = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    dragDepthRef.current += 1;
    setDragOver(true);
  }, []);

  const handleDragLeave = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    dragDepthRef.current -= 1;
    if (dragDepthRef.current <= 0) {
      dragDepthRef.current = 0;
      setDragOver(false);
    }
  }, []);

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      dragDepthRef.current = 0;
      setDragOver(false);
      if (e.dataTransfer?.files?.length) void addImages(e.dataTransfer.files);
    },
    [addImages],
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
      <div
        className={cn(
          'mx-auto max-w-4xl rounded-2xl border border-[var(--color-border)] bg-[var(--color-bg-secondary)] transition-colors',
          dragOver && 'ring-2 ring-blue-500/40 bg-blue-500/5',
        )}
        onDragEnter={handleDragEnter}
        onDragLeave={handleDragLeave}
        onDragOver={(e) => e.preventDefault()}
        onDrop={handleDrop}
      >
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
            {dragOver && (
              <div className="w-16 h-16 rounded-lg border-2 border-dashed border-blue-500/50 flex items-center justify-center text-xs text-blue-500">
                松放放入
              </div>
            )}
          </div>
        )}

        {rejected && (
          <p className="text-xs text-red-500 px-3 pt-2" role="alert">
            {rejected}
          </p>
        )}

        <textarea
          ref={textareaRef}
          value={input}
          onChange={(e) => setInputDraft(draftKey, (prev) => ({ ...prev, text: e.target.value }))}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
          placeholder={
            dragOver ? '松开鼠标添加图片' : '输入消息... (Shift+Enter 换行，可粘贴/拖拽图片)'
          }
          rows={2}
          disabled={isStreaming || compressing || turnRunning || busy}
          aria-label="输入消息"
          className={cn(
            'w-full resize-none bg-transparent px-3.5 pt-3 pb-1',
            'text-sm text-[var(--color-text-primary)]',
            'placeholder:text-[var(--color-text-tertiary)]',
            'focus:outline-none',
            'disabled:opacity-50 disabled:cursor-not-allowed',
            'transition-colors',
          )}
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
