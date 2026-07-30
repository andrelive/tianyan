//! 技能定义和注册表。
//!
//! 本模块提供技能定义框架，包括技能结构、参数模式和技能注册表。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::common::error::{Result, TianyanError};
use crate::common::types::Embedding;

use super::types::{
    ExecutionContext, SecurityLevel, SkillCategory, SkillExample, SkillExecutionResult,
};

/// 技能定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// 技能 ID。
    pub id: String,
    /// 技能名称。
    pub name: String,
    /// 技能描述。
    pub description: String,
    /// 技能分类。
    pub category: SkillCategory,
    /// 参数模式（JSON Schema）。
    pub parameters: Option<ParameterSchema>,
    /// 必需参数。
    #[serde(default)]
    pub required_parameters: Vec<String>,
    /// 使用示例。
    #[serde(default)]
    pub examples: Vec<SkillExample>,
    /// 安全级别。
    pub security_level: SecurityLevel,
    /// 是否启用技能。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 技能版本。
    #[serde(default)]
    pub version: String,
    /// 技能发现标签。
    #[serde(default)]
    pub tags: Vec<String>,
    /// 创建时间。
    pub created_at: DateTime<Utc>,
    /// 最后更新时间。
    pub updated_at: DateTime<Utc>,
    /// 用于语义搜索的描述嵌入向量。
    #[serde(skip)]
    pub description_embedding: Option<Embedding>,
}

fn default_true() -> bool {
    true
}

impl Skill {
    /// 创建新技能。
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            category: SkillCategory::default(),
            parameters: None,
            required_parameters: Vec::new(),
            examples: Vec::new(),
            security_level: SecurityLevel::default(),
            enabled: true,
            version: "1.0.0".to_string(),
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
            description_embedding: None,
        }
    }

    /// 设置分类。
    pub fn with_category(mut self, category: SkillCategory) -> Self {
        self.category = category;
        self
    }

    /// 设置参数模式。
    pub fn with_parameters(mut self, schema: ParameterSchema) -> Self {
        self.parameters = Some(schema);
        self
    }

    /// 设置必需参数。
    pub fn with_required_parameters(mut self, params: Vec<String>) -> Self {
        self.required_parameters = params;
        self
    }

    /// 添加示例。
    pub fn with_example(mut self, example: SkillExample) -> Self {
        self.examples.push(example);
        self
    }

    /// 设置安全级别。
    pub fn with_security_level(mut self, level: SecurityLevel) -> Self {
        self.security_level = level;
        self
    }

    /// 设置版本。
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// 添加标签。
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// 设置描述嵌入向量。
    pub fn with_embedding(mut self, embedding: Embedding) -> Self {
        self.description_embedding = Some(embedding);
        self
    }

    /// 获取用于嵌入向量生成的文本。
    pub fn embedding_text(&self) -> String {
        let mut text = format!("{}: {}", self.name, self.description);
        if !self.tags.is_empty() {
            text.push_str(&format!(" 标签: {}", self.tags.join(", ")));
        }
        text
    }
}

/// 参数模式定义（JSON Schema 格式）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterSchema {
    /// 模式类型。
    #[serde(rename = "type", default = "default_schema_type")]
    pub schema_type: String,
    /// 参数定义。
    pub properties: HashMap<String, ParameterDefinition>,
    /// 必需参数。
    #[serde(default)]
    pub required: Vec<String>,
    /// 是否允许额外属性。
    #[serde(default = "default_false")]
    pub additional_properties: bool,
}

fn default_schema_type() -> String {
    "object".to_string()
}

fn default_false() -> bool {
    false
}

impl ParameterSchema {
    /// 创建新的参数模式。
    pub fn new() -> Self {
        Self {
            schema_type: "object".to_string(),
            properties: HashMap::new(),
            required: Vec::new(),
            additional_properties: false,
        }
    }

    /// 添加参数定义。
    pub fn with_parameter(
        mut self,
        name: impl Into<String>,
        definition: ParameterDefinition,
    ) -> Self {
        self.properties.insert(name.into(), definition);
        self
    }

    /// 添加必需参数。
    pub fn with_required(mut self, name: impl Into<String>) -> Self {
        self.required.push(name.into());
        self
    }

    /// 根据此模式验证参数。
    pub fn validate(&self, params: &HashMap<String, Value>) -> Result<()> {
        // 检查必需参数
        for required in &self.required {
            if !params.contains_key(required) {
                return Err(TianyanError::Custom(format!(
                    "[unknown] 缺少必需参数: {}",
                    required
                )));
            }
        }

        // 验证每个参数
        for (name, value) in params {
            if let Some(def) = self.properties.get(name) {
                def.validate(name, value)?;
            }
        }

        Ok(())
    }
}

impl Default for ParameterSchema {
    fn default() -> Self {
        Self::new()
    }
}

/// 单个参数的定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterDefinition {
    /// 参数类型。
    #[serde(rename = "type")]
    pub param_type: ParameterType,
    /// 参数描述。
    pub description: Option<String>,
    /// 默认值。
    pub default: Option<Value>,
    /// 枚举值（如果适用）。
    #[serde(default)]
    pub enum_values: Vec<Value>,
    /// 最小值（用于数字）。
    pub minimum: Option<f64>,
    /// 最大值（用于数字）。
    pub maximum: Option<f64>,
    /// 最小长度（用于字符串）。
    pub min_length: Option<usize>,
    /// 最大长度（用于字符串）。
    pub max_length: Option<usize>,
    /// 模式（用于字符串）。
    pub pattern: Option<String>,
}

impl ParameterDefinition {
    /// 创建新的参数定义。
    pub fn new(param_type: ParameterType) -> Self {
        Self {
            param_type,
            description: None,
            default: None,
            enum_values: Vec::new(),
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
        }
    }

    /// 设置描述。
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// 设置默认值。
    pub fn with_default(mut self, default: Value) -> Self {
        self.default = Some(default);
        self
    }

    /// 设置枚举值。
    pub fn with_enum_values(mut self, values: Vec<Value>) -> Self {
        self.enum_values = values;
        self
    }

    /// 设置最小值（用于数字）。
    pub fn with_minimum(mut self, min: f64) -> Self {
        self.minimum = Some(min);
        self
    }

    /// 设置最大值（用于数字）。
    pub fn with_maximum(mut self, max: f64) -> Self {
        self.maximum = Some(max);
        self
    }

    /// 设置最小长度（用于字符串）。
    pub fn with_min_length(mut self, min: usize) -> Self {
        self.min_length = Some(min);
        self
    }

    /// 设置最大长度（用于字符串）。
    pub fn with_max_length(mut self, max: usize) -> Self {
        self.max_length = Some(max);
        self
    }

    /// 设置模式（用于字符串）。
    pub fn with_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.pattern = Some(pattern.into());
        self
    }

    /// 根据此定义验证值。
    pub fn validate(&self, name: &str, value: &Value) -> Result<()> {
        // 检查类。
        let type_valid = match self.param_type {
            ParameterType::String => value.is_string(),
            ParameterType::Number => value.is_number(),
            ParameterType::Integer => value.is_i64() || value.is_u64(),
            ParameterType::Boolean => value.is_boolean(),
            ParameterType::Array => value.is_array(),
            ParameterType::Object => value.is_object(),
            ParameterType::Null => value.is_null(),
        };

        if !type_valid {
            return Err(TianyanError::Custom(format!(
                "[unknown] 参数 '{}' 类型错误。期望 {:?}，实际 {:?}",
                name, self.param_type, value
            )));
        }

        // 检查枚举值
        if !self.enum_values.is_empty() && !self.enum_values.contains(value) {
            return Err(TianyanError::Custom(format!(
                "[unknown] 参数 '{}' 必须为 {:?} 之一，实际 {:?}",
                name, self.enum_values, value
            )));
        }

        // 检查字符串约束
        if let Value::String(s) = value {
            if let Some(min) = self.min_length {
                if s.len() < min {
                    return Err(TianyanError::Custom(format!(
                        "[unknown] 参数 '{}' 太短。最小长度为 {}",
                        name, min
                    )));
                }
            }
            if let Some(max) = self.max_length {
                if s.len() > max {
                    return Err(TianyanError::Custom(format!(
                        "[unknown] 参数 '{}' 太长。最大长度为 {}",
                        name, max
                    )));
                }
            }
        }

        // 检查数字约束
        if let Value::Number(n) = value {
            if let Some(min) = self.minimum {
                if let Some(f) = n.as_f64() {
                    if f < min {
                        return Err(TianyanError::Custom(format!(
                            "[unknown] 参数 '{}' 太小。最小值为 {}",
                            name, min
                        )));
                    }
                }
            }
            if let Some(max) = self.maximum {
                if let Some(f) = n.as_f64() {
                    if f > max {
                        return Err(TianyanError::Custom(format!(
                            "[unknown] 参数 '{}' 太大。最大值为 {}",
                            name, max
                        )));
                    }
                }
            }
        }

        Ok(())
    }
}

/// 参数类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParameterType {
    /// 字符串类型。
    String,
    /// 数字类型（浮点数）。
    Number,
    /// 整数类型。
    Integer,
    /// 布尔类型。
    Boolean,
    /// 数组类型。
    Array,
    /// 对象类型。
    Object,
    /// 空值类型。
    Null,
}

/// 技能处理器 trait，用于执行技能。
#[async_trait]
pub trait SkillHandler: Send + Sync {
    /// 使用给定参数执行技能。
    async fn execute(
        &self,
        params: HashMap<String, Value>,
        context: ExecutionContext,
    ) -> Result<SkillExecutionResult>;

    /// 获取此处理器对应的技能 ID。
    fn skill_id(&self) -> &str;
}

/// 技能注册表，用于管理和发现技能。
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
    handlers: HashMap<String, Arc<dyn SkillHandler>>,
}

impl SkillRegistry {
    /// 创建新的技能注册表。
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
            handlers: HashMap::new(),
        }
    }

    /// 注册技能。
    pub fn register(&mut self, skill: Skill) {
        self.skills.insert(skill.id.clone(), skill);
    }

    /// 注册技能及其处理器。
    pub fn register_with_handler(&mut self, skill: Skill, handler: Arc<dyn SkillHandler>) {
        let skill_id = skill.id.clone();
        self.skills.insert(skill_id.clone(), skill);
        self.handlers.insert(skill_id, handler);
    }

    /// 注销技能。
    pub fn unregister(&mut self, skill_id: &str) {
        self.skills.remove(skill_id);
        self.handlers.remove(skill_id);
    }

    /// 根据 ID 获取技能。
    pub fn get(&self, skill_id: &str) -> Option<&Skill> {
        self.skills.get(skill_id)
    }

    /// 获取技能处理器。
    pub fn get_handler(&self, skill_id: &str) -> Option<Arc<dyn SkillHandler>> {
        self.handlers.get(skill_id).cloned()
    }

    /// 列出所有技能。
    pub fn list(&self) -> Vec<&Skill> {
        self.skills.values().collect()
    }

    /// 按分类列出技能。
    pub fn list_by_category(&self, category: SkillCategory) -> Vec<&Skill> {
        self.skills
            .values()
            .filter(|s| s.category == category)
            .collect()
    }

    /// 按安全级别列出技能。
    pub fn list_by_security_level(&self, level: SecurityLevel) -> Vec<&Skill> {
        self.skills
            .values()
            .filter(|s| s.security_level == level)
            .collect()
    }

    /// 按名称查找技能（模糊匹配）。
    pub fn find_by_name(&self, name: &str) -> Vec<&Skill> {
        let name_lower = name.to_lowercase();
        self.skills
            .values()
            .filter(|s| {
                s.name.to_lowercase().contains(&name_lower)
                    || name_lower.contains(&s.name.to_lowercase())
            })
            .collect()
    }

    /// 按标签查找技能。
    pub fn find_by_tags(&self, tags: &[String]) -> Vec<&Skill> {
        self.skills
            .values()
            .filter(|s| {
                tags.iter().any(|t| {
                    s.tags
                        .iter()
                        .any(|st| st.to_lowercase() == t.to_lowercase())
                })
            })
            .collect()
    }

    /// 按语义相似度查找技能。
    pub fn find_by_similarity(
        &self,
        query_embedding: &Embedding,
        top_k: usize,
    ) -> Vec<(&Skill, f32)> {
        let mut results: Vec<(&Skill, f32)> = self
            .skills
            .values()
            .filter_map(|s| {
                s.description_embedding
                    .as_ref()
                    .map(|e| (s, e.cosine_similarity(query_embedding)))
            })
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        results
    }

    /// 获取已注册技能的数量。
    pub fn count(&self) -> usize {
        self.skills.len()
    }

    /// 检查技能是否已注册。
    pub fn contains(&self, skill_id: &str) -> bool {
        self.skills.contains_key(skill_id)
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_creation() {
        let skill = Skill::new("test-skill", "Test Skill", "A test skill")
            .with_category(SkillCategory::FileOperations)
            .with_security_level(SecurityLevel::Safe)
            .with_tag("test");

        assert_eq!(skill.id, "test-skill");
        assert_eq!(skill.name, "Test Skill");
        assert_eq!(skill.category, SkillCategory::FileOperations);
        assert_eq!(skill.security_level, SecurityLevel::Safe);
        assert!(skill.tags.contains(&"test".to_string()));
    }

    #[test]
    fn test_parameter_schema_validation() {
        let schema = ParameterSchema::new()
            .with_parameter(
                "path",
                ParameterDefinition::new(ParameterType::String)
                    .with_description("File path")
                    .with_min_length(1),
            )
            .with_parameter(
                "count",
                ParameterDefinition::new(ParameterType::Integer)
                    .with_minimum(0.0)
                    .with_maximum(100.0),
            )
            .with_required("path");

        // Valid parameters
        let mut params = HashMap::new();
        params.insert(
            "path".to_string(),
            Value::String("/test/file.txt".to_string()),
        );
        params.insert("count".to_string(), Value::Number(50.into()));
        assert!(schema.validate(&params).is_ok());

        // Missing required parameter
        let params = HashMap::new();
        assert!(schema.validate(&params).is_err());

        // Wrong type
        let mut params = HashMap::new();
        params.insert("path".to_string(), Value::Number(123.into()));
        assert!(schema.validate(&params).is_err());

        // Number out of range
        let mut params = HashMap::new();
        params.insert("path".to_string(), Value::String("/test".to_string()));
        params.insert("count".to_string(), Value::Number(200.into()));
        assert!(schema.validate(&params).is_err());
    }

    #[test]
    fn test_skill_registry() {
        let mut registry = SkillRegistry::new();

        let skill1 = Skill::new("skill-1", "Skill One", "First skill")
            .with_category(SkillCategory::FileOperations);
        let skill2 = Skill::new("skill-2", "Skill Two", "Second skill")
            .with_category(SkillCategory::SystemCommands);

        registry.register(skill1);
        registry.register(skill2);

        assert_eq!(registry.count(), 2);
        assert!(registry.contains("skill-1"));
        assert!(registry.get("skill-1").is_some());

        let file_skills = registry.list_by_category(SkillCategory::FileOperations);
        assert_eq!(file_skills.len(), 1);

        let found = registry.find_by_name("One");
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn test_embedding_text() {
        let skill = Skill::new("test", "Test", "A test skill")
            .with_tag("tag1")
            .with_tag("tag2");

        let text = skill.embedding_text();
        assert!(text.contains("Test"));
        assert!(text.contains("A test skill"));
        assert!(text.contains("tag1"));
    }
}
