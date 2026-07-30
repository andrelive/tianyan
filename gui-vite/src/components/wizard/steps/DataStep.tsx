import type { StepProps } from './wizard.types';

interface FieldRowProps {
  label: string;
  children: React.ReactNode;
  description?: string;
}

function FieldRow({ label, children, description }: FieldRowProps) {
  return (
    <div className="flex flex-col gap-1.5">
      <label className="text-sm font-medium text-[var(--color-text-primary)]">
        {label}
      </label>
      {children}
      {description && (
        <p className="text-xs text-[var(--color-text-tertiary)]">{description}</p>
      )}
    </div>
  );
}

export default function DataStep({
  data,
  onChange,
  onNext: _onNext,
  onBack: _onBack,
  errors: _errors,
}: StepProps) {
  return (
    <div>
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">
        数据存储配置
      </h2>
      <p className="text-xs text-[var(--color-text-tertiary)] mb-5">
        配置知识库和向量数据的存储位置。向量数据库使用嵌入式的 LanceDB，无需外部服务。
      </p>

      <div className="space-y-4">
        <FieldRow
          label="数据目录"
          description="文档和索引文件的本地存储路径"
        >
          <input
            type="text"
            value={data.dataDir}
            onChange={(e) => onChange({ dataDir: e.target.value })}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            placeholder="~/.local/share/tianyan"
            aria-label="数据目录"
          />
        </FieldRow>

        <FieldRow
          label="向量维度"
          description="向量嵌入的维度数，取决于使用的嵌入模型"
        >
          <select
            value={data.vectorDim}
            onChange={(e) => onChange({ vectorDim: parseInt(e.target.value) })}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            aria-label="向量维度"
          >
            <option value="384">384</option>
            <option value="768">768</option>
            <option value="1024">1024</option>
            <option value="1536">1536 (OpenAI ada-002 / text-embedding-3-small)</option>
            <option value="3072">3072 (text-embedding-3-large)</option>
          </select>
        </FieldRow>
      </div>
    </div>
  );
}
