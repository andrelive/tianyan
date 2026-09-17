/**
 * 流响应看门狗（T1-8 自愈）。
 *
 * 问题：流的"收尾"依赖事件必达（`finish_reason` / `error` / `turn_state`），
 * 而事件通道是**有界尽力而为**（Lag 丢弃）+ SSE 断线期间事件全丢
 * （`use-unified-events` 的 `onerror` 只依赖自动重连）——于是前端
 * `streamStatus` / `turnState` 可能**永久停在 streaming/running**：消息区不再动、
 * 输入区一直显示停止、发送被禁用（用户观感"卡死"，实测 300s+）。
 *
 * 自愈策略：**任何 chat_stream 事件都"喂狗"**；某会话超过阈值无**任何**事件
 * → 判定异常 → 复位流状态并提示。**不复位消息内容**（截断保留 + 内联标记），
 * 后续若服务端仍在跑，增量事件到达仍可继续渲染（仅状态指示复位，不丢内容）。
 */
import { useAppStore } from '@/lib/store';

/** 无任何事件多久判定异常（前台长工具执行也会喂狗：工具开始/结果都发事件）。 */
export const STREAM_IDLE_TIMEOUT_MS = 180_000;
/** 检查周期（低频，仅遍历内存中的流状态表）。 */
const CHECK_INTERVAL_MS = 15_000;

/** 会话 → 最后一次收到事件的时间（模块级：与 store 生命周期解耦）。 */
const lastActivity = new Map<string, number>();

/** 喂狗：任何 chat_stream 事件到达时调用（无会话归属的事件忽略）。 */
export function touchStreamActivity(sid: string | null | undefined): void {
  if (sid) lastActivity.set(sid, Date.now());
}

/** 测试辅助：清空喂狗记录（模块级状态，测试间隔离）。 */
export function __resetStreamWatchdog(): void {
  lastActivity.clear();
}

/**
 * 启动看门狗（应用级常驻；返回停止函数）。
 *
 * 判定对象：`streamStatus === 'streaming'` 或 `turnState === 'running'` 的会话。
 * 首次见到的会话先记起点（不立即判定），下一周期起才可能超时。
 */
export function startStreamWatchdog(): () => void {
  const timer = setInterval(() => {
    const st = useAppStore.getState();
    const now = Date.now();

    /** 是否已超时（首见先记起点，返回 false）。 */
    const isStale = (sid: string): boolean => {
      const last = lastActivity.get(sid);
      if (last === undefined) {
        lastActivity.set(sid, now);
        return false;
      }
      return now - last > STREAM_IDLE_TIMEOUT_MS;
    };

    for (const [sid, status] of Object.entries(st.streamStatus)) {
      if (status !== 'streaming' || !isStale(sid)) continue;
      st.setStreamStatus('idle', sid);
      st.markLastMessageInterrupted(sid);
      st.showToast('长时间无响应，已复位流状态（服务端可能仍在处理）', 'error');
      lastActivity.delete(sid);
    }

    for (const [sid, turn] of Object.entries(st.turnState)) {
      if (turn.state !== 'running' || !isStale(sid)) continue;
      // 轮状态复位（唤醒轮/用户轮同为权威状态的镜像）：输入区解锁
      st.setTurnState(sid, { state: 'idle', auto: turn.auto });
      lastActivity.delete(sid);
    }
  }, CHECK_INTERVAL_MS);
  return () => clearInterval(timer);
}
