import Modal from './Modal';

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  message: string;
  confirmLabel?: string;
  /** 危险操作样式（红色确认按钮；默认绿色）。 */
  danger?: boolean;
  /** 操作进行中（禁用按钮与关闭）。 */
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * 确认对话框（D4：替代 window.confirm——jsdom 可测、与设计语言一致）。
 * 破坏性操作（删角色/删 MCP/回退种子）统一走此原语。
 */
export default function ConfirmDialog({
  open,
  title,
  message,
  confirmLabel = '确认',
  danger = false,
  busy = false,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  if (!open) return null;
  return (
    <Modal onClose={onCancel} closeDisabled={busy} ariaLabel={title} panelClassName="w-[420px] p-5">
      <h3 className="text-sm font-medium text-[var(--color-text-primary)]">{title}</h3>
      <p className="mt-2 text-xs text-[var(--color-text-secondary)] whitespace-pre-line">
        {message}
      </p>
      <div className="mt-4 flex items-center justify-end gap-2">
        <button
          type="button"
          onClick={onCancel}
          disabled={busy}
          className="px-3 py-1.5 text-xs rounded-md border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50"
        >
          取消
        </button>
        <button
          type="button"
          onClick={onConfirm}
          disabled={busy}
          className={`flex items-center gap-1 px-3 py-1.5 text-xs font-medium rounded-md text-white disabled:opacity-50 ${
            danger ? 'bg-red-600 hover:bg-red-700' : 'bg-green-600 hover:bg-green-700'
          }`}
        >
          {confirmLabel}
        </button>
      </div>
    </Modal>
  );
}
