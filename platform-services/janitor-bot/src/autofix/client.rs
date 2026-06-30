use crate::autofix::workspace::Workspace;
use crate::llm::LlmClient;
use anyhow::bail;
use async_openai::types::chat::{
    ChatCompletionMessageToolCalls, ChatCompletionRequestAssistantMessageArgs,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestToolMessageArgs, ChatCompletionRequestUserMessageArgs, ChatCompletionTool,
    ChatCompletionTools, FunctionObject,
};
use serde::Deserialize;
use serde_json::json;

pub const DEFAULT_MODEL: &str = "gpt-5.4";

/// Maximum tool-calling round-trips before we give up.
const MAX_STEPS: usize = 10;

/// A single file the model wants to rewrite in full.
#[derive(Debug, Clone, Deserialize)]
pub struct FileEdit {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
struct SubmitFixArgs {
    title: String,
    files: Vec<FileEdit>,
}

/// The model's proposed fix: a short human-readable title for the change plus
/// the full new contents of each file it wants to rewrite.
#[derive(Debug, Clone)]
pub struct ProposedFix {
    pub title: String,
    pub files: Vec<FileEdit>,
}

#[derive(Debug, Deserialize)]
struct PathArg {
    path: String,
}

#[derive(Debug, Deserialize)]
struct GrepArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

/// The PR metadata handed to the model alongside the failing CI logs.
pub struct PrMeta {
    pub pr_title: String,
    pub base_branch: String,
    pub head_branch: String,
}

/// The result of dispatching one tool call: either text to feed back to the
/// model, or the final set of edits when the model calls `submit_fix`.
enum ToolOutcome {
    Result(String),
    Done(ProposedFix),
}

const SYSTEM_PROMPT: &str = "You are an automated dependency-upgrade fixer for a GitOps repository. \
A Renovate pull request bumped one or more dependencies and CI is now failing. \
You are given the failing CI logs and a checkout of the repository at the PR's head. \
The bumped dependency usually breaks code in files the PR never touched, so use the \
`read_file`, `list_dir`, and `grep` tools to follow the errors to the real source files. \
When you have determined the fix, call `submit_fix` with a short imperative `title` \
summarising the change (e.g. \"Update deprecated serde derive attribute\") and the COMPLETE \
new contents of every file you change (full file, not a diff). Keep the change minimal and do \
not touch lockfiles you were not asked about. If you cannot determine a fix, call `submit_fix` \
with an empty file list.";

/// Drives the agentic autofix loop on top of a generic [`LlmClient`]: it owns
/// the prompts, tool definitions, and tool dispatch against a [`Workspace`].
pub struct LlmAutofixClient<'a> {
    llm: &'a LlmClient,
}

impl<'a> LlmAutofixClient<'a> {
    pub fn new(llm: &'a LlmClient) -> Self {
        Self { llm }
    }

    /// Lets the model explore `workspace` via read tools plus the failing CI
    /// logs, then returns the edits it submits via `submit_fix`.
    #[tracing::instrument(skip_all, fields(model))]
    pub async fn propose_fix(
        &self,
        workspace: &Workspace,
        failure_logs: &str,
        meta: &PrMeta,
        model: &str,
        instructions: Option<&str>,
    ) -> anyhow::Result<ProposedFix> {
        let tools = Self::tool_defs();
        let user_prompt = Self::user_prompt(meta, failure_logs, instructions);
        tracing::info!("user prompt: {user_prompt}");

        let mut messages: Vec<ChatCompletionRequestMessage> = vec![
            ChatCompletionRequestSystemMessageArgs::default()
                .content(SYSTEM_PROMPT)
                .build()?
                .into(),
            ChatCompletionRequestUserMessageArgs::default()
                .content(user_prompt)
                .build()?
                .into(),
        ];

        for _ in 0..MAX_STEPS {
            let msg = self
                .llm
                .chat(model, messages.clone(), tools.clone())
                .await?;

            tracing::info!("llm response: {:?}", msg.content);
            tracing::info!("tool calls: {:?}", msg.tool_calls);

            let Some(tool_calls) = msg.tool_calls.clone() else {
                bail!(
                    "model stopped without calling submit_fix: {}",
                    msg.content.unwrap_or_default()
                );
            };

            messages.push(
                ChatCompletionRequestAssistantMessageArgs::default()
                    .tool_calls(tool_calls.clone())
                    .build()?
                    .into(),
            );

            for call in tool_calls {
                let ChatCompletionMessageToolCalls::Function(call) = call else {
                    continue;
                };
                match Self::dispatch_tool(workspace, &call.function.name, &call.function.arguments)
                {
                    ToolOutcome::Done(fix) => return Ok(fix),
                    ToolOutcome::Result(text) => {
                        messages.push(
                            ChatCompletionRequestToolMessageArgs::default()
                                .tool_call_id(call.id)
                                .content(text)
                                .build()?
                                .into(),
                        );
                    }
                }
            }
        }

        bail!("autofix exceeded {MAX_STEPS} steps without producing a fix");
    }

    fn user_prompt(meta: &PrMeta, failure_logs: &str, instructions: Option<&str>) -> String {
        let mut user = String::new();
        user.push_str(&format!("PR title: {}\n", meta.pr_title));
        user.push_str(&format!(
            "Branch: {} (targets {})\n\n",
            meta.head_branch, meta.base_branch
        ));
        if let Some(instructions) = instructions.map(str::trim).filter(|s| !s.is_empty()) {
            user.push_str("## Extra instructions from the maintainer\n\n");
            user.push_str(instructions);
            user.push_str("\n\n");
        }
        user.push_str("## Failing CI logs\n\n");
        if failure_logs.is_empty() {
            user.push_str("(no logs available)\n");
        } else {
            user.push_str(failure_logs);
            user.push('\n');
        }
        user
    }

    fn tool_defs() -> Vec<ChatCompletionTools> {
        fn tool(
            name: &str,
            description: &str,
            parameters: serde_json::Value,
        ) -> ChatCompletionTools {
            ChatCompletionTools::Function(ChatCompletionTool {
                function: FunctionObject {
                    name: name.to_owned(),
                    description: Some(description.to_owned()),
                    parameters: Some(parameters),
                    strict: None,
                },
            })
        }

        vec![
            tool(
                "read_file",
                "Read the full contents of a file in the repository.",
                json!({
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "Repo-relative file path." } },
                    "required": ["path"]
                }),
            ),
            tool(
                "list_dir",
                "List the entries of a directory in the repository.",
                json!({
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "Repo-relative directory path; empty or '.' for the repo root." } },
                    "required": ["path"]
                }),
            ),
            tool(
                "grep",
                "Search the repository for a regular expression, returning matching path:line:text entries.",
                json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Regular expression to search for." },
                        "path": { "type": "string", "description": "Optional repo-relative path to limit the search to." }
                    },
                    "required": ["pattern"]
                }),
            ),
            tool(
                "submit_fix",
                "Submit the final fix as the complete new contents of each changed file.",
                json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Short imperative summary of the change, used as the fix PR title." },
                        "files": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "path": { "type": "string" },
                                    "content": { "type": "string" }
                                },
                                "required": ["path", "content"]
                            }
                        }
                    },
                    "required": ["title", "files"]
                }),
            ),
        ]
    }

    /// Dispatches a single tool call against the workspace. Tool errors are
    /// returned as text so the model can recover rather than aborting the loop.
    fn dispatch_tool(ws: &Workspace, name: &str, arguments: &str) -> ToolOutcome {
        match name {
            "read_file" => match serde_json::from_str::<PathArg>(arguments) {
                Ok(a) => ToolOutcome::Result(
                    ws.read_file(&a.path)
                        .unwrap_or_else(|e| format!("error: {e}")),
                ),
                Err(e) => ToolOutcome::Result(format!("error: invalid arguments: {e}")),
            },
            "list_dir" => match serde_json::from_str::<PathArg>(arguments) {
                Ok(a) => ToolOutcome::Result(
                    ws.list_dir(&a.path)
                        .unwrap_or_else(|e| format!("error: {e}")),
                ),
                Err(e) => ToolOutcome::Result(format!("error: invalid arguments: {e}")),
            },
            "grep" => match serde_json::from_str::<GrepArgs>(arguments) {
                Ok(a) => ToolOutcome::Result(
                    ws.grep(&a.pattern, a.path.as_deref())
                        .unwrap_or_else(|e| format!("error: {e}")),
                ),
                Err(e) => ToolOutcome::Result(format!("error: invalid arguments: {e}")),
            },
            "submit_fix" => match serde_json::from_str::<SubmitFixArgs>(arguments) {
                Ok(a) => ToolOutcome::Done(ProposedFix {
                    title: a.title,
                    files: a.files,
                }),
                Err(e) => ToolOutcome::Result(format!("error: invalid submit_fix arguments: {e}")),
            },
            other => ToolOutcome::Result(format!("error: unknown tool {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_with(files: &[(&str, &str)]) -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        for (path, content) in files {
            let abs = dir.path().join(path);
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(abs, content).unwrap();
        }
        let ws = Workspace::new(dir.path());
        (dir, ws)
    }

    #[test]
    fn read_file_dispatch() {
        let (_d, ws) = workspace_with(&[("src/main.rs", "fn main() {}")]);
        match LlmAutofixClient::dispatch_tool(&ws, "read_file", r#"{"path":"src/main.rs"}"#) {
            ToolOutcome::Result(text) => assert_eq!(text, "fn main() {}"),
            _ => panic!("expected Result"),
        }
    }

    #[test]
    fn submit_fix_parses_edits() {
        let (_d, ws) = workspace_with(&[]);
        let args =
            r#"{"title":"Fix main","files":[{"path":"a.rs","content":"fn main() {}"}]}"#;
        match LlmAutofixClient::dispatch_tool(&ws, "submit_fix", args) {
            ToolOutcome::Done(fix) => {
                assert_eq!(fix.title, "Fix main");
                assert_eq!(fix.files.len(), 1);
                assert_eq!(fix.files[0].path, "a.rs");
                assert_eq!(fix.files[0].content, "fn main() {}");
            }
            _ => panic!("expected Done"),
        }
    }

    #[test]
    fn submit_fix_invalid_args_is_recoverable() {
        let (_d, ws) = workspace_with(&[]);
        match LlmAutofixClient::dispatch_tool(&ws, "submit_fix", "not json") {
            ToolOutcome::Result(text) => assert!(text.starts_with("error:")),
            _ => panic!("expected recoverable Result"),
        }
    }

    #[test]
    fn unknown_tool_is_recoverable() {
        let (_d, ws) = workspace_with(&[]);
        match LlmAutofixClient::dispatch_tool(&ws, "frobnicate", "{}") {
            ToolOutcome::Result(text) => assert!(text.contains("unknown tool")),
            _ => panic!("expected Result"),
        }
    }

    #[test]
    fn user_prompt_includes_logs() {
        let meta = PrMeta {
            pr_title: "Bump serde to 2.0".into(),
            base_branch: "main".into(),
            head_branch: "renovate/serde".into(),
        };
        let prompt = LlmAutofixClient::user_prompt(&meta, "error[E0432]: unresolved import", None);
        assert!(prompt.contains("Bump serde to 2.0"));
        assert!(prompt.contains("renovate/serde"));
        assert!(prompt.contains("E0432"));
        assert!(!prompt.contains("Extra instructions"));
    }

    #[test]
    fn user_prompt_includes_extra_instructions() {
        let meta = PrMeta {
            pr_title: "Bump serde to 2.0".into(),
            base_branch: "main".into(),
            head_branch: "renovate/serde".into(),
        };
        let prompt = LlmAutofixClient::user_prompt(
            &meta,
            "error[E0432]: unresolved import",
            Some("prefer the derive feature"),
        );
        assert!(prompt.contains("Extra instructions"));
        assert!(prompt.contains("prefer the derive feature"));
    }
}
