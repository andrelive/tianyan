// 角色学习引擎测试（真实 VFS + mock LLM）。
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::agent::role_learning::{RoleLearningConfig, RoleLearningEngine};
    use crate::agent::role_store::RoleStore;
    use crate::agent::roles::{RoleSource, RoleStatus};
    use crate::common::types::{ContextNamespace, Message as ModelMessage, TianyanUri, TokenUsage};
    use crate::config::StorageConfig;
    use crate::model::types::{ChatChoice, ChatCompletionResponse};
    use crate::model::MockChatService;
    use crate::skills::learning::ExecutionHistory;
    use crate::vfs::backend::LocalFileBackend;
    use crate::vfs::{
        MockVectorStorage, VectorStorage, VfsCore, VirtualFileSystem, VirtualFileSystemImpl,
    };

    /// 真实文件后端 + tempdir 的 VFS。
    async fn real_vfs() -> (Arc<dyn VirtualFileSystem>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let config = StorageConfig {
            data_dir: dir.path().into(),
            ..Default::default()
        };
        let storage = Arc::new(LocalFileBackend::new(config.clone()));
        let vector_storage: Arc<dyn VectorStorage> = Arc::new(MockVectorStorage::new());
        let vfs = VirtualFileSystemImpl::new(storage, vector_storage, config);
        vfs.initialize().await.unwrap();
        (Arc::new(vfs), dir)
    }

    /// 构造 LLM 响应（assistant 文本）。
    fn response_with(text: &str) -> ChatCompletionResponse {
        ChatCompletionResponse {
            id: "resp-1".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ModelMessage::assistant(text.to_string()),
                finish_reason: Some("stop".to_string()),
            }],
            usage: TokenUsage::default(),
        }
    }

    /// 构造成功执行历史（工具粒度记录，同类型重复）。
    fn history(n: usize) -> Vec<ExecutionHistory> {
        (0..n)
            .map(|i| ExecutionHistory {
                task_description: format!("search_code: 查找符号 {}", i),
                steps: vec![],
                result: "找到 3 处引用".to_string(),
                success: true,
                execution_time_ms: 100,
                skills_used: vec![],
            })
            .collect()
    }

    #[tokio::test]
    async fn test_learn_from_history_generates_role_and_skill() {
        // mock LLM：generate 返回角色定义 JSON；验证门关闭（candidate_verification: false）
        let role_json = r###"
```json
{
  "role_name": "symbol-hunter",
  "purpose": "代码符号定位与引用分析",
  "system_prompt": "你是代码符号检索专家。负责定位符号定义与引用，输出结构化结果。只检索不修改。",
  "tools": ["search_code", "read_file"],
  "max_turns": 100,
  "orchestration_skill_id": "role-symbol-hunter-guide",
  "orchestration_skill_description": "符号定位任务委托指南",
  "orchestration_skill_content": "## 触发条件\n需要跨文件定位符号时。\n## 建议工作流\n委托 symbol-hunter 检索，主任务汇总。\n## 注意事项\n不要并行多个符号任务。"
}
```
"###;
        let mut mock = MockChatService::new();
        mock.expect_chat_completion()
            .times(1)
            .returning(move |_| Ok(response_with(role_json)));
        let (vfs, _dir) = real_vfs().await;
        let store = RoleStore::new(vfs.clone());
        let engine = RoleLearningEngine::new(
            Arc::new(mock),
            vfs.clone(),
            store.clone(),
            RoleLearningConfig {
                candidate_verification: false,
                generation_model: "test-model".to_string(),
                ..RoleLearningConfig::default()
            },
        );

        let learned = engine.learn_from_history(&history(3)).await.unwrap();
        assert_eq!(learned.len(), 1, "应生成 1 个角色");
        let outcome = &learned[0];
        assert_eq!(outcome.role.name, "symbol-hunter");
        assert_eq!(outcome.role.source, RoleSource::Learned);
        assert_eq!(outcome.role.version, 1);
        assert_eq!(outcome.role.status, RoleStatus::Active);
        assert_eq!(
            outcome.role.tools.as_deref().unwrap(),
            &["search_code".to_string(), "read_file".to_string()]
        );
        assert!(!outcome.updated_existing);
        assert_eq!(
            outcome.orchestration_skill_id.as_deref(),
            Some("role-symbol-hunter-guide")
        );

        // VFS 落盘验证：角色 + 编排技能
        let stored = store.load_role("symbol-hunter").await.unwrap();
        assert!(stored.is_some());
        let skill_uri = TianyanUri::new(
            ContextNamespace::Skill,
            vec!["role-symbol-hunter-guide".to_string()],
        );
        assert!(
            vfs.exists(&skill_uri).await.unwrap(),
            "编排技能应写入 VFS skill 命名空间"
        );
    }

    #[tokio::test]
    async fn test_learn_insufficient_history_returns_empty() {
        // 历史不足门槛：直接返回空，不触发 LLM
        let mock = MockChatService::new();
        let (vfs, _dir) = real_vfs().await;
        let engine = RoleLearningEngine::new(
            Arc::new(mock),
            vfs.clone(),
            RoleStore::new(vfs),
            RoleLearningConfig::default(),
        );
        let learned = engine.learn_from_history(&history(1)).await.unwrap();
        assert!(learned.is_empty());
    }
}
