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
    ]
}

pub fn register_builtin_skills(registry: &mut SkillRegistry, config: &ExecutorConfig) {
    for skill in create_builtin_skills() {
        registry.register(skill);
    }

    registry.register_with_handler(
        Skill::new("file_read", "Read File", "Read the contents of a file"),
        Arc::new(FileReadHandler::new(config.allowed_paths.clone())),
    );

    registry.register_with_handler(
        Skill::new("file_write", "Write File", "Write content to a file"),
        Arc::new(FileWriteHandler::new(config.allowed_paths.clone())),
    );

    registry.register_with_handler(
        Skill::new("file_delete", "Delete File", "Delete a file or directory"),
        Arc::new(FileDeleteHandler::new(config.allowed_paths.clone())),
    );

    registry.register_with_handler(
        Skill::new(
            "file_list",
            "List Directory",
            "List contents of a directory",
        ),
        Arc::new(FileListHandler::new(config.allowed_paths.clone())),
    );

    registry.register_with_handler(
        Skill::new(
            "system_command",
            "Execute Command",
            "Execute a system shell command",
        ),
        Arc::new(SystemCommandHandler::new(config.blocked_commands.clone())),
    );

    registry.register_with_handler(
        Skill::new("http_request", "HTTP Request", "Make an HTTP request"),
        Arc::new(HttpRequestHandler::new()),
    );
}
