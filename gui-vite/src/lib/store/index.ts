/**
 * 全局状态组合根 —— 切片归位后的单一出口。
 *
 * 接口宽度：三个切片（chatSlice / uiSlice / modelSlice）各自 8-15 个动作，
 * 取代原先 45 个顶层动作的扁平结构；消费者仍从 `@/lib/store` 导入
 * `useAppStore` / `PENDING_SESSION_KEY`（导入面不变）。
 */

import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import {
  createChatSlice,
  PENDING_SESSION_KEY,
  type ChatSlice,
  type InputDraft,
} from './chat-slice';
import { createUiSlice, type UiSlice } from './ui-slice';
import { createModelSlice, type ModelSlice, type SelectedModel } from './model-slice';

export type AppState = ChatSlice & UiSlice & ModelSlice;

/** 待创建会话的本地消息键（见 chat-slice；保持旧导入面）。 */
export { PENDING_SESSION_KEY };
export type { ChatSlice, UiSlice, ModelSlice, SelectedModel, InputDraft };

/** 持久化的 UI 偏好子集（partialize 与 migrate 共用；类型由本函数推导）。 */
const partializePreferences = (state: AppState) => ({
  theme: state.theme,
  fontSize: state.fontSize,
  thinkingEffort: state.thinkingEffort,
  // 打开的会话（按打开顺序）——界面状态（非会话数据），随偏好持久化
  openSessionIds: state.openSessionIds,
});
type PersistedPreferences = ReturnType<typeof partializePreferences>;

export const useAppStore = create<AppState>()(
  persist(
    (set, get, api) => ({
      ...createChatSlice(set, get, api),
      ...createUiSlice(set, get, api),
      ...createModelSlice(set, get, api),
    }),
    {
      name: 'tianyan-ui-preferences',
      // 持久化格式版本：v0 = recentSessionIds（MRU 最近会话条）；v1 = openSessionIds
      //（浏览器式打开顺序 tab 条）。migrate 把 v0 数据平移（旧 MRU 顺序即初始打开顺序）。
      version: 1,
      migrate: (persisted, fromVersion) => {
        const state = { ...((persisted ?? {}) as Record<string, unknown>) };
        if (fromVersion < 1) {
          const legacy = state['recentSessionIds'];
          if (Array.isArray(legacy) && !Array.isArray(state['openSessionIds'])) {
            state['openSessionIds'] = legacy;
          }
          delete state['recentSessionIds'];
        }
        return state as unknown as PersistedPreferences;
      },
      // 只持久化 UI 偏好（theme/fontSize/thinkingEffort/openSessionIds）——会话/消息/任务
      // 数据不入 localStorage（ADR-018：会话权威在 SQLite）。
      partialize: partializePreferences,
    },
  ),
);
