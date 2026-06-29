//! OpenAI request parsing — downgrade OpenAI ChatCompletion request to ds_core::ChatRequest
//!
//! 当前限制：
//! - - Multi-turn conversations compressed into a single prompt string via DeepSeek native tag format
//! - Tool definitions embedded after the last `<｜Assistant｜>`  in an unclosed `<think>`  block

pub(crate) mod files;
pub(crate) mod normalize;
pub(crate) mod prompt;
pub(crate) mod resolver;
pub(crate) mod tools;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai_adapter::OpenAIAdapterError;
    use crate::openai_adapter::types::{
        ChatCompletionsRequest, FunctionCallOption, NamedFunction, NamedToolChoice, Tool,
        ToolChoice,
    };

    fn default_registry() -> std::collections::HashMap<String, String> {
        crate::config::DsCoreSection::default().model_registry()
    }

    /// 测试用的 prepare，模拟 adapter 内部的解析逻辑
    #[derive(Debug)]
    struct TestRequest {
        prompt: String,
        thinking_enabled: bool,
        search_enabled: bool,
        stream: bool,
        include_usage: bool,
        include_obfuscation: bool,
        stop: Vec<String>,
    }

    fn parse_json(val: serde_json::Value) -> Result<TestRequest, OpenAIAdapterError> {
        let mut req: ChatCompletionsRequest = serde_json::from_value(val)
            .map_err(|e| OpenAIAdapterError::BadRequest(format!("bad request: {}", e)))?;
        let registry = default_registry();

        if req.tools.as_ref().map(|t| t.is_empty()).unwrap_or(true)
            && let Some(functions) = req.functions.clone()
            && !functions.is_empty()
        {
            req.tools = Some(
                functions
                    .into_iter()
                    .map(|f| Tool {
                        ty: "function".to_string(),
                        function: Some(f),
                        custom: None,
                    })
                    .collect(),
            );
        }
        if req.tool_choice.is_none()
            && let Some(fc) = req.function_call.clone()
        {
            req.tool_choice = Some(match fc {
                FunctionCallOption::Mode(mode) => ToolChoice::Mode(mode),
                FunctionCallOption::Named(named) => ToolChoice::Named(NamedToolChoice {
                    ty: "function".to_string(),
                    function: NamedFunction { name: named.name },
                }),
            });
        }

        let norm = normalize::apply(&req).map_err(OpenAIAdapterError::BadRequest)?;
        let tool_ctx = tools::extract(&req).map_err(OpenAIAdapterError::BadRequest)?;
        let prompt = prompt::build(&req, &tool_ctx);
        let model_res = resolver::resolve(
            &registry,
            &req.model,
            req.reasoning_effort.as_deref(),
            req.web_search_options.as_ref(),
        )
        .map_err(OpenAIAdapterError::BadRequest)?;

        println!("\n=== PARSED REQUEST ===");
        println!("prompt:\n{}", prompt);
        println!(
            "thinking={} search={}",
            model_res.thinking_enabled, model_res.search_enabled
        );
        println!("======================\n");

        Ok(TestRequest {
            prompt,
            thinking_enabled: model_res.thinking_enabled,
            search_enabled: model_res.search_enabled,
            stream: req.stream,
            include_usage: norm.include_usage,
            include_obfuscation: norm.include_obfuscation,
            stop: norm.stop,
        })
    }

    #[test]
    fn basic_chat() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [
                { "role": "system", "content": "You are a helpful assistant." },
                { "role": "user", "content": "Hello" }
            ]
        });
        let req = parse_json(body).unwrap();
        assert!(!req.prompt.is_empty());
    }

    #[test]
    fn tool_conversation() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [
                { "role": "user", "content": "Beijing天气怎么样？" },
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call_abc123",
                            "type": "function",
                            "function": { "name": "get_weather", "arguments": "{\"city\":\"Beijing\"}" }
                        }
                    ]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_abc123",
                    "content": "Beijing今天晴，25°C。"
                },
                { "role": "user", "content": "Thanks" }
            ]
        });
        let req = parse_json(body).unwrap();
        assert!(req.prompt.contains("get_weather"));
    }

    #[test]
    fn reasoning_and_search_flags() {
        let body = serde_json::json!({
            "model": "deepseek-expert",
            "messages": [
                { "role": "user", "content": "Analyze quantum computing" }
            ],
            "reasoning_effort": "high",
            "web_search_options": { "search_context_size": "high" }
        });
        let req = parse_json(body).unwrap();
        assert!(req.thinking_enabled);
        assert!(req.search_enabled);
    }

    // normalize error scenarios
    #[test]
    fn missing_model() {
        let body = serde_json::json!({
            "messages": [{ "role": "user", "content": "Hello" }]
        });
        let err = parse_json(body).unwrap_err();
        assert!(matches!(err, OpenAIAdapterError::BadRequest(_)));
        assert!(err.to_string().contains("model"));
    }

    #[test]
    fn missing_messages() {
        let body = serde_json::json!({
            "model": "deepseek-default"
        });
        let err = parse_json(body).unwrap_err();
        assert!(matches!(err, OpenAIAdapterError::BadRequest(_)));
        assert!(err.to_string().contains("messages"));
    }

    #[test]
    fn tool_missing_tool_call_id() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [
                { "role": "user", "content": "hi" },
                { "role": "tool", "content": "result" }
            ]
        });
        let err = parse_json(body).unwrap_err();
        assert!(matches!(err, OpenAIAdapterError::BadRequest(_)));
        assert!(err.to_string().contains("tool_call_id"));
    }

    #[test]
    fn function_missing_name() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [
                { "role": "user", "content": "hi" },
                { "role": "function", "content": "result" }
            ]
        });
        let err = parse_json(body).unwrap_err();
        assert!(matches!(err, OpenAIAdapterError::BadRequest(_)));
        assert!(err.to_string().contains("name"));
    }

    // model resolution errors and capability flags
    #[test]
    fn unsupported_model() {
        let body = serde_json::json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "hello" }]
        });
        let err = parse_json(body).unwrap_err();
        assert!(matches!(err, OpenAIAdapterError::BadRequest(_)));
        assert!(err.to_string().contains("unsupported"));
    }

    #[test]
    fn reasoning_effort_variants() {
        for (effort, expected) in [
            ("minimal", true),
            ("low", true),
            ("medium", true),
            ("high", true),
            ("xhigh", true),
            ("unknown", true),
            ("", true),
        ] {
            let body = serde_json::json!({
                "model": "deepseek-default",
                "messages": [{ "role": "user", "content": "hi" }],
                "reasoning_effort": effort
            });
            let req = parse_json(body).unwrap();
            assert_eq!(
                req.thinking_enabled, expected,
                "reasoning_effort={}",
                effort
            );
        }

        // reasoning defaults to enabled when reasoning_effort not provided
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }]
        });
        let req = parse_json(body).unwrap();
        assert!(
            req.thinking_enabled,
            "reasoning_effort absent should default to high"
        );
    }

    #[test]
    fn search_enabled_by_default() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }]
        });
        let req = parse_json(body).unwrap();
        assert!(req.search_enabled);
    }

    // stop sequence and stream_options defaults

    #[test]
    fn stop_single() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "stop": "EOF"
        });
        let req = parse_json(body).unwrap();
        assert_eq!(req.stop, vec!["EOF"]);
    }

    #[test]
    fn stop_multiple() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "stop": ["STOP", "HALT"]
        });
        let req = parse_json(body).unwrap();
        assert_eq!(req.stop, vec!["STOP", "HALT"]);
    }

    #[test]
    fn stream_options() {
        // 默认值
        let req = parse_json(serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();
        assert_eq!(req.stream, false);
        assert_eq!(req.include_usage, false);
        assert_eq!(req.include_obfuscation, true);

        // 显式覆盖
        let req2 = parse_json(serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "stream_options": { "include_usage": true, "include_obfuscation": false }
        }))
        .unwrap();
        assert_eq!(req2.include_usage, true);
        assert_eq!(req2.include_obfuscation, false);
    }

    // tools validation and injection

    #[test]
    fn tool_choice_none_ignores_tools() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [
                {
                    "type": "function",
                    "function": { "name": "f", "parameters": {} }
                }
            ],
            "tool_choice": "none"
        });
        let req = parse_json(body).unwrap();
        assert!(!req.prompt.contains("You can use the following tools"));
    }

    #[test]
    fn tool_choice_required_instruction() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [
                {
                    "type": "function",
                    "function": { "name": "f" }
                }
            ],
            "tool_choice": "required"
        });
        let req = parse_json(body).unwrap();
        assert!(req.prompt.contains("注意：你必须调用一个或多个工具"));
    }

    #[test]
    fn parallel_tool_calls_false_instruction() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [
                { "type": "function", "function": { "name": "f" } }
            ],
            "parallel_tool_calls": false
        });
        let req = parse_json(body).unwrap();
        assert!(req.prompt.contains("注意：一次只能call one tool"));
    }

    #[test]
    fn tool_choice_named_function() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [
                { "type": "function", "function": { "name": "get_weather" } }
            ],
            "tool_choice": { "type": "function", "function": { "name": "get_weather" } }
        });
        let req = parse_json(body).unwrap();
        assert!(req.prompt.contains("注意：你必须调用 'get_weather' 工具"));
    }

    #[test]
    fn tool_choice_allowed_tools() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [
                { "type": "function", "function": { "name": "get_weather" } },
                { "type": "function", "function": { "name": "get_time" } }
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "allowed_tools": {
                    "mode": "required",
                    "tools": [
                        { "type": "function", "function": { "name": "get_weather" } }
                    ]
                }
            }
        });
        let req = parse_json(body).unwrap();
        assert!(
            req.prompt
                .contains("You can only choose from the following allowed tools：get_weather")
        );
        assert!(req.prompt.contains("注意：你必须调用一个或多个工具"));
    }

    #[test]
    fn tool_choice_custom() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [
                {
                    "type": "custom",
                    "custom": { "name": "my_custom", "format": { "type": "text" } }
                }
            ],
            "tool_choice": { "type": "custom", "custom": { "name": "my_custom" } }
        });
        let req = parse_json(body).unwrap();
        assert!(req.prompt.contains("**my_custom** (custom):"));
        assert!(req.prompt.contains("You must call the 'my_custom' custom tool"));
    }

    #[test]
    fn custom_tool_grammar_format() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [
                {
                    "type": "custom",
                    "custom": {
                        "name": "grammar_tool",
                        "description": " grammar based tool",
                        "format": {
                            "type": "grammar",
                            "grammar": {
                                "definition": "start: word+",
                                "syntax": "lark"
                            }
                        }
                    }
                }
            ]
        });
        let req = parse_json(body).unwrap();
        assert!(req.prompt.contains("grammar(syntax: lark)"));
    }

    #[test]
    fn custom_tool_missing_format() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [
                {
                    "type": "custom",
                    "custom": { "name": "no_format" }
                }
            ]
        });
        let req = parse_json(body).unwrap();
        assert!(req.prompt.contains("Invocation method:"));
        assert!(req.prompt.contains("No constraints"));
    }

    #[test]
    fn tool_empty_name() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [
                { "type": "function", "function": { "name": "" } }
            ]
        });
        let err = parse_json(body).unwrap_err();
        assert!(matches!(err, OpenAIAdapterError::BadRequest(_)));
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn tool_choice_required_without_tools() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "tool_choice": "required"
        });
        let err = parse_json(body).unwrap_err();
        assert!(matches!(err, OpenAIAdapterError::BadRequest(_)));
    }

    #[test]
    fn allowed_tools_bad_mode() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [
                { "type": "function", "function": { "name": "f" } }
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "allowed_tools": { "mode": "invalid", "tools": [] }
            }
        });
        let err = parse_json(body).unwrap_err();
        assert!(matches!(err, OpenAIAdapterError::BadRequest(_)));
    }

    // tools injection 位置：嵌入到最后一个 <｜Assistant｜> 后的 <think>  block

    #[test]
    fn tools_injected_into_think_block() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [
                { "role": "user", "content": "First question" },
                { "role": "assistant", "content": "Answer" },
                { "role": "user", "content": "Second question" }
            ],
            "tools": [
                { "type": "function", "function": { "name": "calc" } }
            ]
        });
        let req = parse_json(body).unwrap();
        let prompt = &req.prompt;
        // Tool definition应注入到最后一个 <｜Assistant｜><think>  block
        assert!(
            prompt.contains("<｜Assistant｜><think>嗯，我刚刚被系统提醒需要遵循以下内容:"),
            "Tool definitions should be injected into <think> block"
        );
        assert!(prompt.contains("## Tool Calls"));
        assert!(prompt.contains("calc"));
        // <think> 块应在最后，位于最后的 user 消息之后
        let think_pos = prompt.find("<｜Assistant｜><think>").unwrap();
        let last_user_pos = prompt.rfind("Second question").unwrap();
        assert!(
            think_pos > last_user_pos,
            "<think> block should be after the last user message"
        );
    }

    // functions / function_call legacy compat downgrade

    #[test]
    fn functions_legacy_to_tools() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "Beijing天气？" }],
            "functions": [
                {
                    "name": "get_weather",
                    "description": "Get weather",
                    "parameters": { "type": "object", "properties": { "city": { "type": "string" } } }
                }
            ],
            "function_call": "auto"
        });
        let req = parse_json(body).unwrap();
        assert!(req.prompt.contains("get_weather"));
        assert!(req.prompt.contains("You can use the following tools"));
    }

    #[test]
    fn function_call_named_legacy() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "Check weather" }],
            "functions": [
                { "name": "get_weather", "parameters": {} }
            ],
            "function_call": { "name": "get_weather" }
        });
        let req = parse_json(body).unwrap();
        assert!(req.prompt.contains("你必须调用 'get_weather' 工具"));
    }

    #[test]
    fn tools_priority_over_functions() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [
                { "type": "function", "function": { "name": "tool_a", "parameters": {} } }
            ],
            "functions": [
                { "name": "func_b", "parameters": {} }
            ],
            "tool_choice": "auto",
            "function_call": { "name": "func_b" }
        });
        let req = parse_json(body).unwrap();
        assert!(req.prompt.contains("tool_a"));
        assert!(!req.prompt.contains("func_b"));
    }

    // response_format legacy compat downgrade

    #[test]
    fn response_format_json_object() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "Output JSON" }],
            "response_format": { "type": "json_object" }
        });
        let req = parse_json(body).unwrap();
        assert!(req.prompt.contains("Please output a valid JSON object directly"));
    }

    #[test]
    fn response_format_json_schema() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "Structured output" }],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "person",
                    "schema": { "type": "object", "properties": { "name": { "type": "string" } } }
                }
            }
        });
        let req = parse_json(body).unwrap();
        assert!(req.prompt.contains("Follow the format below"));
        assert!(req.prompt.contains("person"));
    }

    #[test]
    fn response_format_text_no_injection() {
        let body = serde_json::json!({
            "model": "deepseek-default",
            "messages": [{ "role": "user", "content": "hi" }],
            "response_format": { "type": "text" }
        });
        let req = parse_json(body).unwrap();
        assert!(!req.prompt.contains("Please output in"));
        assert!(!req.prompt.contains("JSON"));
    }
}
