import { useEffect, useState, type ReactNode } from 'react';
import { X } from 'lucide-react';
import { cn } from '@/lib/utils';

interface ZoomableImageProps {
  /** 图片地址（data URL / 本地路径）。 */
  src: string;
  /** 无障碍文本（缩略图与放大图共用）。 */
  alt: string;
  /** 缩略图 class（尺寸/圆角/边框由调用方决定）。 */
  className?: string;
  /** 缩略图包裹容器附加 class（如输入框预览的 `group` hover 组）。 */
  wrapperClassName?: string;
  /** 懒加载（会话历史图片建议 lazy）。 */
  loading?: 'lazy' | 'eager';
  /** 叠在缩略图上的附加控件（如输入框的「移除图片」按钮）。 */
  children?: ReactNode;
}

/**
 * 可点开放大的图片原语（会话内图片与输入框预览共用）。
 *
 * 缩略图本身是 `<button>`（键盘可达）——点击（或 Enter/Space）打开全屏遮罩：
 * 大图 `object-contain` 适配视口；遮罩点击 / Escape / 右上角按钮关闭。
 * 沿用 Modal（D4 原语）的遮罩惯例：`fixed inset-0 z-50` + 调用方条件渲染，
 * 不引入 Portal。
 */
export default function ZoomableImage({
  src,
  alt,
  className,
  wrapperClassName,
  loading,
  children,
}: ZoomableImageProps) {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  return (
    <>
      <span className={cn('relative inline-block', wrapperClassName)}>
        <button
          type="button"
          onClick={() => setOpen(true)}
          aria-label={`放大查看：${alt}`}
          title="点击放大"
          className="block cursor-zoom-in p-0 border-0 bg-transparent"
        >
          <img src={src} alt={alt} loading={loading} className={className} />
        </button>
        {children}
      </span>

      {open && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-4"
          role="presentation"
          onClick={(e) => {
            if (e.target === e.currentTarget) setOpen(false);
          }}
        >
          <div
            role="dialog"
            aria-modal="true"
            aria-label={`图片预览：${alt}`}
            className="relative max-w-[92vw] max-h-[92vh]"
            onClick={(e) => e.stopPropagation()}
          >
            <img
              src={src}
              alt={alt}
              className="max-w-[92vw] max-h-[92vh] object-contain rounded-lg shadow-2xl"
            />
            <button
              type="button"
              onClick={() => setOpen(false)}
              aria-label="关闭图片预览"
              className="absolute -top-3 -right-3 p-1.5 rounded-full bg-black/70 text-white hover:bg-black/90 transition-colors"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>
      )}
    </>
  );
}
