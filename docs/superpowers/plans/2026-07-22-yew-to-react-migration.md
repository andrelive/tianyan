# Yew WASM → React/TypeScript 前端迁移计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将天演的前端从 Yew 0.22 WASM 完全迁移到 React 18 + TypeScript + Vite，保留 Rust 后端（Axum + Core）和 Tauri 壳不变。

**Architecture:** Tauri 2.2 壳内嵌 Axum HTTP 服务（端口 3000），WebView 加载 React SPA。前端通过 HTTP/SSE 消费已有的 Axum API 端点。状态管理用 Zustand，UI 组件用 shadcn/ui + Tailwind CSS，流式对话用 Vercel AI SDK。

**Tech Stack:** React 18, TypeScript 5, Vite 5, Tailwind CSS 3, shadcn/ui, Zustand, Vercel AI SDK, React Router 6

**当前状态:** `gui/` 目录是 Yew WASM 前端，`gui-vite/` 将新建为 React 前端。两者在迁移期间共存，`tauri/` 构建脚本逐步切换。

---

## 文件结构

```
gui-vite/
├── package.json
├── tsconfig.json
├── vite.config.ts
├── tailwind.config.ts
├── postcss.config.js
├── index.html
├── src/
│   ├── main.tsx                    # ReactDOM 入口
│   ├── App.tsx                     # 根组件：路由 + 主题 + Toast
│   ├── index.css                   # Tailwind 指令 + 自定义 CSS 变量
│   ├── lib/
│   │   ├── types.ts                # 共享类型（ChatMessage, Session, Skill 等）
│   │   ├── api-client.ts           # fetch 封装 + base URL 检测
│   │   ├── store.ts                # Zustand store（AppState）
│   │   └── utils.ts                # 日期格式化等工具函数
│   ├── hooks/
│   │   ├── use-api-base.ts         # API base URL hook
│   │   ├── use-keyboard-shortcuts.ts
│   │   └── use-theme.ts            # 主题切换 hook
│   ├── components/
│   │   ├── layout/
│   │   │   ├── AppLayout.tsx       # Sidebar + 主内容区
│   │   │   └── Toast.tsx           # Toast 通知浮层
│   │   ├── chat/
│   │   │   ├── ChatPanel.tsx       # 聊天主面板（useChat）
│   │   │   ├── MessageBubble.tsx   # 单条消息（含 Markdown 渲染）
│   │   │   ├── ChatInput.tsx       # 输入区域
│   │   │   ├── ModelSelector.tsx   # 模型选择下拉框
│   │   │   └── SkillCallCard.tsx   # 技能/工具调用卡片
│   │   ├── sidebar/
│   │   │   ├── Sidebar.tsx         # 侧边栏容器
│   │   │   └── SessionItem.tsx     # 会话列表项
│   │   ├── settings/
│   │   │   └── SettingsPanel.tsx   # 设置页（10 个 tab）
│   │   ├── knowledge/
│   │   │   └── KnowledgePanel.tsx  # 知识管理页
│   │   ├── skills/
│   │   │   └── SkillsPanel.tsx     # 技能浏览页
│   │   └── wizard/
│   │       └── ConfigWizard.tsx    # 初次配置向导
│   └── vite-env.d.ts
└── components.json                  # shadcn/ui 配置
```

---

## Phase 0: 项目脚手架

### Task 0.1: 初始化 Vite + React + TypeScript 项目

**Files:**
- Create: `gui-vite/package.json`
- Create: `gui-vite/tsconfig.json`
- Create: `gui-vite/vite.config.ts`
- Create: `gui-vite/index.html`
- Create: `gui-vite/src/vite-env.d.ts`

- [ ] **Step 1: 创建项目目录并初始化**

```bash
mkdir gui-vite
cd gui-vite
npm init -y
```

- [ ] **Step 2: 安装核心依赖**

```bash
cd gui-vite
npm install react@18 react-dom@18 react-router-dom@6 zustand ai @ai-sdk/react
npm install -D typescript @types/react @types/react-dom vite @vitejs/plugin-react tailwindcss@3 postcss autoprefixer
```

- [ ] **Step 3: 创建 `package.json`**

```json
{
  "name": "tianyan-gui",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc && vite build",
    "preview": "vite preview"
  },
  "dependencies": {
    "ai": "^4.0.0",
    "@ai-sdk/react": "^1.0.0",
    "react": "^18.3.0",
    "react-dom": "^18.3.0",
    "react-router-dom": "^6.26.0",
    "zustand": "^4.5.0",
    "react-markdown": "^9.0.0",
    "react-syntax-highlighter": "^15.5.0",
    "lucide-react": "^0.400.0"
  },
  "devDependencies": {
    "@types/react": "^18.3.0",
    "@types/react-dom": "^18.3.0",
    "@types/react-syntax-highlighter": "^15.5.0",
    "@vitejs/plugin-react": "^4.3.0",
    "autoprefixer": "^10.4.0",
    "postcss": "^8.4.0",
    "tailwindcss": "^3.4.0",
    "typescript": "^5.5.0",
    "vite": "^5.4.0"
  }
}
```

- [ ] **Step 4: 创建 `tsconfig.json`**

```json
{
  "compilerOptions": {
    "target": "ES2020",
    "useDefineForClassFields": true,
    "lib": ["ES2020", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "skipLibCheck": true,
    "moduleResolution": "bundler",
    "allowImportingTsExtensions": true,
    "resolveJsonModule": true,
    "isolatedModules": true,
    "noEmit": true,
    "jsx": "react-jsx",
    "strict": true,
    "noUnusedLocals": false,
    "noUnusedParameters": false,
    "noFallthroughCasesInSwitch": true,
    "paths": {
      "@/*": ["./src/*"]
    },
    "baseUrl": "."
  },
  "include": ["src"]
}
```

- [ ] **Step 5: 创建 `vite.config.ts`**

```typescript
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  server: {
    port: 5173,
    proxy: {
      '/api': {
        target: 'http://localhost:3000',
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
});
```

- [ ] **Step 6: 创建 `index.html`**

```html
<!DOCTYPE html>
<html lang="zh-CN">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>天演 Tianyan</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

- [ ] **Step 7: 创建 `vite-env.d.ts`**

```typescript
/// <reference types="vite/client" />
```

- [ ] **Step 8: 验证项目能启动**

```bash
npm run dev
```

### Task 0.2: 配置 Tailwind CSS + shadcn/ui

**Files:**
- Create: `gui-vite/tailwind.config.ts`
- Create: `gui-vite/postcss.config.js`
- Modify: `gui-vite/src/index.css`

- [ ] **Step 1: 初始化 Tailwind**

```bash
cd gui-vite
npx tailwindcss init -p --ts
```

- [ ] **Step 2: 配置 `tailwind.config.ts`**

```typescript
import type { Config } from 'tailwindcss';

export default {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        accent: {
          DEFAULT: '#2563eb',
          hover: '#1d4ed8',
          light: '#eff6ff',
        },
      },
      spacing: {
        sidebar: '280px',
        'sidebar-collapsed': '64px',
        header: '60px',
      },
      borderRadius: {
        sm: '6px',
        md: '10px',
        lg: '16px',
      },
    },
  },
  plugins: [],
} satisfies Config;
```

- [ ] **Step 3: 创建基础 CSS (`src/index.css`)**

```css
@tailwind base;
@tailwind components;
@tailwind utilities;

:root {
  --color-bg-primary: #ffffff;
  --color-bg-secondary: #f5f5f5;
  --color-bg-tertiary: #e8e8e8;
  --color-bg-hover: #f0f0f0;
  --color-text-primary: #1a1a1a;
  --color-text-secondary: #666666;
  --color-text-tertiary: #999999;
  --color-border: #e0e0e0;
  --color-error: #dc2626;
  --color-error-bg: #fef2f2;
  --color-success: #16a34a;
  --color-warning: #f59e0b;
  --sidebar-width: 280px;
  --sidebar-collapsed-width: 64px;
}

.dark {
  --color-bg-primary: #1a1a2e;
  --color-bg-secondary: #16213e;
  --color-bg-tertiary: #0f3460;
  --color-bg-hover: #1a1a3e;
  --color-text-primary: #e4e4e7;
  --color-text-secondary: #a1a1aa;
  --color-text-tertiary: #71717a;
  --color-border: #27272a;
  --color-error: #ef4444;
  --color-error-bg: #450a0a;
  --color-success: #22c55e;
  --color-warning: #eab308;
}

* {
  box-sizing: border-box;
  margin: 0;
  padding: 0;
}

html, body, #root {
  height: 100%;
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
  font-size: 14px;
  line-height: 1.5;
  background-color: var(--color-bg-primary);
  color: var(--color-text-primary);
}
```

- [ ] **Step 4: 初始化 shadcn/ui**

```bash
cd gui-vite
npx shadcn-ui@latest init
# 选择: TypeScript, Default style, Slate base color, CSS variables = yes
```

---

## Phase 1: 核心基础设施

### Task 1.1: TypeScript 类型定义

**Files:**
- Create: `gui-vite/src/lib/types.ts`

- [ ] **Step 1: 创建 `src/lib/types.ts`** — 从现有 Rust 类型映射

```typescript
// ========== Chat Types ==========

export type MessageRole = 'system' | 'user' | 'assistant';

export type StreamChunkType = 'answer' | 'thought' | 'tool_call' | 'observation' | 'clarification' | 'error';

export interface ChatMessage {
  role: MessageRole;
  content: string;
  timestamp?: string;
  skill_calls?: SkillCallInfo[];
  chunk_type?: StreamChunkType;
}

export interface ChatRequest {
  session_id?: string;
  messages: ChatMessage[];
  stream: boolean;
  temperature: number;
  max_tokens: number;
  model?: string;
}

export interface ChatResponse {
  id: string;
  session_id: string;
  message: ChatMessage;
  usage: TokenUsage;
}

export interface TokenUsage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
}

export interface ChatStreamEvent {
  id: string;
  session_id: string;
  delta: string;
  finish_reason?: string;
  skill_calls?: SkillCallInfo[];
  chunk_type: StreamChunkType;
}

export interface SkillCallInfo {
  skill_id: string;
  skill_name: string;
  success: boolean;
  execution_time_ms: number;
  error?: string;
}

export interface RegenerateRequest {
  session_id: string;
  message_index: number;
}

export interface EditMessageRequest {
  session_id: string;
  message_index: number;
  new_content: string;
}

// ========== Session Types ==========

export interface Session {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  message_count: number;
}

// ========== Skill Types ==========

export interface SkillParameter {
  name: string;
  type: string;
  description: string;
  required: boolean;
  default_value?: unknown;
}

export interface Skill {
  id: string;
  name: string;
  description: string;
  parameters: SkillParameter[];
  category: string;
}

export interface SkillExecutionStatus {
  status: 'pending' | 'running' | 'completed' | 'failed';
  result?: string;
  error?: string;
  execution_time_ms: number;
}

export interface ExecuteSkillRequest {
  skill_id: string;
  params: Record<string, string>;
}

// ========== Knowledge Types ==========

export interface KnowledgeSearchResult {
  id: string;
  title: string;
  content: string;
  score: number;
  source: string;
}

// ========== Config Types ==========

export interface ConfigStatus {
  configured: boolean;
}

export interface ModelServiceState {
  name: string;
  service_type: string;
  endpoint: string;
  api_key: string;
  default_model: string;
  models: string[];
  timeout: number;
  enabled: boolean;
  priority: number;
  show_advanced: boolean;
  test_status: TestStatus;
}

export type TestStatus = 'idle' | 'testing' | 'success' | 'error';

export interface ConfigState {
  model_services: ModelServiceState[];
  default_chat_model: string;
  default_embedding_model: string;
  default_vision_model: string;
  data_dir: string;
  vector_url: string;
  collection_name: string;
  vector_dimension: number;
  max_storage_size: number;
  auto_cleanup: boolean;
  cleanup_days: number;
  enable_skills: boolean;
  enable_memory: boolean;
  stream_responses: boolean;
  enable_thinking: boolean;
  default_top_k: number;
  max_turns: number;
  learned_rules_top_k: number;
  learned_rules_max_tokens: number;
  log_level: string;
  log_format: string;
  log_max_file_size: number;
  log_max_files: number;
  log_include_timestamp: boolean;
  log_include_location: boolean;
  security_enabled: boolean;
  confirm_commands: boolean;
  audit_logging: boolean;
  max_file_size: number;
  allowed_directories: string;
  blocked_directories: string;
  allowed_commands: string;
  blocked_commands: string;
  max_session_memory: number;
  max_long_term_memory: number;
  importance_threshold: number;
  auto_consolidation: boolean;
  consolidation_interval: number;
  decay_rate: number;
  retrieval_top_k: number;
  min_score: number;
  two_stage_retrieval: boolean;
  l0_multiplier: number;
  max_context_tokens: number;
  enable_cache: boolean;
  cache_ttl: number;
}

// ========== App State Types ==========

export type View = 'chat' | 'skills' | 'knowledge' | 'settings';

export type StreamStatus = 'idle' | 'streaming' | 'error';

export type Theme = 'light' | 'dark' | 'system';

export type FontSize = 'small' | 'medium' | 'large';

export type ToastType = 'error' | 'success' | 'info';

export interface ToastMessage {
  message: string;
  type: ToastType;
}
```

### Task 1.2: API 客户端

**Files:**
- Create: `gui-vite/src/lib/api-client.ts`
- Create: `gui-vite/src/hooks/use-api-base.ts`

- [ ] **Step 1: 创建 `src/hooks/use-api-base.ts`**

```typescript
// 检测当前环境决定 API base URL
// 在 Tauri 中: http://localhost:3000/api/v1
// 在开发模式(Vite dev server): /api/v1 (通过 Vite proxy)
export function getApiBase(): string {
  const href = window.location.href;
  // Tauri 环境检测：不是 localhost:5173 时使用 localhost:3000
  if (href.includes('localhost:5173') || href.includes('127.0.0.1:5173')) {
    return '/api/v1';
  }
  return 'http://localhost:3000/api/v1';
}
```

- [ ] **Step 2: 创建 `src/lib/api-client.ts`**

```typescript
import { getApiBase } from '@/hooks/use-api-base';

const DEFAULT_TIMEOUT = 60000;

class ApiError extends Error {
  code?: string;
  constructor(message: string, code?: string) {
    super(message);
    this.name = 'ApiError';
    this.code = code;
  }
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
  signal?: AbortSignal
): Promise<T> {
  const url = `${getApiBase()}${path}`;
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    Accept: 'application/json',
  };

  const controller = new AbortController();
  const timeoutId = setTimeout(() => controller.abort(), DEFAULT_TIMEOUT);
  const finalSignal = signal || controller.signal;

  try {
    const response = await fetch(url, {
      method,
      headers,
      body: body ? JSON.stringify(body) : undefined,
      signal: finalSignal,
    });

    if (!response.ok) {
      const text = await response.text();
      let message = `HTTP ${response.status}: ${text}`;
      try {
        const err = JSON.parse(text);
        message = err.error || err.message || message;
      } catch {}
      throw new ApiError(message, String(response.status));
    }

    return await response.json();
  } finally {
    clearTimeout(timeoutId);
  }
}

export async function apiGet<T>(path: string): Promise<T> {
  return request<T>('GET', path);
}

export async function apiPost<T>(path: string, body: unknown): Promise<T> {
  return request<T>('POST', path, body);
}

export async function apiDelete<T>(path: string): Promise<T> {
  return request<T>('DELETE', path);
}

export async function apiPostMultipart<T>(path: string, formData: FormData): Promise<T> {
  const url = `${getApiBase()}${path}`;
  const response = await fetch(url, {
    method: 'POST',
    body: formData,
  });
  if (!response.ok) {
    const text = await response.text();
    throw new ApiError(`HTTP ${response.status}: ${text}`, String(response.status));
  }
  return response.json();
}

export { ApiError, getApiBase };
```

### Task 1.3: Zustand Store

**Files:**
- Create: `gui-vite/src/lib/store.ts`

Note: This module should mirror `gui/src/state/mod.rs` AppState + AppAction.

The store should include:
- `currentView: View`
- `currentSessionId: string | null`
- `sessions: Session[]`
- `messages: ChatMessage[]`
- `streamStatus: StreamStatus`
- `isSidebarOpen: boolean`
- `theme: Theme`
- `fontSize: FontSize`
- `skills: Skill[]`
- `currentSkillId: string | null`
- `toast: ToastMessage | null`
- `selectedModel: string | null`
- Actions: setView, setCurrentSession, setSessions, addSession, removeSession, setMessages, addMessage, updateLastMessage, appendSkillCalls, setStreamStatus, toggleSidebar, updateSettings, clearMessages, setSkills, setCurrentSkill, regenerateFrom, editMessage, deleteMessagesFrom, showToast, hideToast, setModel

### Task 1.4: 工具函数

**Files:**
- Create: `gui-vite/src/lib/utils.ts`

```typescript
// Date formatting (replaces utils.rs)
export function formatDate(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' });
}

export function formatTime(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
}

export function formatRelativeTime(iso: string): string {
  const d = new Date(iso);
  const now = new Date();
  const diffMs = now.getTime() - d.getTime();
  const diffMin = Math.floor(diffMs / 60000);
  if (diffMin < 1) return '刚刚';
  if (diffMin < 60) return `${diffMin}分钟前`;
  const diffHour = Math.floor(diffMin / 60);
  if (diffHour < 24) return `${diffHour}小时前`;
  const diffDay = Math.floor(diffHour / 24);
  if (diffDay < 7) return `${diffDay}天前`;
  return formatDate(iso);
}

// Group skills by category
export function groupByCategory<T extends { category: string }>(items: T[]): Map<string, T[]> {
  const map = new Map<string, T[]>();
  for (const item of items) {
    const list = map.get(item.category) || [];
    list.push(item);
    map.set(item.category, list);
  }
  return map;
}

// cn utility for className merging (shadcn/ui pattern)
export function cn(...classes: (string | boolean | undefined | null)[]): string {
  return classes.filter(Boolean).join(' ');
}
```

### Task 1.5: 主题 Hook

**Files:**
- Create: `gui-vite/src/hooks/use-theme.ts`

```typescript
import { useEffect } from 'react';

type Theme = 'light' | 'dark' | 'system';

export function useTheme(theme: Theme) {
  useEffect(() => {
    const root = document.documentElement;
    root.classList.remove('light', 'dark');

    if (theme === 'system') {
      const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
      root.classList.add(prefersDark ? 'dark' : 'light');
    } else {
      root.classList.add(theme);
    }
  }, [theme]);
}
```

### Task 1.6: 键盘快捷键 Hook

**Files:**
- Create: `gui-vite/src/hooks/use-keyboard-shortcuts.ts`

行为映射（对应 main.rs）:
- `Ctrl+N` → 新建会话
- `Ctrl+Shift+Delete` → 清空消息
- `Ctrl+,` → 打开设置

---

## Phase 2: 布局与导航

### Task 2.1: AppLayout + Sidebar

- 两栏布局：侧边栏 (`280px` / `64px` collapsed) + 主内容区
- 侧边栏内容：
  - 新建对话按钮
  - 导航项（对话 / 技能 / 知识 / 设置）— 使用 `<NavLink>`
  - 会话列表（按时间倒序）
  - 底部折叠/展开按钮
- 适配 `<Outlet>` 渲染子路由

### Task 2.2: App.tsx（路由 + 初始化）

- React Router 路由表：
  - `/` → `<Navigate to="/chat" />`
  - `/chat` → `<ChatPanel />`
  - `/chat/:sessionId` → `<ChatPanel />`
  - `/skills` → `<SkillsPanel />`
  - `/knowledge` → `<KnowledgePanel />`
  - `/settings` → `<SettingsPanel />`
- 初始化逻辑：检查配置状态 → 有配置进入主界面，无配置进入向导
- Toast 浮层渲染

### Task 2.3: Toast 组件

- 位置：右上角固定浮层
- 类型：error (红)、success (绿)、info (蓝)
- 3 秒自动消失
- 点击关闭

---

## Phase 3: 聊天面板（核心功能）

### Task 3.1: ChatPanel

- 使用 Vercel AI SDK `useChat` hook
- 配置：连接到 Axum SSE 端点 `POST /api/v1/chat/stream`
- 请求体格式：[现有 ChatRequest JSON](gui/src/api/chat.rs:66-78)
- 状态管理：通过 Zustand store

关键实现细节：
1. **session_id**：前端首次发消息时传 `null`，服务端返回带 `session_id` 的响应，前端保存到 store
2. **消息列表**：用 `messages` 数组，每条消息包含 `role` / `content` / `skill_calls`
3. **流式追加**：通过 SSE `data:` 行解析 `ChatStreamEvent`，delta 追加到 `store.updateLastMessage`
4. **工具调用**：`skill_calls` 字段渲染为 SkillCallCard
5. **停止生成**：AbortController 取消
6. **重新生成**：POST `/api/v1/chat/regenerate`
7. **编辑消息**：POST `/api/v1/chat/edit`
8. **自动滚动**：新消息到达时滚动到底部（仅在用户已在底部时）
9. **模型选择**：下拉框切换 `selectedModel`

### Task 3.2: ChatInput

- 多行 textarea，Shift+Enter 换行，Enter 发送
- 发送/停止按钮（根据 streamStatus 切换）
- 自动调整高度（最大 200px）
- 发送后清空输入框

### Task 3.3: MessageBubble

- 用户消息：右对齐，不同背景色
- 助手消息：左对齐
- Markdown 渲染：`react-markdown` + `react-syntax-highlighter` 代码块高亮
- 工具调用卡片：嵌入在消息内容中，可折叠
- 消息操作按钮（hover 显示）：复制、重新生成、编辑、删除
- 流式状态：末尾闪烁光标

### Task 3.4: SkillCallCard

- 显示技能名称、执行状态（pending/running/success/failed）
- 成功：绿色边框 + 执行时间
- 失败：红色边框 + 错误信息
- 可展开查看参数和结果

---

## Phase 4: 侧边栏与会话管理

### Task 4.1: Sidebar

- 顶部：Logo + 应用名（天演）
- 新建对话按钮
- 导航项（4 个图标按钮，对应 4 个 View）
- 会话列表：滚动区域，按 updated_at 倒序
- 底部：折叠按钮 + 设置按钮
- 折叠态：仅图标，64px 宽

### Task 4.2: SessionItem

- 显示会话标题（截断 20 字） + 时间
- 当前选中项高亮
- hover 显示删除按钮（X）
- 点击切换当前会话（加载历史消息）

接口:
- `GET /api/v1/sessions` → 获取会话列表
- `POST /api/v1/sessions` → 创建会话
- `DELETE /api/v1/sessions/:id` → 删除会话

---

## Phase 5: 设置面板

### Task 5.1: SettingsPanel

10 个 tab，每个 tab 有自己的表单字段（对应 ConfigState 字段）：
1. **模型服务** — 管理 LLM 提供商（添加/删除/测试连接/展开高级设置）
2. **数据存储** — VFS 路径、向量数据库配置
3. **Agent 行为** — skills/memory/streaming/thinking/max_turns 开关
4. **安全** — 命令确认、审计日志、文件访问限制
5. **日志** — 日志级别、格式、文件轮转
6. **记忆** — 记忆容量、重要性阈值、衰减率
7. **检索** — top_k、min_score、two_stage 开关
8. **外观** — 主题 (Light/Dark/System)、字号 (S/M/L)
9. **连接** — API 服务器地址
10. **关于** — 版本信息 + 致谢

保存：整个配置对象 POST 到 `/api/v1/config`
加载：GET `/api/v1/config` 在进入设置页时调用

外观设置（theme/fontSize）存储在 Zustand store 中，不随 config 保存。

---

## Phase 6: 知识面板

### Task 6.1: KnowledgePanel

两个子 tab：
1. **搜索** — 搜索框（300ms 防抖）、建议列表、搜索结果列表、结果详情
2. **导入** — 拖拽上传区域、文件列表、tags 输入、导入进度

接口:
- `POST /api/v1/knowledge/search` → 搜索知识库
- `POST /api/v1/knowledge/ingest` → 导入文档

---

## Phase 7: 技能面板

### Task 7.1: SkillsPanel

- 技能列表（按 category 分组）
- 点击技能 → 右侧显示详情
- 技能详情：名称、描述、参数表单
- 执行按钮 → POST `/api/v1/skills/execute`
- 轮询执行状态 → GET `/api/v1/skills/:id/status`
- 显示执行结果

---

## Phase 8: 配置向导

### Task 8.1: ConfigWizard

5 步向导（对应现有 Yew 版本）：
1. **欢迎** — 介绍 + 下一步
2. **模型配置** — 添加第一个 LLM 提供商
3. **数据配置** — VFS 路径 + 向量数据库
4. **Agent 配置** — 基本行为设定
5. **确认** — 汇总 + 保存

保存后调用 `POST /api/v1/config`，成功则跳转到主界面。

---

## Phase 9: Tauri 集成

### Task 9.1: 更新 Tauri 配置

**Files:**
- Modify: `tauri/tauri.conf.json`

修改：
```json
{
  "build": {
    "beforeBuildCommand": "npm run build",
    "beforeBuildCommand_cwd": "../gui-vite",
    "frontendDist": "../gui-vite/dist"
  }
}
```

CSP 保持不变（已有 `http://localhost:3000` 和 `http://127.0.0.1:3000` 的白名单）。

### Task 9.2: 更新构建脚本

**Files:**
- Modify: `scripts/build.ps1`

```powershell
# 在构建 tauri 之前构建 React 前端
Write-Host "Building React frontend..."
Push-Location gui-vite
npm install
npm run build
Pop-Location

# 原有 Tauri 构建逻辑...
```

---

## Phase 10: 清理

### Task 10.1: 移除 Yew 依赖

**Files:**
- Modify: `Cargo.toml` (workspace root — 移除 `gui/` member)
- Delete: `gui/` 目录（或归档到 `gui-yew-legacy/`）
- Modify: `tauri/Cargo.toml` — 移除对 Yew GUI 的隐式依赖
- Modify: `README.md` — 更新技术栈描述

### Task 10.2: 验证完整性

- [ ] `cargo check --workspace` 通过
- [ ] `npm run build` 在生产模式成功
- [ ] Tauri dev 模式启动正常（`cd tauri && cargo tauri dev`）
- [ ] SSE 流式对话正常工作
- [ ] 所有面板切换无报错
- [ ] 暗色主题切换生效
- [ ] 键盘快捷键生效
