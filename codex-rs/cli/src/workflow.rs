use clap::Parser;
use codex_common::CliConfigOverrides;
use codex_core::agents;
use codex_core::config::{self, Config, ConfigOverrides};
use codex_core::workflows::{self, StepKind};
use std::path::PathBuf;
use codex_core::ConversationManager;
use codex_core::NewConversation;
use codex_core::protocol::{Event, EventMsg, InputItem, Op, TaskCompleteEvent};
use tracing::error;

#[derive(Debug, Parser)]
pub struct WorkflowCli {
    #[clap(flatten)]
    pub config_overrides: CliConfigOverrides,

    #[command(subcommand)]
    pub cmd: WorkflowSubcommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum WorkflowSubcommand {
    /// Run a workflow defined under `.codex/workflows/<name>.toml`.
    Run(WorkflowRunArgs),
}

#[derive(Debug, Parser)]
pub struct WorkflowRunArgs {
    /// Workflow name (file stem under `.codex/workflows/`).
    pub name: String,

    /// Print events as JSONL.
    #[arg(long = "json", default_value_t = false)]
    pub json: bool,

    /// Write the last agent message to this file.
    #[arg(long = "output-last-message")]
    pub last_message_file: Option<PathBuf>,

    /// Working directory for the session (root for project discovery).
    #[arg(long = "cd", short = 'C')]
    pub cwd: Option<PathBuf>,

    /// Convenience alias for low-friction sandboxed automatic execution (-a on-failure, --sandbox workspace-write).
    #[arg(long = "full-auto", default_value_t = false)]
    pub full_auto: bool,

    /// EXTREMELY DANGEROUS. Skip confirmations and sandboxing.
    #[arg(long = "dangerously-bypass-approvals-and-sandbox", alias = "yolo", default_value_t = false)]
    pub dangerously_bypass_approvals_and_sandbox: bool,

    /// Configuration profile from config.toml to specify defaults.
    #[arg(long = "profile", short = 'p')]
    pub config_profile: Option<String>,
}

pub async fn run_main(cli: WorkflowCli, codex_linux_sandbox_exe: Option<PathBuf>) -> anyhow::Result<()> {
    match cli.cmd {
        WorkflowSubcommand::Run(args) => run_workflow(cli.config_overrides, args, codex_linux_sandbox_exe).await,
    }
}

async fn run_workflow(
    config_overrides: CliConfigOverrides,
    args: WorkflowRunArgs,
    codex_linux_sandbox_exe: Option<PathBuf>,
) -> anyhow::Result<()> {
    let WorkflowRunArgs {
        name,
        json,
        last_message_file,
        cwd,
        full_auto,
        dangerously_bypass_approvals_and_sandbox,
        config_profile,
    } = args;

    // Discover project `.codex` dir and load workflow definition.
    let project_dir = match agents::discover_project_codex_dir(cwd.clone())? {
        Some(dir) => dir,
        None => {
            anyhow::bail!("No project .codex/ directory discovered (use -C to set working dir)");
        }
    };
    let wf = workflows::load_workflow(&project_dir, &name)?;
    if wf.steps.is_empty() {
        println!("Workflow '{}' has no steps", wf.name);
        return Ok(());
    }

    // Load project config.toml as TOML for agent MCP inheritance.
    let project_cfg_toml = config::load_config_as_toml_with_cli_overrides(
        &config::find_codex_home()?,
        Vec::new(),
    )?;

    // Build a base Config that will be cloned and adjusted per step.
    let overrides = ConfigOverrides {
        model: None,
        config_profile,
        // Headless run: never prompt for approvals.
        approval_policy: Some(codex_core::protocol::AskForApproval::Never),
        sandbox_mode: if full_auto {
            Some(codex_protocol::config_types::SandboxMode::WorkspaceWrite)
        } else if dangerously_bypass_approvals_and_sandbox {
            Some(codex_protocol::config_types::SandboxMode::DangerFullAccess)
        } else {
            None
        },
        cwd: cwd.clone(),
        model_provider: None,
        codex_linux_sandbox_exe,
        base_instructions: None,
        include_plan_tool: None,
        include_apply_patch_tool: None,
        disable_response_storage: None,
        show_raw_agent_reasoning: None,
    };
    let cli_kv_overrides = config_overrides
        .parse_overrides()
        .map_err(|e| anyhow::anyhow!("Error parsing -c overrides: {e}"))?;
    let base_config = Config::load_with_cli_overrides(cli_kv_overrides, overrides)?;

    // Run each step sequentially as a clean session.
    for (idx, step) in wf.steps.iter().enumerate() {
        println!("--- Step {}/{}: {} {}", idx + 1, wf.steps.len(), match step.kind { StepKind::Agent => "agent", StepKind::Team => "team" }, step.id);

        // Derive agent + prompt for this step.
        let (_agent_name, combined_prompt, model_override, provider_override, include_plan, include_apply, mcp_servers) = match step.kind {
            StepKind::Agent => {
                let def = agents::load_agent(&project_dir, &step.id, &project_cfg_toml)?;
                (
                    step.id.clone(),
                    step.prompt.clone().or(def.prompt.clone()).unwrap_or_default(),
                    def.config.model.clone(),
                    def.config.model_provider.clone(),
                    def.config.include_plan_tool,
                    def.config.include_apply_patch_tool,
                    def.mcp_servers.clone(),
                )
            }
            StepKind::Team => {
                let team = agents::load_team(&project_dir, &step.id)?;
                let first_member = team
                    .config
                    .members
                    .first()
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!(format!("Team '{}' has no members", step.id)))?;
                let agent = agents::load_agent(&project_dir, &first_member, &project_cfg_toml)?;
                let combined_prompt = match (team.prompt.as_ref(), agent.prompt.as_ref(), step.prompt.as_ref()) {
                    // Priority: explicit step prompt if provided, otherwise TEAM + AGENT prompts.
                    (_t, _a, Some(p)) => Some(p.clone()),
                    (Some(t), Some(a), None) => Some(format!("{t}\n\n{a}")),
                    (Some(t), None, None) => Some(t.clone()),
                    (None, Some(a), None) => Some(a.clone()),
                    (None, None, None) => None,
                }
                .unwrap_or_default();
                (
                    first_member,
                    combined_prompt,
                    agent.config.model.clone(),
                    agent.config.model_provider.clone(),
                    agent.config.include_plan_tool,
                    agent.config.include_apply_patch_tool,
                    agent.mcp_servers.clone(),
                )
            }
        };

        // Derive per-step config by cloning and applying agent-specific overrides.
        let mut step_config = base_config.clone();
        if let Some(m) = model_override.as_ref() {
            step_config.model = m.clone();
            // Also refresh family and caps if needed – rely on Config::load for this; here we keep it simple.
        }
        if let Some(provider_id) = provider_override.as_ref() {
            if let Some(info) = step_config.model_providers.get(provider_id).cloned() {
                step_config.model_provider_id = provider_id.clone();
                step_config.model_provider = info;
            }
        }
        if let Some(v) = include_plan { step_config.include_plan_tool = v; }
        if let Some(v) = include_apply { step_config.include_apply_patch_tool = v; }
        step_config.base_instructions = Some(combined_prompt.clone());
        step_config.mcp_servers = mcp_servers;

        // Run this step as a clean session using a minimal inline runner.
        run_step_with_config(step_config, combined_prompt, json, last_message_file.clone()).await?;
    }

    Ok(())
}

/// Minimal non-interactive runner for a single step using a pre-built Config.
async fn run_step_with_config(
    config: Config,
    prompt: String,
    json_mode: bool,
    last_message_file: Option<PathBuf>,
) -> anyhow::Result<()> {

    // Create conversation
    let conversation_manager = ConversationManager::default();
    let NewConversation { conversation_id: _, conversation, session_configured: _ } =
        conversation_manager.new_conversation(config.clone()).await?;

    // Print a compact config summary and the prompt (simple version)
    if !json_mode {
        let entries = codex_common::create_config_summary_entries(&config);
        eprintln!("Workflow step config:");
        for (k, v) in entries { eprintln!("- {k} {v}"); }
        eprintln!("--------\nUser instructions:\n{prompt}");
    }

    // Event loop
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    {
        let conversation = conversation.clone();
        tokio::spawn(async move {
            loop {
                match conversation.next_event().await {
                    Ok(event) => {
                        let is_shutdown_complete = matches!(event.msg, EventMsg::ShutdownComplete);
                        if let Err(e) = tx.send(event) { error!("send event: {e:?}"); break; }
                        if is_shutdown_complete { break; }
                    }
                    Err(e) => { error!("next_event: {e:?}"); break; }
                }
            }
        });
    }

    // Send prompt
    let _ = conversation
        .submit(Op::UserInput { items: vec![InputItem::Text { text: prompt }] })
        .await?;

    // Drain until TaskComplete, then Shutdown
    let mut last_message: Option<String> = None;
    while let Some(event) = rx.recv().await {
        if let EventMsg::TaskComplete(TaskCompleteEvent { ref last_agent_message }) = event.msg {
            last_message = last_agent_message.clone();
            conversation.submit(Op::Shutdown).await?;
        }
        if matches!(event.msg, EventMsg::ShutdownComplete) { break; }
    }

    // Output last message
    if let Some(text) = last_message {
        if let Some(path) = last_message_file.as_deref() {
            let _ = std::fs::write(path, &text);
        }
        if json_mode {
            println!("{{\"type\":\"last_message\",\"text\":{}}}", serde_json::to_string(&text)?);
        } else {
            println!("\n{text}");
        }
    }

    Ok(())
}
