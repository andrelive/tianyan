/**
 * fetch + AbortSignal 的单一封装（C9 卫生项）。
 *
 * 背景：vitest jsdom 环境里 `new AbortController()` 产生 jsdom realm 的
 * AbortSignal，而 Node fetch（undici）只接受本 realm 的 signal——跨 realm
 * 会抛 `TypeError: Expected signal`。此前 api-client 与 useChatStream
 * 各有一份降级分支；本模块为唯一实现。生产浏览器同 realm，此分支永不
 * 触发；降级路径下 abort 退化为无效操作——仅影响测试环境。
 */

/**
 * fetch 包装：显式传入 signal 时若被环境拒绝（跨 realm），降级为不带
 * signal 的请求（测试环境专用；生产永不命中）。
 */
export async function fetchWithSignal(
  url: string,
  init: RequestInit,
  signal?: AbortSignal,
): Promise<Response> {
  try {
    return await fetch(url, { ...init, signal });
  } catch (err) {
    if (signal === undefined) throw err;
    if (err instanceof TypeError && /Expected signal/.test(err.message)) {
      return fetch(url, init);
    }
    throw err;
  }
}
