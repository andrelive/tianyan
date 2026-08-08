# ADR-010: 对话多模态链路 —— 图片输入 + MCP 截图落盘

**日期**: 2026-08
**状态**: ✅ 已采纳
**影响范围**: 消息模型（`common/types/`）、协调器签名（`agent/`）、provider 序列化（`model/provider/`）、MCP 桥接（`server/src/mcp_bridge.rs`）

---

## 背景

能力缺口审查识别出两项感知缺口：

1. **对话图片输入完全缺失**：从消息模型到前端 UI 五个层级均不支持图片——`StructuredMessage::Part` 无图片变体、`Message.content` 为纯 `String`、provider 序列化无 `image_url` 分支、API 请求体无图片字段、前端无上传入口。底层已有独立的 VLM 管道（`model/provider/vision.rs` 完整实现 base64 → OpenAI image_url），但未与 chat 管道打通。
2. **MCP 图片返回被丢弃**：`mcp/src/client.rs::extract_text_content` 把 `type: "image"` content block 降级为 `[Image: <mime> (<bytes>)]` 占位符，base64 数据直接丢失——Playwright 等浏览器 MCP 的截图对 LLM 不可见、不可访问。

## 决策

### 1. 多模态消息模型（图片输入链路）

- **`ContentPart` / `ImageUrl` 上移 common 层**（`common/types/content_part.rs`）：原定义于 `model/types/vision.rs`（VLM 专用），现作为基础 DTO 供对话消息与视觉请求共用；`model/types/vision.rs` re-export 保持 `crate::model::types::ContentPart` 路径兼容（`knowledge/image/analyzer.rs` 等调用方零改动）。
- **`Message.content_parts: Option<Vec<ContentPart>>`**（`message.rs`）：仅用户消息携带图片时使用；`content` 保持纯文本（可为空，图片消息以图为主体）。`#[serde(skip_serializing_if = "Option::is_none", default)]` 保证旧会话 JSON 向后兼容。新增 `Message::user_with_images()` / `image_urls()`。
- **`Part::Image { url, time }`**（`structured_message.rs`）：持久化 data URL（`data:image/png;base64,...`），随会话 JSONL 存储，历史重放时由 `ContextAssembler::structured_to_messages` 转回多模态传输消息。
- **签名穿透**：`AgentCoordinator::process_message` / `process_message_stream` 参数由 `&str` 改为 `&Message`；`prepare_context` / `persist_user_message` / `run_agent_turn` 同步。server 层 `ChatMessage.images`（data URL 列表）→ `to_core_message` 组装。
- **provider 序列化**（`model/provider/chat.rs::user_content`）：携带图片时输出 OpenAI 数组格式（text + image_url content parts，`ImageDetail` 映射 low/high/auto），无图片保持纯文本（行为零变化）。

### 2. MCP 图片落盘（浏览器截图链路）

- **`McpImage` / `McpCallOutput`**（`mcp/src/client.rs`）：`call_tool_detailed` 保留完整 base64 图片数据；`call_tool` 委托并保持返回 `String` 兼容。
- **桥接层落盘**（`server/src/mcp_bridge.rs`）：`McpToolBridge` 配置 `image_dir` 时，图片 base64 解码保存到 `{data_dir}/mcp_images/`（与 snapshot 同为运行时产物，不经 VFS），并以 `[截图已保存: <path>]` 追加到工具结果文本——截图对 LLM 可见、可被 read_file 等工具后续访问；单图失败不阻断整体；未配置时保持占位符降级。

## 后果

### 正面
- 对话图片输入五层全打通：粘贴/拖拽/选图 → API → 多模态 LLM 请求 → 持久化 → 历史重放与回显
- MCP 浏览器截图（Playwright 等）对 LLM 可见可访问，解锁浏览器感知的视觉侧
- 两链路共享同一套 `ContentPart` DTO，未来 vision 模型自动路由可直接复用

### 负面 / 代价
- `Message` 与协调器签名变更波及全链路（coordinator trait、WizardModeAgent stub、server services）——已一次性穿透
- 图片 data URL 持久化到 JSONL 会增大会话文件体积（前端限 4MB/张、最多 4 张）
- chat 模型不支持 vision 时，图片请求返回模型错误（由 provider 报错呈现，未做模型能力自动检测）

### 边界条件（违反即重新评估）
- `Message.content_parts` 仅用户消息使用，assistant/tool 消息不得携带
- 图片必须为 data URL（server validate 强制 `data:` 前缀），拒绝任意 URL（SSRF 面）
- MCP 图片落盘目录固定为 `{data_dir}/mcp_images/`，不写入 VFS（运行时产物，与 snapshot 同级例外）

## 关键文件

- `core/src/common/types/content_part.rs` — `ContentPart` / `ImageUrl`（含 `text()` / `image()` 构造器）
- `core/src/common/types/message.rs` — `content_parts` / `user_with_images` / `image_urls`
- `core/src/common/types/structured_message.rs` — `Part::Image`
- `core/src/context/assembler.rs` — `structured_to_messages` 图片重放 + `message_to_structured` 持久化
- `core/src/agent/coordinator.rs` / `agent_core.rs` — 签名 `&Message` 穿透
- `core/src/model/provider/chat.rs` — `user_content` 多模态数组
- `mcp/src/client.rs` — `McpImage` / `McpCallOutput` / `call_tool_detailed`
- `server/src/mcp_bridge.rs` — `persist_images` / `append_image_paths`
- `server/src/api/chat/{types,services}.rs` — `images` 字段 + `to_core_message`
- `gui-vite/src/components/chat/ChatInput.tsx` — 图片选择/粘贴/拖拽/预览
