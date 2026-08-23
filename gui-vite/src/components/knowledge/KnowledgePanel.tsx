import { useState } from 'react';
import { Search, Upload, FolderTree } from 'lucide-react';
import KnowledgeSearchTab from './KnowledgeSearchTab';
import KnowledgeIngestTab from './KnowledgeIngestTab';
import KnowledgeBrowseTab from './KnowledgeBrowseTab';

type Tab = 'search' | 'ingest' | 'browse';

/**
 * 知识库面板（容器）：三块互斥业务各自独立组件（search/ingest/browse），
 * 本组件只承担 tab 切换与布局。子组件保持挂载（hidden 切换）——
 * 切走再切回不丢失浏览路径/搜索结果状态（与原单组件行为一致）。
 */
export default function KnowledgePanel() {
  const [activeTab, setActiveTab] = useState<Tab>('search');

  const tabClasses = (tab: Tab) =>
    `px-4 py-2 text-sm font-medium rounded-md transition-colors ${
      activeTab === tab
        ? 'bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300'
        : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
    }`;

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-6 py-4 border-b border-[var(--color-border)]">
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">知识库</h2>
      </div>

      {/* Sub-tabs */}
      <div
        role="tablist"
        aria-label="知识库功能"
        className="flex gap-2 px-6 py-3 border-b border-[var(--color-border)] bg-[var(--color-bg-secondary)]"
      >
        <button
          role="tab"
          aria-selected={activeTab === 'search'}
          aria-controls="tabpanel-search"
          className={tabClasses('search')}
          onClick={() => setActiveTab('search')}
        >
          <Search size={16} className="inline mr-1.5" />
          搜索
        </button>
        <button
          role="tab"
          aria-selected={activeTab === 'ingest'}
          aria-controls="tabpanel-ingest"
          className={tabClasses('ingest')}
          onClick={() => setActiveTab('ingest')}
        >
          <Upload size={16} className="inline mr-1.5" />
          导入
        </button>
        <button
          role="tab"
          aria-selected={activeTab === 'browse'}
          aria-controls="tabpanel-browse"
          className={tabClasses('browse')}
          onClick={() => setActiveTab('browse')}
        >
          <FolderTree size={16} className="inline mr-1.5" />
          浏览
        </button>
      </div>

      {/* Content（三个 tab 保持挂载，hidden 切换以保留各自状态） */}
      <div className="flex-1 overflow-y-auto p-6">
        <div
          role="tabpanel"
          id="tabpanel-search"
          className={activeTab === 'search' ? '' : 'hidden'}
        >
          <KnowledgeSearchTab />
        </div>
        <div
          role="tabpanel"
          id="tabpanel-ingest"
          className={activeTab === 'ingest' ? '' : 'hidden'}
        >
          <KnowledgeIngestTab />
        </div>
        <div
          role="tabpanel"
          id="tabpanel-browse"
          className={activeTab === 'browse' ? '' : 'hidden'}
        >
          <KnowledgeBrowseTab />
        </div>
      </div>
    </div>
  );
}
