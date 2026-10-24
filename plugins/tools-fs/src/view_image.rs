//! `view_image` 工具（ADR-050）：读本地图片 → 冻结副本 → 返回引用。
//!
//! 冻结副本落 `{data_dir}/session_images/{sha256}.{ext}`（同内容只存一份，幂等），
//! 工具结果只带**引用**（`session_images/<sha256>.<ext>`）——组装期按引用把字节
//! 加载成 data URL 随本条 tool 消息一起发给模型（模型直接看到像素）。
//!
//! 底层算法（magic bytes / sha256 / 冻结副本）在 core `executor::image`：
//! 与 `read_file` 的「定义在插件、算法在 core」同形（ADR-049）。

use async_trait::async_trait;
use tianyan::agent::{
    parse_params, safety_violation, DynamicToolExecutor, ToolContext, ViewImageParams,
};
use tianyan::common::error::{Result, TianyanError};
use tianyan::config::ModelCapability;
use tianyan::model::types::{FunctionDefinition, ToolDefinition, ToolPresentation};

/// `view_image` 工具：查看本地图片文件。
pub struct ViewImageTool {
    /// 工具上下文（装配层从 `ToolRegistry::context()` 取共享句柄）。
    ctx: ToolContext,
}

impl ViewImageTool {
    /// 以工具上下文构造。
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl DynamicToolExecutor for ViewImageTool {
    fn tool_name(&self) -> String {
        "view_image".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::from_schema::<ViewImageParams>(
            "view_image",
            "查看本地图片文件（png/jpeg/gif/bmp/webp，单张 ≤ 5MB）。图片内容会**随本工具结果一起发送给你**——你能直接看到像素，不需要其他工具转述；同一张图重复调用是幂等的（按内容 sha256 冻结副本，原文件后续被移动/修改不影响历史回放）。超大图会自动等比缩到 2048 像素以内，结果里的 original_dimensions 与 scale_hint 给出原图尺寸和坐标乘数（在原图上定位特征时按它换算）。只支持本地文件，不支持 URL；读文本仍用 read_file；要求当前模型声明视觉（vision）能力。返回 {image, sha256, mime, bytes, width, height, original_dimensions, scale_hint, path}。",
        ))
    }

    /// ADR-050：图片查看有自己的卡片形态（前端展示图片卡片）。
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::Image
    }

    async fn execute(&self, session_id: &str, arguments: &str) -> Result<serde_json::Value> {
        let params: ViewImageParams = parse_params(arguments)?;
        let path = self.ctx.resolve_tool_path(session_id, &params.path).await;
        safety_violation(
            self.ctx
                .security_policy
                .check_path(std::path::Path::new(&path)),
        )?;
        // 数据目录未注入 = 该能力不可用：明确报装配错误（不 panic、不静默成功）
        let data_dir = self.ctx.data_dir.as_deref().ok_or_else(|| {
            TianyanError::Custom(
                "view_image: 装配错误：未注入数据目录（data_dir），无法保存图片副本".to_string(),
            )
        })?;
        // 能力门控（ADR-050）：当前会话模型未声明视觉能力时**早失败**——比让上游
        // 用 400 拒绝整轮请求更明确，也不浪费一次往返。拿不到能力的三种情形
        // （未注入会话管理器 / 配置未初始化 / 未知模型）一律放行，由上游兜底。
        if let Some((model, capabilities)) = self.ctx.chat_model_capabilities(session_id).await {
            assert_vision_capable(&model, &capabilities)?;
        }
        // 冻结副本的错误已在 core 侧带 `view_image:` 前缀（ADR-050），不再二次包装
        let prepared = tianyan::executor::image::prepare_session_image(&path, data_dir).await?;
        Ok(serde_json::json!({
            "image": prepared.reference,
            "sha256": prepared.sha256,
            "mime": prepared.mime,
            "bytes": prepared.bytes,
            "width": prepared.width,
            "height": prepared.height,
            "original_dimensions": prepared
                .original_dimensions
                .map(|(w, h)| serde_json::json!({ "width": w, "height": h })),
            "scale_hint": prepared.scale_hint(),
            "path": path,
        }))
    }
}

/// 要求当前会话模型声明视觉能力（ADR-050 的 route gate）。
///
/// 与 DSH `assertImageCapableRoute` 的**刻意差异**：DSH 的
/// `inputModalities === undefined` 也拒绝（它的 undefined 表示"路由解析不到模型"，
/// 属异常）；天演的 `capabilities` 是**可选**的用户标签（`serde(default)` → 空数组，
/// 旧配置与新加模型都可能未标注），空数组只说明"没标"，不说明"不能"。
/// 所以这里只在**明确声明了能力、却不含 vision** 时拒绝——那种情况下上游 400
/// 几乎必然发生，提前拦住能省一次往返与一条误导性错误。
fn assert_vision_capable(model: &str, capabilities: &[ModelCapability]) -> Result<()> {
    if capabilities.is_empty() || capabilities.contains(&ModelCapability::Vision) {
        return Ok(());
    }
    Err(TianyanError::permission(format!(
        "view_image: 当前会话模型 {model} 未声明视觉（vision）能力，图片无法进入请求；\
         请切换到声明了视觉能力的模型（配置 → 模型 → 能力标签），或在模型上补勾 vision"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vision_gate_accepts_declared_vision() {
        assert!(
            assert_vision_capable("m", &[ModelCapability::Chat, ModelCapability::Vision]).is_ok()
        );
    }

    #[test]
    fn test_vision_gate_rejects_chat_only_model() {
        let err = assert_vision_capable("text-only", &[ModelCapability::Chat])
            .expect_err("未声明 vision 必须拒绝");
        let msg = err.to_string();
        assert!(msg.contains("view_image"), "错误带工具前缀：{msg}");
        assert!(msg.contains("text-only"), "错误点名模型：{msg}");
        assert!(msg.contains("vision"), "错误点明缺件：{msg}");
    }

    #[test]
    fn test_vision_gate_allows_undeclared_capabilities() {
        // 未声明能力 ≠ 不能看图：capabilities 是可选标签（旧配置 / 新加模型可能为空）。
        // 空数组放行，否则「没标注」会被误判成「不支持」。
        assert!(assert_vision_capable("unlabeled", &[]).is_ok());
    }
}
