import { useState, useEffect, useRef, useCallback } from 'react';
import { apiGet, apiPostMultipart } from '@/lib/api-client';
import {
  fetchKnowledgeEntries,
  fetchKnowledgeEntryContent,
  type KnowledgeEntryItem,
} from '@/lib/api-client';
import type { KnowledgeSearchResult, KnowledgeSearchResponse } from '@/lib/types';
import {
  Search,
  Upload,
  File,
  X,
  ChevronDown,
  ChevronUp,
  Loader2,
  CheckCircle2,
  AlertCircle,
  Inbox,
  BookOpen,
  FolderTree,
  Folder,
  ChevronRight,
} from 'lucide-react';

type Tab = 'search' | 'ingest' | 'browse';

function HighlightedText({ text, query }: { text: string; query: string }) {
  if (!query.trim()) return <>{text}</>;
  const escaped = query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const parts = text.split(new RegExp(`(${escaped})`, 'gi'));
  return (
    <>
      {parts.map((part, i) =>
        part.toLowerCase() === query.toLowerCase() ? (
          <mark
            key={`hl-${i}-${part.substring(0, 5)}`}
            className="bg-yellow-200 dark:bg-yellow-800 rounded-sm px-0.5"
          >
            {part}
          </mark>
        ) : (
          part
        ),
      )}
    </>
  );
}

export default function KnowledgePanel() {
  const [activeTab, setActiveTab] = useState<Tab>('search');

  // Search state
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<KnowledgeSearchResult[]>([]);
  const [totalResults, setTotalResults] = useState(0);
  const [isSearching, setIsSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [expandedResultId, setExpandedResultId] = useState<string | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout>>();

  // Ingest state
  const [dragOver, setDragOver] = useState(false);
  const [selectedFiles, setSelectedFiles] = useState<File[]>([]);
  const [tagsInput, setTagsInput] = useState('');
  const [isIngesting, setIsIngesting] = useState(false);
  const [ingestSuccess, setIngestSuccess] = useState(false);
  const [ingestError, setIngestError] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Browse state
  const [browseEntries, setBrowseEntries] = useState<KnowledgeEntryItem[]>([]);
  const [browseLoading, setBrowseLoading] = useState(false);
  const [browseError, setBrowseError] = useState<string | null>(null);
  const [selectedBrowseEntry, setSelectedBrowseEntry] = useState<KnowledgeEntryItem | null>(null);
  const [browseContent, setBrowseContent] = useState('');
  const [browseLevel, setBrowseLevel] = useState('detail');
  const [browseContentLoading, setBrowseContentLoading] = useState(false);
  const [browsePath, setBrowsePath] = useState<string[]>([]);

  const loadBrowseEntries = useCallback(async () => {
    setBrowseLoading(true);
    setBrowseError(null);
    try {
      const res = await fetchKnowledgeEntries(browsePath.join('/') || undefined);
      setBrowseEntries(res.entries);
    } catch (err: unknown) {
      setBrowseError(err instanceof Error ? err.message : '加载失败');
    } finally {
      setBrowseLoading(false);
    }
  }, [browsePath]);

  useEffect(() => {
    if (activeTab === 'browse') {
      loadBrowseEntries();
    }
  }, [activeTab, loadBrowseEntries]);

  const handleBrowseView = async (entry: KnowledgeEntryItem) => {
    if (entry.is_directory) {
      // Navigate into subdirectory
      setBrowsePath((prev) => [...prev, entry.name]);
      setSelectedBrowseEntry(null);
      return;
    }
    setSelectedBrowseEntry(entry);
    setBrowseContentLoading(true);
    try {
      const res = await fetchKnowledgeEntryContent(entry.uri, browseLevel);
      setBrowseContent(res.content);
    } catch (err: unknown) {
      setBrowseContent(`加载失败: ${err instanceof Error ? err.message : '未知错误'}`);
    } finally {
      setBrowseContentLoading(false);
    }
  };

  // Debounced search
  useEffect(() => {
    if (!searchQuery.trim()) {
      setSearchResults([]);
      setTotalResults(0);
      return;
    }

    if (debounceRef.current) {
      clearTimeout(debounceRef.current);
    }

    const query = searchQuery.trim();
    debounceRef.current = setTimeout(async () => {
      setIsSearching(true);
      setSearchError(null);
      try {
        const res = await apiGet<KnowledgeSearchResponse>(
          `/knowledge/search?q=${encodeURIComponent(query)}&limit=10`,
        );
        setSearchResults(res.results);
        setTotalResults(res.total);
      } catch (err) {
        setSearchError(err instanceof Error ? err.message : '搜索失败');
        setSearchResults([]);
      } finally {
        setIsSearching(false);
      }
    }, 300);

    return () => {
      if (debounceRef.current) {
        clearTimeout(debounceRef.current);
      }
    };
  }, [searchQuery]);

  const toggleResultExpand = (id: string) => {
    setExpandedResultId((prev) => (prev === id ? null : id));
  };

  // Ingest handlers
  const handleDragOver = (e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(true);
  };

  const handleDragLeave = (e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(false);
  };

  const handleDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(false);
    const files = Array.from(e.dataTransfer.files);
    if (files.length > 0) {
      setSelectedFiles((prev) => [...prev, ...files]);
      setIngestSuccess(false);
      setIngestError(null);
    }
  };

  const handleFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(e.target.files ?? []);
    if (files.length > 0) {
      setSelectedFiles((prev) => [...prev, ...files]);
      setIngestSuccess(false);
      setIngestError(null);
    }
    e.target.value = '';
  };

  const removeFile = (index: number) => {
    setSelectedFiles((prev) => prev.filter((_, i) => i !== index));
  };

  const formatFileSize = (bytes: number): string => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  };

  const handleIngest = async () => {
    if (selectedFiles.length === 0) return;

    setIsIngesting(true);
    setIngestError(null);
    setIngestSuccess(false);

    try {
      const formData = new FormData();
      for (const file of selectedFiles) {
        formData.append('files', file);
      }
      if (tagsInput.trim()) {
        const tags = tagsInput
          .split(',')
          .map((t) => t.trim())
          .filter(Boolean);
        // 后端契约：元数据通过 "metadata" 字段（IngestRequest JSON）传递，
        // 而不是顶层 "tags" 字段（会被后端忽略并静默丢弃）。
        formData.append('metadata', JSON.stringify({ tags }));
      }
      await apiPostMultipart('/knowledge/ingest', formData);
      setIngestSuccess(true);
      setSelectedFiles([]);
      setTagsInput('');
    } catch (err) {
      setIngestError(err instanceof Error ? err.message : '导入失败');
    } finally {
      setIsIngesting(false);
    }
  };

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

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-6">
        {activeTab === 'search' ? (
          <div className="space-y-4">
            {/* Search input */}
            <div className="relative">
              <Search
                size={18}
                className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--color-text-tertiary)]"
              />
              <input
                type="text"
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                placeholder="搜索知识库..."
                aria-label="搜索知识库"
                className="w-full pl-10 pr-10 py-2.5 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-sm text-[var(--color-text-primary)] placeholder:text-[var(--color-text-tertiary)] focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-500 transition-colors"
              />
              {isSearching && (
                <Loader2
                  size={16}
                  className="absolute right-3 top-1/2 -translate-y-1/2 text-[var(--color-text-tertiary)] animate-spin"
                  aria-label="正在搜索"
                />
              )}
            </div>

            {/* Search error */}
            {searchError && (
              <div
                role="alert"
                className="flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
              >
                <AlertCircle size={16} />
                <span>{searchError}</span>
              </div>
            )}

            {/* Results list */}
            {!searchError && totalResults > 0 && (
              <div className="space-y-2">
                <p className="text-xs text-[var(--color-text-tertiary)]">
                  共找到 {totalResults} 条结果
                </p>
                {searchResults.map((result) => (
                  <div
                    key={result.id}
                    className="rounded-lg border border-[var(--color-border)] overflow-hidden transition-shadow hover:shadow-sm"
                  >
                    <button
                      onClick={() => toggleResultExpand(result.id)}
                      className="w-full flex items-start justify-between p-4 text-left hover:bg-[var(--color-bg-hover)] transition-colors"
                    >
                      <div className="flex-1 min-w-0">
                        <h4 className="text-sm font-medium text-[var(--color-text-primary)] truncate">
                          <HighlightedText
                            text={result.metadata?.title ?? ''}
                            query={searchQuery}
                          />
                        </h4>
                        <div className="flex items-center gap-3 mt-1">
                          <span className="text-xs text-blue-600 dark:text-blue-400 font-medium">
                            {Math.round(result.score * 100)}%
                          </span>
                          <span className="text-xs text-[var(--color-text-tertiary)] truncate">
                            {result.source}
                          </span>
                        </div>
                      </div>
                      {expandedResultId === result.id ? (
                        <ChevronUp
                          size={16}
                          className="mt-1 shrink-0 text-[var(--color-text-tertiary)]"
                        />
                      ) : (
                        <ChevronDown
                          size={16}
                          className="mt-1 shrink-0 text-[var(--color-text-tertiary)]"
                        />
                      )}
                    </button>
                    {expandedResultId === result.id && (
                      <div className="px-4 pb-4">
                        <div className="p-3 rounded-md bg-[var(--color-bg-secondary)] text-sm text-[var(--color-text-secondary)] leading-relaxed">
                          <HighlightedText text={result.content} query={searchQuery} />
                        </div>
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )}

            {/* Empty state when searching with no results */}
            {!isSearching &&
              !searchError &&
              searchQuery.trim() !== '' &&
              searchResults.length === 0 && (
                <div className="flex flex-col items-center justify-center py-16 text-[var(--color-text-tertiary)]">
                  <Search size={40} className="mb-3 opacity-40" />
                  <p className="text-sm">未找到匹配结果</p>
                </div>
              )}

            {/* Initial empty state */}
            {searchQuery.trim() === '' && !searchError && (
              <div className="flex flex-col items-center justify-center py-16 text-[var(--color-text-tertiary)]">
                <BookOpen size={40} className="mb-3 opacity-40" />
                <p className="text-sm">输入关键词搜索知识库内容</p>
              </div>
            )}
          </div>
        ) : activeTab === 'browse' ? (
          <div className="space-y-4">
            {/* Breadcrumb */}
            {browsePath.length > 0 && (
              <div className="flex items-center gap-1 text-xs text-[var(--color-text-secondary)]">
                <button onClick={() => setBrowsePath([])} className="hover:text-blue-600">
                  知识库
                </button>
                {browsePath.map((seg, i) => (
                  <span key={i} className="flex items-center gap-1">
                    <ChevronRight size={12} />
                    <button
                      onClick={() => setBrowsePath(browsePath.slice(0, i + 1))}
                      className="hover:text-blue-600"
                    >
                      {seg}
                    </button>
                  </span>
                ))}
              </div>
            )}
            {/* 查看层级选择 */}
            <div className="flex items-center gap-2 mb-2">
              <label className="text-xs text-[var(--color-text-tertiary)]">查看层级:</label>
              {(['abstract', 'overview', 'detail'] as const).map((lvl) => (
                <button
                  key={lvl}
                  onClick={() => setBrowseLevel(lvl)}
                  className={`px-2 py-0.5 text-xs rounded border ${browseLevel === lvl ? 'border-blue-500 bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300' : 'border-[var(--color-border)] text-[var(--color-text-secondary)]'}`}
                >
                  {lvl === 'abstract' ? 'L0 摘要' : lvl === 'overview' ? 'L1 概览' : 'L2 详情'}
                </button>
              ))}
              <button
                onClick={loadBrowseEntries}
                disabled={browseLoading}
                className="ml-auto px-2 py-0.5 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
              >
                {browseLoading ? '刷新中...' : '刷新'}
              </button>
            </div>
            {browseError && (
              <div
                role="alert"
                className="flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
              >
                <AlertCircle size={16} />
                <span>{browseError}</span>
              </div>
            )}
            {browseLoading ? (
              <div className="flex items-center justify-center py-10">
                <Loader2 size={20} className="animate-spin text-[var(--color-text-tertiary)]" />
              </div>
            ) : (
              <div className="space-y-1">
                {browseEntries.map((entry) => (
                  <div
                    key={entry.uri}
                    onClick={() => handleBrowseView(entry)}
                    className={`flex items-center justify-between px-3 py-2 rounded-md cursor-pointer transition-colors ${selectedBrowseEntry?.uri === entry.uri ? 'bg-blue-50 dark:bg-blue-900/20 border border-blue-200 dark:border-blue-800' : 'border border-transparent hover:bg-[var(--color-bg-hover)]'}`}
                  >
                    <div className="flex items-center gap-2 min-w-0">
                      {entry.is_directory ? (
                        <Folder size={16} className="shrink-0 text-yellow-500" />
                      ) : (
                        <File size={16} className="shrink-0 text-[var(--color-text-tertiary)]" />
                      )}
                      <span className="text-sm text-[var(--color-text-primary)] truncate">
                        {entry.name}
                      </span>
                    </div>
                  </div>
                ))}
                {browseEntries.length === 0 && !browseLoading && !browseError && (
                  <p className="text-sm text-[var(--color-text-tertiary)] py-8 text-center">
                    知识库暂无条目
                  </p>
                )}
              </div>
            )}

            {selectedBrowseEntry && (
              <div className="border-t border-[var(--color-border)] pt-4">
                <div className="flex items-center justify-between mb-3">
                  <h4 className="text-sm font-medium text-[var(--color-text-primary)] truncate max-w-[70%]">
                    {selectedBrowseEntry.name}
                  </h4>
                </div>
                {browseContentLoading ? (
                  <div className="flex items-center justify-center py-10">
                    <Loader2 size={20} className="animate-spin text-[var(--color-text-tertiary)]" />
                  </div>
                ) : (
                  <pre className="max-h-[300px] overflow-y-auto p-4 rounded-lg bg-[var(--color-bg-secondary)] text-sm text-[var(--color-text-secondary)] whitespace-pre-wrap font-mono leading-relaxed border border-[var(--color-border)]">
                    {browseContent || '(空内容)'}
                  </pre>
                )}
              </div>
            )}
          </div>
        ) : (
          <div className="space-y-6 max-w-lg">
            {/* Drag & drop zone */}
            <div
              onDragOver={handleDragOver}
              onDragLeave={handleDragLeave}
              onDrop={handleDrop}
              onClick={() => fileInputRef.current?.click()}
              role="button"
              tabIndex={0}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  fileInputRef.current?.click();
                }
              }}
              aria-label="拖拽文件到此处或点击选择文件"
              className={`relative flex flex-col items-center justify-center p-10 rounded-xl border-2 border-dashed transition-colors cursor-pointer ${
                dragOver
                  ? 'border-blue-500 bg-blue-50 dark:bg-blue-900/20'
                  : 'border-[var(--color-border)] hover:border-blue-400 hover:bg-[var(--color-bg-hover)]'
              }`}
            >
              <input
                ref={fileInputRef}
                type="file"
                multiple
                onChange={handleFileSelect}
                className="hidden"
                accept=".txt,.pdf,.md,.json,.csv,.xml,.yaml,.yml,.html,.htm"
              />
              <Inbox
                size={40}
                className={`mb-3 ${
                  dragOver ? 'text-blue-500' : 'text-[var(--color-text-tertiary)]'
                }`}
              />
              <p
                className={`text-sm font-medium ${
                  dragOver ? 'text-blue-600' : 'text-[var(--color-text-secondary)]'
                }`}
              >
                {dragOver ? '释放文件以上传' : '拖拽文件到此处，或点击选择'}
              </p>
              <p className="text-xs text-[var(--color-text-tertiary)] mt-1">
                支持 TXT、PDF、Markdown、JSON、CSV 等格式
              </p>
            </div>

            {/* Selected files list */}
            {selectedFiles.length > 0 && (
              <div className="space-y-2">
                <p className="text-sm font-medium text-[var(--color-text-primary)]">
                  已选择 {selectedFiles.length} 个文件
                </p>
                <div className="space-y-1">
                  {selectedFiles.map((file, i) => (
                    <div
                      key={`${file.name}-${i}`}
                      className="flex items-center justify-between px-3 py-2 rounded-md bg-[var(--color-bg-secondary)] border border-[var(--color-border)]"
                    >
                      <div className="flex items-center gap-2 min-w-0">
                        <File size={16} className="shrink-0 text-[var(--color-text-tertiary)]" />
                        <span className="text-sm text-[var(--color-text-primary)] truncate">
                          {file.name}
                        </span>
                        <span className="text-xs text-[var(--color-text-tertiary)] shrink-0">
                          {formatFileSize(file.size)}
                        </span>
                      </div>
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          removeFile(i);
                        }}
                        className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-[var(--color-text-tertiary)] hover:text-red-500"
                        aria-label={`删除文件 ${file.name}`}
                      >
                        <X size={14} />
                      </button>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* Tags input */}
            <div>
              <label className="block text-sm font-medium text-[var(--color-text-primary)] mb-1.5">
                标签（可选，逗号分隔）
              </label>
              <input
                type="text"
                value={tagsInput}
                onChange={(e) => setTagsInput(e.target.value)}
                placeholder="例如: 文档, 技术, 参考"
                aria-label="标签输入，逗号分隔"
                className="w-full px-3 py-2 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-sm text-[var(--color-text-primary)] placeholder:text-[var(--color-text-tertiary)] focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-500 transition-colors"
              />
            </div>

            {/* Submit button */}
            <button
              onClick={handleIngest}
              disabled={selectedFiles.length === 0 || isIngesting}
              className={`w-full flex items-center justify-center gap-2 px-4 py-2.5 rounded-lg text-sm font-medium transition-colors ${
                selectedFiles.length === 0 || isIngesting
                  ? 'bg-[var(--color-bg-secondary)] text-[var(--color-text-tertiary)] cursor-not-allowed'
                  : 'bg-blue-600 text-white hover:bg-blue-700 active:bg-blue-800'
              }`}
            >
              {isIngesting ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  正在导入...
                </>
              ) : (
                <>
                  <Upload size={16} />
                  开始导入
                </>
              )}
            </button>

            {/* Success message */}
            {ingestSuccess && (
              <div
                role="status"
                aria-live="polite"
                className="flex items-center gap-2 p-3 rounded-lg bg-green-50 dark:bg-green-950 border border-green-200 dark:border-green-800 text-green-700 dark:text-green-300 text-sm"
              >
                <CheckCircle2 size={16} />
                <span>文件导入成功！</span>
              </div>
            )}

            {/* Error message */}
            {ingestError && (
              <div
                role="alert"
                className="flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
              >
                <AlertCircle size={16} />
                <span>{ingestError}</span>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
