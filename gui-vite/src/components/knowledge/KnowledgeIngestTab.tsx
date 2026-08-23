import { useRef, useState } from 'react';
import { apiPostMultipart } from '@/lib/api-client';
import { toErrorMessage } from '@/lib/errors';
import type { IngestResponse } from '@/lib/types';
import { Inbox, File, X, Loader2, CheckCircle2, AlertCircle, Upload } from 'lucide-react';

/** 知识库导入（拖拽/选择 + 标签 + 分文件结果呈现）。 */
export default function KnowledgeIngestTab() {
  const [dragOver, setDragOver] = useState(false);
  const [selectedFiles, setSelectedFiles] = useState<File[]>([]);
  const [tagsInput, setTagsInput] = useState('');
  const [isIngesting, setIsIngesting] = useState(false);
  const [ingestSuccess, setIngestSuccess] = useState(false);
  const [ingestError, setIngestError] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

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
      const resp = await apiPostMultipart<IngestResponse>('/knowledge/ingest', formData);
      // 后端 HTTP 200 但可能携带文件级失败（success=false 或个别文件 status=failed）：
      // 必须如实呈现，不能无条件显示"导入成功"。
      const failed = (resp.files ?? []).filter((f) => f.status === 'failed');
      if (resp.success && failed.length === 0) {
        setIngestSuccess(true);
        setSelectedFiles([]);
        setTagsInput('');
      } else {
        const detail = failed[0]?.error ?? resp.message ?? '导入失败';
        setIngestError(`${failed.length} 个文件导入失败：${detail}`);
      }
    } catch (err) {
      setIngestError(toErrorMessage(err, '导入失败'));
    } finally {
      setIsIngesting(false);
    }
  };

  return (
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
          className={`mb-3 ${dragOver ? 'text-blue-500' : 'text-[var(--color-text-tertiary)]'}`}
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
  );
}
