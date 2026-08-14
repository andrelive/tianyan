use crate::common::error::Result;
use crate::common::llm_judge::parse_llm_json;

use super::types::{ExecutionHistory, GeneratedSkill, SkillParameter};

pub fn build_skill_generation_prompt(task_type: &str, executions: &[&ExecutionHistory]) -> String {
    let mut prompt = format!(
        r#"基于以下成功的执行历史，生成一个可复用的技能定义。

任务类型: {}

执行历史:
"#,
        task_type
    );

    for (i, exec) in executions.iter().enumerate() {
        prompt.push_str(&format!("\n--- 执行 {} ---\n", i + 1));
        prompt.push_str(&format!("任务: {}\n", exec.task_description));
        prompt.push_str(&format!("结果: {}\n", exec.result));
        prompt.push_str("步骤:\n");
        for step in &exec.steps {
            prompt.push_str(&format!(
                "  - {} ({}): {}\n",
                step.description, step.action, step.result
            ));
        }
    }

    prompt.push_str(
        r#"
请生成一个结构化的技能定义，格式如下：

```json
{
  "id": "技能唯一标识（小写英文和连字符）",
  "name": "技能名称",
  "description": "技能描述",
  "applicable_scenarios": ["适用场景1", "适用场景2"],
  "parameters": [
    {
      "name": "参数名",
      "type": "string|number|boolean|array|object",
      "description": "参数描述",
      "required": true|false,
      "default": "默认值（可选）"
    }
  ],
  "content": "技能的详细内容（Markdown 格式，包含使用说明、示例、注意事项）"
}
```

要求：
1. 技能应该通用化，不依赖于具体的项目路径或文件名
2. 参数应该覆盖执行中的可变部分
3. 内容应该包含清晰的使用步骤和示例
4. 适用场景应该描述清楚何时使用此技能
"#,
    );

    prompt
}

pub fn parse_generated_skill(
    response: &str,
    task_type: &str,
    executions: &[&ExecutionHistory],
) -> Result<GeneratedSkill> {
    let json: serde_json::Value = parse_llm_json(response).ok_or_else(|| {
        crate::common::error::TianyanError::Custom("内部错误：解析生成的技能失败".to_string())
    })?;

    let id = json
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("learned-{}", task_type));

    let name = json
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("未命名技能")
        .to_string();

    let description = json
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let content = json
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let applicable_scenarios = json
        .get("applicable_scenarios")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let parameters = json
        .get("parameters")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    Some(SkillParameter {
                        name: p.get("name")?.as_str()?.to_string(),
                        param_type: p.get("type")?.as_str()?.to_string(),
                        description: p.get("description")?.as_str()?.to_string(),
                        required: p.get("required")?.as_bool().unwrap_or(false),
                        default_value: p
                            .get("default")
                            .and_then(|v| v.as_str().map(|s| s.to_string())),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let source_history: Vec<String> = executions
        .iter()
        .map(|e| e.task_description.clone())
        .collect();

    Ok(GeneratedSkill {
        id,
        name,
        description,
        content,
        applicable_scenarios,
        parameters,
        version: "1.0.0".to_string(),
        source_history,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_generated_skill() {
        let response = r#"{
            "id": "read-config-file",
            "name": "读取配置文件",
            "description": "读取并解析项目配置文件",
            "applicable_scenarios": ["项目初始化", "配置检查"],
            "parameters": [
                {
                    "name": "path",
                    "type": "string",
                    "description": "配置文件路径",
                    "required": true
                }
            ],
            "content": "使用 file_read 技能读取配置文件..."
        }"#;

        let executions: Vec<&ExecutionHistory> = vec![];
        let skill = parse_generated_skill(response, "file_operation", &executions).unwrap();

        assert_eq!(skill.id, "read-config-file");
        assert_eq!(skill.name, "读取配置文件");
        assert_eq!(skill.parameters.len(), 1);
        assert_eq!(skill.version, "1.0.0");
    }
}
