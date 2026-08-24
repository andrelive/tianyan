import type { ReactNode } from 'react';
import { AlertCircle, Loader2 } from 'lucide-react';

/**
 * 列表-详情双列布局（收敛 Roles/Skills 面板重复脚手架）。
 *
 * - 顶部概览区（header 插槽，如统计栏）；
 * - 左列：标题区（listHeader）+ 列表三态（loading / error / empty）+ 列表内容；
 * - 右列：详情区（detail 整体由调用方渲染——详情内容差异大，不进组件）。
 *
 * 三态语义单点：加载旋转、错误横幅、空态占位的 JSX 只在此实现一次。
 */
interface ListDetailPanelProps {
  /** 顶部统计/概览区（可选）。 */
  header?: ReactNode;
  /** 左列标题区。 */
  listHeader: ReactNode;
  /** 列表加载中。 */
  listLoading?: boolean;
  /** 列表加载错误消息。 */
  listError?: string | null;
  /** 列表空态内容（非 null 时展示；loading/error 优先）。 */
  listEmpty?: ReactNode;
  /** 列表内容（非三态时）。 */
  list: ReactNode;
  /** 加载指示 aria-label。 */
  listAriaLabel?: string;
  /** 详情区内容。 */
  detail: ReactNode;
}

export default function ListDetailPanel({
  header,
  listHeader,
  listLoading = false,
  listError = null,
  listEmpty,
  list,
  listAriaLabel = '正在加载',
  detail,
}: ListDetailPanelProps) {
  return (
    <div className="flex flex-col h-full">
      {header}
      <div className="flex flex-1 min-h-0">
        <div className="w-[40%] min-w-[260px] max-w-[360px] flex flex-col border-r border-[var(--color-border)]">
          {listHeader}
          <div className="flex-1 overflow-y-auto p-3">
            {listLoading ? (
              <div
                className="flex items-center justify-center py-16"
                aria-live="polite"
                aria-label={listAriaLabel}
              >
                <Loader2 size={24} className="animate-spin text-[var(--color-text-tertiary)]" />
              </div>
            ) : listError ? (
              <div
                role="alert"
                className="flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
              >
                <AlertCircle size={16} />
                <span>{listError}</span>
              </div>
            ) : listEmpty ? (
              <div className="flex flex-col items-center justify-center py-16 text-[var(--color-text-tertiary)]">
                {listEmpty}
              </div>
            ) : (
              list
            )}
          </div>
        </div>
        <div className="flex-1 flex flex-col overflow-hidden">{detail}</div>
      </div>
    </div>
  );
}
