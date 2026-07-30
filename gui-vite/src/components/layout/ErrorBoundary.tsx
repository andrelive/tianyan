import { Component, type ErrorInfo, type ReactNode } from 'react';

interface Props {
  children: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
}

/**
 * 错误边界组件
 * 捕获子组件渲染过程中的 JavaScript 错误，防止白屏崩溃
 * 显示友好的错误提示和重试按钮
 */
export default class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, errorInfo: ErrorInfo): void {
    console.error('[ErrorBoundary] 捕获渲染错误:', error, errorInfo);
  }

  /** 重置错误状态，重新渲染子组件 */
  handleRetry = (): void => {
    this.setState({ hasError: false, error: null });
  };

  render(): ReactNode {
    if (this.state.hasError) {
      return (
        <div className="h-screen flex items-center justify-center bg-[var(--color-bg-primary)]">
          <div role="alert" className="flex flex-col items-center gap-4 max-w-md text-center px-6">
            {/* 错误图标 */}
            <div className="w-14 h-14 rounded-full bg-red-50 dark:bg-red-950 flex items-center justify-center">
              <svg
                className="w-7 h-7 text-red-500"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={2}
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z"
                />
              </svg>
            </div>

            {/* 错误标题 */}
            <div>
              <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">
                页面渲染出错
              </h2>
              <p className="mt-1 text-sm text-[var(--color-text-secondary)]">
                应用遇到了一个意外错误，请尝试刷新或重试。
              </p>
            </div>

            {/* 错误详情（仅开发环境显示） */}
            {import.meta.env.DEV && this.state.error && (
              <details className="w-full text-left">
                <summary className="text-xs text-[var(--color-text-secondary)] cursor-pointer hover:text-[var(--color-text-primary)]">
                  错误详情
                </summary>
                <pre className="mt-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-xs text-red-700 dark:text-red-300 overflow-auto max-h-40">
                  {this.state.error.message}
                </pre>
              </details>
            )}

            {/* 操作按钮 */}
            <div className="flex items-center gap-3">
              <button
                onClick={this.handleRetry}
                className="px-4 py-2 rounded-lg bg-[var(--color-accent)] text-white text-sm font-medium
                  hover:opacity-90 transition-opacity"
              >
                重试
              </button>
              <button
                onClick={() => window.location.reload()}
                className="px-4 py-2 rounded-lg border border-[var(--color-border)] text-[var(--color-text-secondary)] text-sm
                  hover:bg-[var(--color-bg-secondary)] transition-colors"
              >
                刷新页面
              </button>
            </div>
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}
