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
            "查看本地图片文件（png/jpeg/gif/bmp/webp，单张 ≤ 5MB）。图片内容会**随本工具结果一起发送给你**——你能直接看到像素，不需要其他工具转述；同一张图重复调用是幂等的（按内容 sha256 冻结副本，原文件后续被移动/修改不影响历史回放）。只支持本地文件，不支持 URL；读文本仍用 read_file。返回 {image, sha256, mime, bytes, path}。",
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
        // 冻结副本的错误已在 core 侧带 `view_image:` 前缀（ADR-050），不再二次包装
        let prepared = tianyan::executor::image::prepare_session_image(&path, data_dir).await?;
        Ok(serde_json::json!({
            "image": prepared.reference,
            "sha256": prepared.sha256,
            "mime": prepared.mime,
            "bytes": prepared.bytes,
            "path": path,
        }))
    }
}
