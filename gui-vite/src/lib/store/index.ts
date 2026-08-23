/**
 * 全局状态组合根 —— 切片归位后的单一出口。
 *
 * 接口宽度：三个切片（chatSlice / uiSlice / modelSlice）各自 8-15 个动作，
 * 取代原先 45 个顶层动作的扁平结构；消费者仍从 `@/lib/store` 导入
 * `useAppStore` / `PENDING_SESSION_KEY`（导入面不变）。
 */

import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { createChatSlice, PENDING_SESSION_KEY, type ChatSlice } from './chat-slice';
import { createUiSlice, type UiSlice } from './ui-slice';
import { createModelSlice, type ModelSlice } from './model-slice';

export type AppState = ChatSlice & UiSlice & ModelSlice;

/** 待创建会话的本地消息键（见 chat-slice；保持旧导入面）。 */
export { PENDING_SESSION_KEY };
export type { ChatSlice, UiSlice, ModelSlice };

export const useAppStore = create<AppState>()(
  persist(
    (set, get, api) => ({
      ...createChatSlice(set, get, api),
      ...createUiSlice(set, get, api),
      ...createModelSlice(set, get, api),
    }),
    {
      name: 'tianyan-ui-preferences',
      // 只持久化 UI 偏好（theme/fontSize/thinkingEffort）——会话/消息/任务
      // 数据不入 localStorage（ADR-018：会话权威在 SQLite）。
      partialize: (state) => ({
        theme: state.theme,
        fontSize: state.fontSize,
        thinkingEffort: state.thinkingEffort,
      }),
    },
  ),
);
