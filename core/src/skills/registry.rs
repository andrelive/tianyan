use std::sync::Arc;

use serde_json::Value;

use super::definition::{
    ParameterDefinition, ParameterSchema, ParameterType, Skill, SkillRegistry,
};
use super::executor::ExecutorConfig;
use super::handlers::{
    FileDeleteHandler, FileListHandler, FileReadHandler, FileWriteHandler, HttpRequestHandler,
    SystemCommandHandler,
};
use super::types::{SecurityLevel, SkillCategory};

/// 创建内置技能列表。
pub fn create_builtin_skills() -> Vec<Skill> {
    vec![
        Skill::new("file_read", "Read File", "Read the contents of a file")
            .with_category(SkillCategory::FileOperations)
            .with_security_level(SecurityLevel::Safe)
            .with_parameters(
                ParameterSchema::new()
                    .with_parameter(
                        "path",
                        ParameterDefinition::new(ParameterType::String)
                            .with_description("Path to the file to read"),
                    )
                    .with_required("path"),
            )
            .with_tag("file")
            .with_tag("read"),
        Skill::new("file_write", "Write File", "Write content to a file")
            .with_category(SkillCategory::FileOperations)
            .with_security_level(SecurityLevel::Moderate)
            .with_parameters(
                ParameterSchema::new()
                    .with_parameter(
                        "path",
                        ParameterDefinition::new(ParameterType::String)
                            .with_description("Path to the file to write"),
                    )
                    .with_parameter(
                        "content",
                        ParameterDefinition::new(ParameterType::String)
                            .with_description("Content to write to the file"),
                    )
                    .with_required("path")
                    .with_required("content"),
            )
            .with_tag("file")
            .with_tag("write"),
        Skill::new("file_delete", "Delete File", "Delete a file or directory")
            .with_category(SkillCategory::FileOperations)
            .with_security_level(SecurityLevel::Dangerous)
            .with_parameters(
                ParameterSchema::new()
                    .with_parameter(
                        "path",
                        ParameterDefinition::new(ParameterType::String)
                            .with_description("Path to the file or directory to delete"),
                    )
                    .with_required("path"),
            )
            .with_tag("file")
            .with_tag("delete"),
        Skill::new(
            "file_list",
            "List Directory",
            "List contents of a directory",
        )
        .with_category(SkillCategory::FileOperations)
        .with_security_level(SecurityLevel::Safe)
        .with_parameters(
            ParameterSchema::new().with_parameter(
                "path",
                ParameterDefinition::new(ParameterType::String)
                    .with_description("Path to the directory to list (default: current directory)"),
            ),
        )
        .with_tag("file")
        .with_tag("list"),
        Skill::new(
            "system_command",
            "Execute Command",
            "Execute a system shell command",
        )
        .with_category(SkillCategory::SystemCommands)
        .with_security_level(SecurityLevel::Dangerous)
        .with_parameters(
            ParameterSchema::new()
                .with_parameter(
                    "command",
                    ParameterDefinition::new(ParameterType::String)
                        .with_description("The shell command to execute"),
                )
                .with_required("command"),
        )
        .with_tag("system")
        .with_tag("command")
        .with_tag("shell"),
        Skill::new("http_request", "HTTP Request", "Make an HTTP request")
            .with_category(SkillCategory::NetworkRequests)
            .with_security_level(SecurityLevel::Moderate)
            .with_parameters(
                ParameterSchema::new()
                    .with_parameter(
                        "url",
                        ParameterDefinition::new(ParameterType::String)
                            .with_description("The URL to request"),
                    )
                    .with_parameter(
                        "method",
                        ParameterDefinition::new(ParameterType::String)
                            .with_description("HTTP method (GET, POST, PUT, DELETE, PATCH)")
                            .with_default(Value::String("GET".to_string())),
                    )
                    .with_parameter(
                        "headers",
                        ParameterDefinition::new(ParameterType::Object)
                            .with_description("HTTP headers as key-value pairs"),
                    )
                    .with_parameter(
                        "body",
                        ParameterDefinition::new(ParameterType::String)
                            .with_description("Request body (for POST, PUT, PATCH)"),
                    )
                    .with_required("url"),
            )
            .with_tag("http")
            .with_tag("network")
            .with_tag("request"),
        // 计划阶段技能（软约束：无副作用，返回行为指南注入对话；
        // 用户"计划一下"触发，替代显式 Plan 模式切换——见 REJECTED.md）
        Skill::new(
            "planning",
            "Planning",
            "进入计划阶段：只读研究并输出结构化计划，等待用户确认后执行",
        )
        .with_category(SkillCategory::Custom)
        .with_security_level(SecurityLevel::Safe)
        .with_parameters(
            ParameterSchema::new().with_parameter(
                "goal",
                ParameterDefinition::new(ParameterType::String)
                    .with_description("本次规划的目标（可选）"),
            ),
        )
        .with_tag("planning")
        .with_tag("plan"),
    ]
}

/// 注册内置技能到注册表。
///
/// 单次遍历完成注册：每个技能携带完整元数据（参数 schema、分类、安全级别）
/// 与对应处理器一起注册，避免先注册元数据、再用裸 `Skill::new` 覆盖
/// 导致参数 schema 丢失的问题。
pub fn register_builtin_skills(
    registry: &mut SkillRegistry,
    config: &ExecutorConfig,
) -> crate::common::error::Result<()> {
    let handlers: Vec<(&str, Arc<dyn super::definition::SkillHandler>)> = vec![
        (
            "file_read",
            Arc::new(
                FileReadHandler::new(config.allowed_paths.clone())
                    .with_max_size(config.skill_file_read_max_size)
                    .with_timeout(config.skill_file_read_timeout_secs),
            ),
        ),
        (
            "file_write",
            Arc::new(
                FileWriteHandler::new(config.allowed_paths.clone())
                    .with_max_size(config.skill_file_write_max_size)
                    .with_timeout(config.skill_file_write_timeout_secs),
            ),
        ),
        (
            "file_delete",
            Arc::new(FileDeleteHandler::new(config.allowed_paths.clone())),
        ),
        (
            "file_list",
            Arc::new(
                FileListHandler::new(config.allowed_paths.clone())
                    .with_max_entries(config.skill_file_list_max_entries),
            ),
        ),
        (
            "system_command",
            Arc::new(
                SystemCommandHandler::new(config.blocked_commands.clone())
                    .with_timeout(config.skill_command_timeout_secs),
            ),
        ),
        (
            "http_request",
            Arc::new(HttpRequestHandler::with_timeout(
                config.skill_http_timeout_secs,
            )?),
        ),
        (
            "planning",
            Arc::new(crate::skills::handlers::PlanningHandler::new()),
        ),
    ];

    for skill in create_builtin_skills() {
        match handlers.iter().find(|(id, _)| *id == skill.id) {
            Some((_, handler)) => registry.register_with_handler(skill, handler.clone()),
            None => registry.register(skill),
        }
    }

    Ok(())
}
