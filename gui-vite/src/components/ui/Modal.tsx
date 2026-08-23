import { useEffect, type ReactNode } from 'react';

interface ModalProps {
  /** 关闭回调（点遮罩/Escape/右上角按钮；closeDisabled 时忽略）。 */
  onClose: () => void;
  /** 关闭被禁用（保存中/操作中：遮罩点击与 Escape 不生效）。 */
  closeDisabled?: boolean;
  /** 无障碍标签。 */
  ariaLabel: string;
  /** 面板容器 class（宽度/高度/布局）。 */
  panelClassName?: string;
  children: ReactNode;
}

/**
 * 模态框原语（D4：收敛 WorkspacePicker/DiffView 两套手写壳）：
 * 固定遮罩 + 面板壳 + Escape 关闭 + 遮罩点击关闭 + dialog 无障碍语义。
 * 调用方自行条件渲染（open 时不渲染本组件），面板内容（头部/正文/按钮）由 children 提供。
 */
export default function Modal({
  onClose,
  closeDisabled = false,
  ariaLabel,
  panelClassName = '',
  children,
}: ModalProps) {
  // Escape 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !closeDisabled) onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose, closeDisabled]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      role="presentation"
      onClick={(e) => {
        if (e.target === e.currentTarget && !closeDisabled) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={ariaLabel}
        onClick={(e) => e.stopPropagation()}
        className={`rounded-xl border border-[var(--color-border)] bg-[var(--color-bg-primary)] shadow-xl ${panelClassName}`}
      >
        {children}
      </div>
    </div>
  );
}
