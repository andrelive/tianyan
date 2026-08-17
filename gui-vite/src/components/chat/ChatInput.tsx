import { useState, useRef, useCallback, useEffect } from 'react';
import { Send, Square, ImagePlus, X } from 'lucide-react';
import { cn } from '@/lib/utils';
import type { StreamUsage } from '@/lib/types';
import ModelSelector from './ModelSelector';
import ThinkingSelect from './ThinkingSelect';
import ContextRing from './ContextRing';

interface Props {
  onSend: (content: string, images: string[]) => void;
  onStop: () => void;
  isStreaming: boolean;
  /** 当前会话上下文占用（近一轮完成的 token 用量；切会话随之更新） */
  usage: StreamUsage | null;
}

/** 单张图片大小上限（4MB，data URL base64 膨胀约 1.33 倍后约 5.3MB 文本） */
const MAX_IMAGE_BYTES = 4 * 1024 * 1024;
/** 单次最多图片数 */
const MAX_IMAGES = 4;

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

export default function ChatInput({ onSend, onStop, isStreaming, usage }: Props) {
  const [input, setInput] = useState('');
  const [images, setImages] = useState<string[]>([]);
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

  const canSend = !isStreaming && (input.trim().length > 0 || images.length > 0);

  const addImages = useCallback(async (files: FileList | File[]) => {
    const list = Array.from(files).filter((f) => f.type.startsWith('image/'));
    if (list.length === 0) return;
    setRejected(null);

    // 读取在 setState 之外完成：updater 保持纯函数（StrictMode 双调用安全）
    const urls: string[] = [];
    const tooLarge: string[] = [];
    for (const file of list) {
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

    setImages((prev) => {
      const room = MAX_IMAGES - prev.length;
      if (urls.length > room) {
        setRejected('最多上传 ' + MAX_IMAGES + ' 张图片');
      }
      return [...prev, ...urls.slice(0, Math.max(room, 0))];
    });
  }, []);

  const handleSend = useCallback(() => {
    if (isStreaming || (input.trim().length === 0 && images.length === 0)) return;
    onSend(input, images);
    setInput('');
    setImages([]);
    setRejected(null);
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto';
    }
  }, [input, images, isStreaming, onSend]);

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
      const items = Array.from(e.clipboardData?.items ?? []).filter((it) =>
        it.type.startsWith('image/'),
      );
      if (items.length > 0) {
        e.preventDefault();
        const files = items.map((it) => it.getAsFile()).filter((f): f is File => f !== null);
        if (files.length > 0) void addImages(files);
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

  const removeImage = useCallback((idx: number) => {
    setImages((prev) => prev.filter((_, i) => i !== idx));
  }, []);

  return (
    <div className="border-t border-[var(--color-border)] bg-[var(--color-bg-primary)]">
      <div
        className={cn(
          'relative mx-auto max-w-4xl px-4 pt-3 pb-2 transition-colors',
          dragOver && 'rounded-xl ring-2 ring-blue-500/50 bg-blue-500/5',
        )}
        onDragEnter={handleDragEnter}
        onDragLeave={handleDragLeave}
        onDragOver={(e) => e.preventDefault()}
        onDrop={handleDrop}
      >
        {/* 图片预览 */}
        {images.length > 0 && (
          <div className="flex flex-wrap gap-2 mb-2">
            {images.map((src, i) => (
              <div key={src.slice(0, 32) + '-' + i} className="relative group">
                <img
                  src={src}
                  alt={'待发送图片 ' + (i + 1)}
                  className="w-16 h-16 rounded-lg object-cover border border-[var(--color-border)]"
                />
                <button
                  onClick={() => removeImage(i)}
                  className="absolute -top-1.5 -right-1.5 p-0.5 rounded-full bg-black/70 text-white opacity-0 group-hover:opacity-100 transition-opacity"
                  aria-label="移除图片"
                >
                  <X className="w-3 h-3" />
                </button>
              </div>
            ))}
            {dragOver && (
              <div className="w-16 h-16 rounded-lg border-2 border-dashed border-blue-500/50 flex items-center justify-center text-xs text-blue-500">
                松放放入
              </div>
            )}
          </div>
        )}

        {rejected && (
          <p className="text-xs text-red-500 mb-2" role="alert">
            {rejected}
          </p>
        )}

        <textarea
          ref={textareaRef}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
          placeholder={dragOver ? '松开鼠标添加图片' : '输入消息... (Shift+Enter 换行，可粘贴/拖拽图片)'}
          rows={1}
          disabled={isStreaming}
          aria-label="输入消息"
          className={cn(
            'w-full resize-none rounded-xl border border-[var(--color-border)]',
            'bg-[var(--color-bg-secondary)] px-4 py-3',
            'text-sm text-[var(--color-text-primary)]',
            'placeholder:text-[var(--color-text-tertiary)]',
            'focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-500',
            'disabled:opacity-50 disabled:cursor-not-allowed',
            'transition-colors',
          )}
        />

        {/* 底部控件行：左 = 模型 / 思考强度 / 图片；右 = 上下文圆环 + 发送（DSH 布局） */}
        <div className="mt-2 flex items-center justify-between gap-2 flex-wrap">
          <div className="flex items-center gap-1.5 min-w-0">
            <ModelSelector />
            <ThinkingSelect />
            {!isStreaming && (
              <>
                <input
                  ref={fileInputRef}
                  type="file"
                  accept="image/*"
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
            <ContextRing usage={usage} />
            {isStreaming ? (
              <button
                onClick={onStop}
                className="flex items-center gap-2 px-3.5 py-2 rounded-xl bg-red-500 hover:bg-red-600 text-white text-sm font-medium transition-colors"
                aria-label="停止生成"
              >
                <Square className="w-4 h-4 fill-current" />
                停止
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
    </div>
  );
}
