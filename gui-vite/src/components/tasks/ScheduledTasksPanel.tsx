import ScheduledTasksSection from './ScheduledTasksSection';

/**
 * 定时任务面板（一级导航「定时任务」）：到点调用智能体在指定工作区
 * 完成给定指令的周期任务（类 openclaw 的定时 AI 工作）。
 *
 * 与其他任务语义分层：内置调度任务（摘要/GC 等系统 cron）在「洞察」
 * 展示；会话发起的后台任务（委托/终端）会话绑定，在会话页内展示。
 */
export default function ScheduledTasksPanel() {
  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center justify-between px-6 py-4 border-b border-[var(--color-border)]">
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">定时任务</h2>
      </div>
      <div className="flex-1 overflow-y-auto p-6">
        <div className="max-w-3xl mx-auto">
          <p className="text-xs text-[var(--color-text-tertiary)] mb-4">
            到点自动调用智能体在指定工作区完成指令（cron 周期）。可让智能体用 schedule_task
            工具创建，或在此手动管理。
          </p>
          <ScheduledTasksSection />
        </div>
      </div>
    </div>
  );
}
