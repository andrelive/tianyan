/**
 * 模型/能力目录切片（modelSlice）—— 会话级模型选择（对齐 DSH 的
 * Session-local model selection）+ 聊天模型目录。
 *
 * 视图字段（selectedModel / thinkingEffort）= **当前会话**的选择投影：
 * - 会话有记录（`sessionModelSelections[key]`）→ 显示该记录；
 * - 无记录 → 回落全局默认（`defaultModelSelection`，来自 preferences.chat）。
 * 选择动作只改当前会话（写 map + 调后端会话 API，见 ModelSelector/ThinkingSelect），
 * **不再改写全局配置**（避免"切会话即切全局模型"导致的上下文缓存击穿）；
 * 全局默认仍在设置页配置。
 */

import type { StateCreator } from 'zustand';
import type { ModelInfo } from '@/lib/types';

/** 当前选中的聊天模型（provider+model 复合引用；同名模型跨 provider 的唯一标识）。 */
export interface SelectedModel {
  provider: string;
  model: string;
}

/** 会话级模型选择（与后端 SessionModelSelection 对齐；thinking 缺省 = off）。 */
export interface SessionModelSelectionValue {
  provider: string;
  model: string;
  thinking?: string;
}

export interface ModelSlice {
  // Model
  selectedModel: SelectedModel | null;
  setModel: (model: SelectedModel | null) => void;

  // 会话级思考强度档位（对话时选择，随每次请求下发；值为当前模型声明的档位）
  thinkingEffort: string;
  setThinkingEffort: (effort: string) => void;

  // 聊天模型目录（含每模型思考档位；ThinkingSelect 按当前模型档位渲染）
  chatModels: ModelInfo[];
  setChatModels: (models: ModelInfo[]) => void;

  /** 全局默认聊天模型（preferences.chat；新会话 / 无记录会话回落）。 */
  defaultModelSelection: SelectedModel | null;
  setDefaultModelSelection: (model: SelectedModel | null) => void;

  /** 每会话模型选择（键 = sessionId ?? PENDING_SESSION_KEY；服务端来源 + PENDING 槽）。 */
  sessionModelSelections: Record<string, SessionModelSelectionValue>;
  /** 写入/清除某会话的选择（只动 map；视图同步由 `syncModelView` 完成）。 */
  setSessionSelection: (key: string, selection: SessionModelSelectionValue | null) => void;
  /** 按会话键同步视图（切会话 / 选择写入 / 拉取完成后调用；无记录回落默认）。 */
  syncModelView: (key: string) => void;
}

export const createModelSlice: StateCreator<ModelSlice, [], [], ModelSlice> = (set) => ({
  // Model
  selectedModel: null,
  setModel: (model) => set({ selectedModel: model }),

  // 会话级思考强度（默认关闭；非 off 时对支持思考的模型生效）
  thinkingEffort: 'off',
  setThinkingEffort: (effort) => set({ thinkingEffort: effort }),
  chatModels: [],
  setChatModels: (models) => set({ chatModels: models }),

  defaultModelSelection: null,
  setDefaultModelSelection: (model) => set({ defaultModelSelection: model }),

  sessionModelSelections: {},
  setSessionSelection: (key, selection) =>
    set((s) => {
      const sessionModelSelections = { ...s.sessionModelSelections };
      if (selection) {
        sessionModelSelections[key] = selection;
      } else {
        delete sessionModelSelections[key];
      }
      return { sessionModelSelections };
    }),
  syncModelView: (key) =>
    set((s) => {
      const selection = s.sessionModelSelections[key];
      if (selection) {
        return {
          selectedModel: { provider: selection.provider, model: selection.model },
          thinkingEffort: selection.thinking ?? 'off',
        };
      }
      return {
        selectedModel: s.defaultModelSelection,
        thinkingEffort: 'off',
      };
    }),
});
