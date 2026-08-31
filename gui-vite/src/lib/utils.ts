export function formatDate(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' });
}

/** RFC3339 → 本地时钟时间（HH:mm:ss；非法返回原文）。 */
export function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleTimeString('zh-CN', { hour12: false });
}

/** epoch 毫秒 → 本地日期时间（yyyy/M/d HH:mm:ss；空/非法返回 ''）。 */
export function formatTimestamp(ts?: number): string {
  if (!ts || ts <= 0) return '';
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return '';
  return d.toLocaleString('zh-CN', { hour12: false });
}

/** RFC3339 → 本地日期时间（yyyy/M/d HH:mm:ss；非法返回原文）。 */
export function formatDateTime(iso?: string): string {
  if (!iso) return '';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString('zh-CN', { hour12: false });
}

/** 大数 → 千分位（en-US 分组，全前端统一口径）。 */
export function formatNumber(n: number): string {
  return n.toLocaleString('en-US');
}

/** 秒数 → 人类可读间隔（ADR-024 间隔制展示；如 21600 → "每 6 小时"）。 */
export function formatInterval(secs: number): string {
  if (secs >= 604800 && secs % 604800 === 0) return '每 ' + secs / 604800 + ' 周';
  if (secs >= 86400 && secs % 86400 === 0) {
    return secs === 86400 ? '每天' : '每 ' + secs / 86400 + ' 天';
  }
  if (secs >= 3600 && secs % 3600 === 0) return '每 ' + secs / 3600 + ' 小时';
  if (secs >= 60 && secs % 60 === 0) return '每 ' + secs / 60 + ' 分钟';
  return '每 ' + secs + ' 秒';
}

/** RFC3339 → 相对时间（刚刚/N分钟前/N小时前/N天前；超出 7 天回退日期）。 */
export function formatRelativeTime(iso: string): string {
  const d = new Date(iso);
  const now = new Date();
  const diffMs = now.getTime() - d.getTime();
  const diffMin = Math.floor(diffMs / 60000);
  if (diffMin < 1) return '刚刚';
  if (diffMin < 60) return diffMin + '分钟前';
  const diffHour = Math.floor(diffMin / 60);
  if (diffHour < 24) return diffHour + '小时前';
  const diffDay = Math.floor(diffHour / 24);
  if (diffDay < 7) return diffDay + '天前';
  return formatDate(iso);
}

export function groupByCategory<T extends { category: string }>(items: T[]): Map<string, T[]> {
  const map = new Map<string, T[]>();
  for (const item of items) {
    const list = map.get(item.category) || [];
    list.push(item);
    map.set(item.category, list);
  }
  return map;
}

export function cn(...classes: (string | boolean | undefined | null)[]): string {
  return classes.filter(Boolean).join(' ');
}
