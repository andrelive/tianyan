# Tianyan 配置指南

本文档详细介绍 Tianyan 的配置方式，包括配置文件、环境变量以及各配置项的说明。

## 目录

- [配置文件概述](#配置文件概述)
- [配置文件位置和加载顺序](#配置文件位置和加载顺序)
- [配置项详细说明](#配置项详细说明)
- [环境变量说明](#环境变量说明)
- [配置最佳实践](#配置最佳实践)

---

## 配置文件概述

Tianyan 支持两种配置方式：

1. **TOML 配置文件**：主配置文件，支持完整的配置选项
2. **环境变量**：用于敏感信息和简单的运行时配置

### 配置文件格式

Tianyan 使用 TOML 格式的配置文件，具有以下特点：

- 结构清晰，易于阅读和编辑
- 支持嵌套配置节
- 支持环境变量引用
- 支持注释

---

## 配置文件位置和加载顺序

### 配置文件位置

Tianyan 按以下顺序查找配置文件：

1. **环境变量指定路径**：`TIANYAN_CONFIG_PATH` 环境变量指定的路径
2. **用户配置目录**：
   - Linux/macOS: `~/.tianyan/config.toml`
   - Windows: `%USERPROFILE%\.tianyan\config.toml`
3. **当前工作目录**：`./config.toml`
4. **项目根目录**：`./config.example.toml`（仅作为示例参考）

### 加载顺序

配置加载遵循以下优先级（从高到低）：

1. 环境变量
2. 用户配置目录的 `config.toml`
3. 当前工作目录的 `config.toml`
4. 默认值

### 快速开始

```bash
# 复制示例配置文件
cp config.example.toml ~/.tianyan/config.toml

# 编辑配置文件
vim ~/.tianyan/config.toml

# 或使用环境变量指定配置文件路径
export TIANYAN_CONFIG_PATH=/path/to/your/config.toml
```

---

## 配置项详细说明

### 存储配置 [storage]

存储配置控制数据持久化和向量存储相关设置。

```toml
[storage]
data_dir = "~/.tianyan"           # 数据存储根目录
max_storage_size = 0              # 最大存储大小（字节，0 = 无限制）
auto_cleanup = true               # 启用旧数据自动清理
cleanup_days = 365                # 清理前数据保留天数
```

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `data_dir` | string | `~/.tianyan` | 数据存储根目录，支持 `~` 表示用户主目录 |
| `max_storage_size` | int | `0` | 最大存储大小（字节），0 表示无限制 |
| `auto_cleanup` | bool | `true` | 是否启用旧数据自动清理 |
| `cleanup_days` | int | `365` | 清理前数据保留天数 |

#### 向量存储配置 [storage.vector]

系统**仅支持 Qdrant** 作为向量存储解决方案，确保向量数据持久化。

```toml
[storage.vector]
url = "http://localhost:6334"     # Qdrant 服务地址
collection_name = "tianyan_contexts"  # Collection 名称
vector_dimension = 1536           # 向量维度
```

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `url` | string | `http://localhost:6334` | Qdrant 服务地址 |
| `collection_name` | string | `tianyan_contexts` | 存储向量的 Collection 名称 |
| `vector_dimension` | int | `1536` | 向量维度，需与嵌入模型匹配 |

**向量维度参考：**

| 嵌入模型 | 向量维度 |
|----------|----------|
| text-embedding-3-small | 1536 |
| text-embedding-3-large | 3072 |
| text-embedding-ada-002 | 1536 |

---

### 模型服务配置 [model]

模型服务配置支持多个模型提供商，可以配置多个命名配置节。

#### 默认模型配置 [model.default]

```toml
[model.default]
provider = "openai"               # 服务提供商
api_key = "${TIANYAN_OPENAI_API_KEY}"  # API 密钥
base_url = "https://api.openai.com/v1" # API 基础地址
chat_model = "gpt-4"              # 聊天模型
embedding_model = "text-embedding-3-small"  # 嵌入模型
max_retries = 3                   # 最大重试次数
timeout = 60                      # 请求超时（秒）
```

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `provider` | string | `openai` | 服务提供商，见下表 |
| `api_key` | string | - | API 密钥，支持环境变量引用 `${VAR_NAME}` |
| `base_url` | string | - | API 基础地址，用于自定义端点 |
| `chat_model` | string | `gpt-4` | 聊天模型名称 |
| `embedding_model` | string | `text-embedding-3-small` | 嵌入模型名称 |
| `max_retries` | int | `3` | 请求失败时的最大重试次数 |
| `timeout` | int | `60` | 请求超时时间（秒） |

**支持的提供商：**

| 提供商 | 说明 |
|--------|------|
| `openai` | OpenAI 官方 API |
| `openai-compatible` | 兼容 OpenAI API 格式的服务（包括 DeepSeek、Claude 等） |

#### 多模型配置示例

```toml
# OpenAI 配置
[model.openai]
provider = "openai"
api_key = "${TIANYAN_OPENAI_API_KEY}"
chat_model = "gpt-4"

# DeepSeek 配置（使用 OpenAI 兼容接口）
[model.deepseek]
provider = "openai-compatible"
api_key = "${TIANYAN_DEEPSEEK_API_KEY}"
base_url = "https://api.deepseek.com/v1"
chat_model = "deepseek-chat"

# Claude 配置（使用 OpenAI 兼容接口）
[model.claude]
provider = "openai-compatible"
api_key = "${TIANYAN_ANTHROPIC_API_KEY}"
base_url = "https://api.anthropic.com/v1"
chat_model = "claude-3-opus-20240229"
```

---

### 日志配置 [logging]

```toml
[logging]
level = "info"                    # 日志级别
format = "text"                   # 日志格式
output_to_file = false            # 是否输出到文件
log_file = "~/.tianyan/logs/tianyan.log"  # 日志文件路径
```

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `level` | string | `info` | 日志级别：`trace`, `debug`, `info`, `warn`, `error` |
| `format` | string | `text` | 日志格式：`text`, `json` |
| `output_to_file` | bool | `false` | 是否将日志输出到文件 |
| `log_file` | string | - | 日志文件路径 |

**日志级别说明：**

| 级别 | 说明 |
|------|------|
| `trace` | 最详细的日志，用于调试 |
| `debug` | 调试信息 |
| `info` | 常规信息（推荐） |
| `warn` | 警告信息 |
| `error` | 错误信息 |

---

### 记忆系统配置 [memory]

```toml
[memory]
enabled = true                    # 是否启用记忆系统
session_timeout = 3600            # 会话超时时间（秒）
decay_half_life = 30              # 记忆衰减半衰期（天）
min_importance = 0.1              # 最小重要性阈值
```

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `enabled` | bool | `true` | 是否启用记忆系统 |
| `session_timeout` | int | `3600` | 会话超时时间（秒），超时后归档 |
| `decay_half_life` | int | `30` | 记忆衰减半衰期（天） |
| `min_importance` | float | `0.1` | 最小重要性阈值，低于此值的记忆将被清理 |

---

### 检索配置 [retrieval]

```toml
[retrieval]
max_results = 10                  # 检索结果数量上限
min_score = 0.5                   # 最小相关性分数
token_budget = 4000               # Token 预算
```

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `max_results` | int | `10` | 检索返回的最大结果数量 |
| `min_score` | float | `0.5` | 最小相关性分数，范围 0.0-1.0 |
| `token_budget` | int | `4000` | 检索内容的总 Token 数限制 |

---

### 安全配置 [security]

```toml
[security]
skill_security_level = "moderate" # 技能执行安全级别
forbidden_commands = ["rm -rf", "format", "del /s"]  # 禁止执行的命令
allowed_directories = []          # 允许访问的目录白名单
```

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `skill_security_level` | string | `moderate` | 技能执行安全级别 |
| `forbidden_commands` | array | `[]` | 禁止执行的命令列表 |
| `allowed_directories` | array | `[]` | 允许访问的目录白名单，为空表示允许所有 |

**安全级别说明：**

| 级别 | 说明 |
|------|------|
| `safe` | 仅允许安全技能，禁止所有文件系统操作 |
| `moderate` | 允许中等风险技能，有基本的文件系统保护 |
| `dangerous` | 允许所有技能，谨慎使用 |

---

## 环境变量说明

环境变量用于存储敏感信息或进行简单的运行时配置。所有 Tianyan 环境变量都以 `TIANYAN_` 为前缀。

### API 密钥

| 环境变量 | 说明 | 获取地址 |
|----------|------|----------|
| `TIANYAN_OPENAI_API_KEY` | OpenAI API 密钥 | https://platform.openai.com/api-keys |
| `TIANYAN_ANTHROPIC_API_KEY` | Anthropic API 密钥 | https://console.anthropic.com/ |
| `TIANYAN_DEEPSEEK_API_KEY` | DeepSeek API 密钥 | https://platform.deepseek.com/ |

### 存储配置

| 环境变量 | 说明 | 默认值 |
|----------|------|--------|
| `TIANYAN_DATA_DIR` | 数据存储目录 | `~/.tianyan` |
| `TIANYAN_QDRANT_URL` | Qdrant 服务地址 | `http://localhost:6333` |
| `TIANYAN_QDRANT_API_KEY` | Qdrant API 密钥 | - |

### 日志配置

| 环境变量 | 说明 | 默认值 |
|----------|------|--------|
| `TIANYAN_LOG_LEVEL` | 日志级别 | `info` |
| `TIANYAN_LOG_FORMAT` | 日志格式 | `text` |

### 模型配置

| 环境变量 | 说明 | 默认值 |
|----------|------|--------|
| `TIANYAN_DEFAULT_CHAT_MODEL` | 默认聊天模型 | `gpt-4` |
| `TIANYAN_DEFAULT_EMBEDDING_MODEL` | 默认嵌入模型 | `text-embedding-3-small` |

### 安全配置

| 环境变量 | 说明 | 默认值 |
|----------|------|--------|
| `TIANYAN_SKILL_SECURITY_LEVEL` | 技能安全级别 | `moderate` |

### 使用方式

**Linux/macOS:**
```bash
export TIANYAN_OPENAI_API_KEY=sk-xxxxxxxx
export TIANYAN_LOG_LEVEL=debug
```

**Windows (PowerShell):**
```powershell
$env:TIANYAN_OPENAI_API_KEY = "sk-xxxxxxxx"
$env:TIANYAN_LOG_LEVEL = "debug"
```

**使用 .env 文件:**
```bash
# 复制示例文件
cp .env.example .env

# 编辑 .env 文件
vim .env
```

---

## 配置最佳实践

### 1. 敏感信息管理

- **不要将 API 密钥提交到版本控制系统**
- 使用环境变量或 `.env` 文件存储敏感信息
- 将 `.env` 添加到 `.gitignore`

```gitignore
# .gitignore
.env
config.toml
```

### 2. 环境隔离

为不同环境创建不同的配置文件：

```
~/.tianyan/
├── config.toml          # 默认配置
├── config.dev.toml      # 开发环境
├── config.prod.toml     # 生产环境
└── config.test.toml     # 测试环境
```

使用环境变量切换配置：
```bash
export TIANYAN_CONFIG_PATH=~/.tianyan/config.prod.toml
```

### 3. 配置验证

启动时验证配置是否正确：
```bash
tianyan config validate
```

### 4. 向量存储选择

| 场景 | 推荐配置 |
|------|----------|
| 开发测试 | `vector_store_type = "memory"` |
| 生产环境 | `vector_store_type = "qdrant"` |
| 小规模数据 | 内存存储即可 |
| 大规模数据 | 使用 Qdrant 并配置持久化 |

### 5. 日志配置建议

| 环境 | 推荐配置 |
|------|----------|
| 开发 | `level = "debug"`, `format = "text"` |
| 生产 | `level = "info"`, `format = "json"` |
| 调试问题 | `level = "trace"` |

### 6. 安全配置建议

- 生产环境使用 `safe` 或 `moderate` 安全级别
- 配置 `allowed_directories` 限制文件访问范围
- 定期审查 `forbidden_commands` 列表

### 7. 性能优化

```toml
[retrieval]
# 根据实际需求调整
max_results = 5          # 减少结果数量可提高响应速度
token_budget = 2000      # 限制 Token 数量可减少 API 调用

[model.default]
timeout = 30             # 适当降低超时可快速失败
max_retries = 2          # 减少重试次数可降低延迟
```

---

## 常见问题

### Q: 配置文件修改后需要重启吗？

A: 是的，目前配置文件在启动时加载，修改后需要重启 Tianyan 才能生效。

### Q: 如何查看当前使用的配置？

A: 使用以下命令查看当前配置：
```bash
tianyan config show
```

### Q: 环境变量和配置文件哪个优先级更高？

A: 环境变量优先级更高。如果同时设置了环境变量和配置文件，环境变量的值会覆盖配置文件中的值。

### Q: 如何调试配置问题？

A: 将日志级别设置为 `debug` 或 `trace`：
```bash
export TIANYAN_LOG_LEVEL=debug
tianyan start
```

---

## 附录：完整配置示例

参见项目根目录的 `config.example.toml` 和 `.env.example` 文件。
