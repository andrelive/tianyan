import { Check, ShieldAlert, X } from 'lucide-react';
import type { ApprovalStatusSnapshot } from '@/lib/types';

type PendingApproval = ApprovalStatusSnapshot['pending_approvals'][number];

interface Props {
  /** 待审批操作（null 不渲染）。 */
  approval: PendingApproval | null;
  /** 响应请求进行中（禁用按钮防重入）。 */
  busy: boolean;
  /** 批准/拒绝（decision='approve' | 'deny'）。 */
  onRespond: (decision: 'approve' | 'deny') => void;
}

/**
 * 应用层授权卡片：会话流中危险操作等待人工批准（交互模式，
 * 与会话/LLM 澄清无关）。批准/拒绝后挂起的工具自动继续。
 */
export default function ApprovalBanner({ approval, busy, onRespond }: Props) {
  if (!approval) return null;
  return (
    <div
      role="alert"
      aria-label="操作等待授权"
      className="mx-4 mb-2 p-3 rounded-lg border border-amber-500/40 bg-amber-50/60 dark:bg-amber-950/20 text-sm"
    >
      <div className="flex items-center gap-2 text-amber-700 dark:text-amber-400">
        <ShieldAlert size={16} className="shrink-0" />
        <span className="font-medium">操作等待授权</span>
        <span className="text-xs opacity-70 ml-auto">风险：{approval.risk_level}</span>
      </div>
      <p className="mt-1.5 text-xs font-mono text-[var(--color-text-primary)] break-all">
        {approval.action_description}
      </p>
      <div className="mt-2 flex items-center gap-2">
        <button
          type="button"
          onClick={() => onRespond('approve')}
          disabled={busy}
          className="flex items-center gap-1 px-3 py-1.5 text-xs font-medium rounded-md bg-green-600 text-white hover:bg-green-700 disabled:opacity-50"
        >
          <Check size={12} />
          批准
        </button>
        <button
          type="button"
          onClick={() => onRespond('deny')}
          disabled={busy}
          className="flex items-center gap-1 px-3 py-1.5 text-xs font-medium rounded-md bg-red-600 text-white hover:bg-red-700 disabled:opacity-50"
        >
          <X size={12} />
          拒绝
        </button>
        <span className="text-xs text-[var(--color-text-tertiary)]">
          批准后挂起的操作将自动继续执行
        </span>
      </div>
    </div>
  );
}
