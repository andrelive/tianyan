//! 配置向导类型定义
//!
//! 本模块提供配置向导相关的请求和响应类型。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// 从 tianyan::config::wizard 重新导出核心类型
pub use tianyan::config::wizard::{
    ConfigStatus, TestConnectionRequest, TestConnectionResponse, WizardAgentConfig, WizardConfig,
    WizardModelService, WizardModelsConfig, WizardStorageConfig, WizardVectorStorageConfig,
};

/// 配置保存请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveConfigRequest {
    /// 配置数据
    pub config: WizardConfig,
    /// 自定义保存路径（可选，默认使用系统配置目录）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_path: Option<PathBuf>,
}

/// 配置保存响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveConfigResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
    /// 保存的配置文件路径
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_config_response() {
        let success = SaveConfigResponse {
            success: true,
            message: "保存成功".to_string(),
            config_path: Some(PathBuf::from("/test/config.toml")),
        };
        assert!(success.success);

        let error = SaveConfigResponse {
            success: false,
            message: "保存失败".to_string(),
            config_path: None,
        };
        assert!(!error.success);
    }
}
