use crate::LoginStatus;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::chatwidget::ChatWidget;
use crate::file_search::FileSearchManager;
use crate::get_git_diff::get_git_diff;
use crate::get_login_status;
use crate::onboarding::onboarding_screen::KeyboardHandler;
use crate::onboarding::onboarding_screen::OnboardingScreen;
use crate::onboarding::onboarding_screen::OnboardingScreenArgs;
use crate::slash_command::SlashCommand;
use crate::tui;
use crate::history_cell::new_info_block;
use crate::history_cell::HistoryCell;
use codex_core::agents;
use codex_core::protocol::InputItem;
use codex_core::NewConversation;
// ConversationManager already imported below; avoid duplicate import
#[derive(Clone, Debug)]
struct TeamContext {
    name: String,
    prompt: Option<String>,
    members: Vec<String>,
    mode: Option<String>,
    next_idx: usize,
    turns_taken: usize,
    max_turns: Option<usize>,
    selector_model: Option<String>,
    selector_prompt: Option<String>,
    allow_repeated_speaker: bool,
}

#[derive(Clone, Debug)]
struct WorkflowContext {
    name: String,
    steps: Vec<WorkflowStepRuntime>,
    index: usize,
}

#[derive(Clone, Debug)]
struct WorkflowStepRuntime {
    kind: String, // agent|team
    id: String,
    prompt: Option<String>,
}
use codex_core::ConversationManager;
use codex_core::config::Config;
use codex_core::protocol::Event;
use codex_core::protocol::Op;
use color_eyre::eyre::Result;
use crossterm::SynchronizedUpdate;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::terminal::supports_keyboard_enhancement;
use ratatui::layout::Offset;
use ratatui::prelude::Backend;
use ratatui::text::Line;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;
use std::time::Instant;

/// Time window for debouncing redraw requests.
const REDRAW_DEBOUNCE: Duration = Duration::from_millis(1);

/// Top-level application state: which full-screen view is currently active.
#[allow(clippy::large_enum_variant)]
enum AppState<'a> {
    Onboarding {
        screen: OnboardingScreen,
    },
    /// The main chat UI is visible.
    Chat {
        /// Boxed to avoid a large enum variant and reduce the overall size of
        /// `AppState`.
        widget: Box<ChatWidget<'a>>,
    },
}

pub(crate) struct App<'a> {
    server: Arc<ConversationManager>,
    app_event_tx: AppEventSender,
    app_event_rx: Receiver<AppEvent>,
    app_state: AppState<'a>,

    /// Config is stored here so we can recreate ChatWidgets as needed.
    config: Config,

    file_search: FileSearchManager,

    pending_history_lines: Vec<Line<'static>>,

    enhanced_keys_supported: bool,

    /// Controls the animation thread that sends CommitTick events.
    commit_anim_running: Arc<AtomicBool>,

    /// Channel to schedule one-shot animation frames; coalesced by a single
    /// scheduler thread.
    frame_schedule_tx: std::sync::mpsc::Sender<Instant>,
    /// Optional active team context when the user switched to a team.
    team_context: Option<TeamContext>,
    /// Optional active workflow context (sequential preview).
    workflow_context: Option<WorkflowContext>,
}

/// Aggregate parameters needed to create a `ChatWidget`, as creation may be
/// deferred until after the Git warning screen is dismissed.
#[derive(Clone, Debug)]
pub(crate) struct ChatWidgetArgs {
    pub(crate) config: Config,
    initial_prompt: Option<String>,
    initial_images: Vec<PathBuf>,
    enhanced_keys_supported: bool,
}

impl App<'_> {
    pub(crate) fn new(
        config: Config,
        initial_prompt: Option<String>,
        initial_images: Vec<std::path::PathBuf>,
        show_trust_screen: bool,
    ) -> Self {
        let conversation_manager = Arc::new(ConversationManager::default());

        let (app_event_tx, app_event_rx) = channel();
        let app_event_tx = AppEventSender::new(app_event_tx);

        let enhanced_keys_supported = supports_keyboard_enhancement().unwrap_or(false);

        // Spawn a dedicated thread for reading the crossterm event loop and
        // re-publishing the events as AppEvents, as appropriate.
        {
            let app_event_tx = app_event_tx.clone();
            std::thread::spawn(move || {
                loop {
                    // This timeout is necessary to avoid holding the event lock
                    // that crossterm::event::read() acquires. In particular,
                    // reading the cursor position (crossterm::cursor::position())
                    // needs to acquire the event lock, and so will fail if it
                    // can't acquire it within 2 sec. Resizing the terminal
                    // crashes the app if the cursor position can't be read.
                    if let Ok(true) = crossterm::event::poll(Duration::from_millis(100)) {
                        if let Ok(event) = crossterm::event::read() {
                            match event {
                                crossterm::event::Event::Key(key_event) => {
                                    app_event_tx.send(AppEvent::KeyEvent(key_event));
                                }
                                crossterm::event::Event::Resize(_, _) => {
                                    app_event_tx.send(AppEvent::RequestRedraw);
                                }
                                crossterm::event::Event::Paste(pasted) => {
                                    // Many terminals convert newlines to \r when pasting (e.g., iTerm2),
                                    // but tui-textarea expects \n. Normalize CR to LF.
                                    // [tui-textarea]: https://github.com/rhysd/tui-textarea/blob/4d18622eeac13b309e0ff6a55a46ac6706da68cf/src/textarea.rs#L782-L783
                                    // [iTerm2]: https://github.com/gnachman/iTerm2/blob/5d0c0d9f68523cbd0494dad5422998964a2ecd8d/sources/iTermPasteHelper.m#L206-L216
                                    let pasted = pasted.replace("\r", "\n");
                                    app_event_tx.send(AppEvent::Paste(pasted));
                                }
                                _ => {
                                    // Ignore any other events.
                                }
                            }
                        }
                    } else {
                        // Timeout expired, no `Event` is available
                    }
                }
            });
        }

        let login_status = get_login_status(&config);
        let should_show_onboarding =
            should_show_onboarding(login_status, &config, show_trust_screen);
        let app_state = if should_show_onboarding {
            let show_login_screen = should_show_login_screen(login_status, &config);
            let chat_widget_args = ChatWidgetArgs {
                config: config.clone(),
                initial_prompt,
                initial_images,
                enhanced_keys_supported,
            };
            AppState::Onboarding {
                screen: OnboardingScreen::new(OnboardingScreenArgs {
                    event_tx: app_event_tx.clone(),
                    codex_home: config.codex_home.clone(),
                    cwd: config.cwd.clone(),
                    show_trust_screen,
                    show_login_screen,
                    chat_widget_args,
                    login_status,
                }),
            }
        } else {
            let chat_widget = ChatWidget::new(
                config.clone(),
                conversation_manager.clone(),
                app_event_tx.clone(),
                initial_prompt,
                initial_images,
                enhanced_keys_supported,
            );
            AppState::Chat {
                widget: Box::new(chat_widget),
            }
        };

        let file_search = FileSearchManager::new(config.cwd.clone(), app_event_tx.clone());

        // Spawn a single scheduler thread that coalesces both debounced redraw
        // requests and animation frame requests, and emits a single Redraw event
        // at the earliest requested time.
        let (frame_tx, frame_rx) = channel::<Instant>();
        {
            let app_event_tx = app_event_tx.clone();
            std::thread::spawn(move || {
                use std::sync::mpsc::RecvTimeoutError;
                let mut next_deadline: Option<Instant> = None;
                loop {
                    if next_deadline.is_none() {
                        match frame_rx.recv() {
                            Ok(deadline) => next_deadline = Some(deadline),
                            Err(_) => break,
                        }
                    }

                    #[expect(clippy::expect_used)]
                    let deadline = next_deadline.expect("deadline set");
                    let now = Instant::now();
                    let timeout = if deadline > now {
                        deadline - now
                    } else {
                        Duration::from_millis(0)
                    };

                    match frame_rx.recv_timeout(timeout) {
                        Ok(new_deadline) => {
                            next_deadline =
                                Some(next_deadline.map_or(new_deadline, |d| d.min(new_deadline)));
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            app_event_tx.send(AppEvent::Redraw);
                            next_deadline = None;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
        }
        Self {
            server: conversation_manager,
            app_event_tx,
            pending_history_lines: Vec::new(),
            app_event_rx,
            app_state,
            config,
            file_search,
            enhanced_keys_supported,
            commit_anim_running: Arc::new(AtomicBool::new(false)),
            frame_schedule_tx: frame_tx,
            team_context: None,
            workflow_context: None,
        }
    }

    fn schedule_frame_in(&self, dur: Duration) {
        let _ = self.frame_schedule_tx.send(Instant::now() + dur);
    }

    fn start_current_workflow_step(&mut self) {
        let Some(ctx) = &self.workflow_context else { return; };
        if ctx.index >= ctx.steps.len() { return; }
        let step = ctx.steps[ctx.index].clone();
        match step.kind.as_str() {
            "agent" => {
                self.app_event_tx.send(AppEvent::SwitchToAgent { name: step.id, initial_prompt: step.prompt });
            }
            "team" => {
                // Switch to team; initial prompt sent to first member; team context will be set.
                self.app_event_tx.send(AppEvent::SwitchToAgent { name: step.id, initial_prompt: step.prompt });
            }
            _ => {
                self.pending_history_lines.extend(new_info_block(vec![format!("Unsupported step kind: {}", step.kind)]).display_lines());
                self.app_event_tx.send(AppEvent::RequestRedraw);
            }
        }
    }

    fn advance_workflow(&mut self) {
        if let Some(ctx) = &mut self.workflow_context {
            ctx.index += 1;
            if ctx.index < ctx.steps.len() {
                self.start_current_workflow_step();
            } else {
                let name = ctx.name.clone();
                self.workflow_context = None;
                self.pending_history_lines.extend(new_info_block(vec![format!("Workflow '{}' completed", name)]).display_lines());
                self.app_event_tx.send(AppEvent::RequestRedraw);
            }
        }
    }

    pub(crate) fn run(&mut self, terminal: &mut tui::Tui) -> Result<()> {
        // Schedule the first render immediately.
        let _ = self.frame_schedule_tx.send(Instant::now());

        while let Ok(event) = self.app_event_rx.recv() {
            match event {
                AppEvent::InsertHistory(lines) => {
                    self.pending_history_lines.extend(lines);
                    self.app_event_tx.send(AppEvent::RequestRedraw);
                }
                AppEvent::RequestRedraw => {
                    self.schedule_frame_in(REDRAW_DEBOUNCE);
                }
                AppEvent::ScheduleFrameIn(dur) => {
                    self.schedule_frame_in(dur);
                }
                AppEvent::Redraw => {
                    std::io::stdout().sync_update(|_| self.draw_next_frame(terminal))??;
                }
                AppEvent::StartCommitAnimation => {
                    if self
                        .commit_anim_running
                        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                        .is_ok()
                    {
                        let tx = self.app_event_tx.clone();
                        let running = self.commit_anim_running.clone();
                        thread::spawn(move || {
                            while running.load(Ordering::Relaxed) {
                                thread::sleep(Duration::from_millis(50));
                                tx.send(AppEvent::CommitTick);
                            }
                        });
                    }
                }
                AppEvent::StopCommitAnimation => {
                    self.commit_anim_running.store(false, Ordering::Release);
                }
                AppEvent::CommitTick => {
                    if let AppState::Chat { widget } = &mut self.app_state {
                        widget.on_commit_tick();
                    }
                }
                AppEvent::KeyEvent(key_event) => {
                    match key_event {
                        KeyEvent {
                            code: KeyCode::Char('c'),
                            modifiers: crossterm::event::KeyModifiers::CONTROL,
                            kind: KeyEventKind::Press,
                            ..
                        } => match &mut self.app_state {
                            AppState::Chat { widget } => {
                                widget.on_ctrl_c();
                            }
                            AppState::Onboarding { .. } => {
                                self.app_event_tx.send(AppEvent::ExitRequest);
                            }
                        },
                        KeyEvent {
                            code: KeyCode::Char('z'),
                            modifiers: crossterm::event::KeyModifiers::CONTROL,
                            kind: KeyEventKind::Press,
                            ..
                        } => {
                            #[cfg(unix)]
                            {
                                self.suspend(terminal)?;
                            }
                            // No-op on non-Unix platforms.
                        }
                        KeyEvent {
                            code: KeyCode::Char('d'),
                            modifiers: crossterm::event::KeyModifiers::CONTROL,
                            kind: KeyEventKind::Press,
                            ..
                        } => {
                            match &mut self.app_state {
                                AppState::Chat { widget } => {
                                    if widget.composer_is_empty() {
                                        self.app_event_tx.send(AppEvent::ExitRequest);
                                    } else {
                                        // Treat Ctrl+D as a normal key event when the composer
                                        // is not empty so that it doesn't quit the application
                                        // prematurely.
                                        self.dispatch_key_event(key_event);
                                    }
                                }
                                AppState::Onboarding { .. } => {
                                    self.app_event_tx.send(AppEvent::ExitRequest);
                                }
                            }
                        }
                        KeyEvent {
                            kind: KeyEventKind::Press | KeyEventKind::Repeat,
                            ..
                        } => {
                            self.dispatch_key_event(key_event);
                        }
                        _ => {
                            // Ignore Release key events.
                        }
                    };
                }
                AppEvent::Paste(text) => {
                    self.dispatch_paste_event(text);
                }
                AppEvent::CodexEvent(event) => {
                    // Intercept TaskComplete to advance workflow steps, then forward to UI.
                    if let codex_core::protocol::EventMsg::TaskComplete(_ev) = &event.msg {
                        if self.workflow_context.is_some() {
                            self.advance_workflow();
                        }
                    }
                    self.dispatch_codex_event(event);
                }
                AppEvent::ExitRequest => {
                    break;
                }
                AppEvent::RunWorkflow { name } => {
                    // Discover and load workflow
                    let mut lines: Vec<String> = Vec::new();
                    match codex_core::agents::discover_project_codex_dir(Some(self.config.cwd.clone())) {
                        Ok(Some(project_dir)) => match codex_core::workflows::load_workflow(&project_dir, &name) {
                            Ok(wf) => {
                                if wf.steps.is_empty() {
                                    self.pending_history_lines.extend(new_info_block(vec![format!("Workflow '{}' has no steps", name)]).display_lines());
                                    self.app_event_tx.send(AppEvent::RequestRedraw);
                                } else {
                                    // Build runtime steps
                                    let steps: Vec<WorkflowStepRuntime> = wf
                                        .steps
                                        .into_iter()
                                        .map(|s| WorkflowStepRuntime {
                                            kind: match s.kind { codex_core::workflows::StepKind::Agent => "agent".to_string(), codex_core::workflows::StepKind::Team => "team".to_string() },
                                            id: s.id,
                                            prompt: s.prompt,
                                        })
                                        .collect();
                                    self.workflow_context = Some(WorkflowContext { name: wf.name, steps, index: 0 });
                                    self.start_current_workflow_step();
                                }
                            }
                            Err(e) => {
                                lines.push(format!("Failed to load workflow '{name}': {e}"));
                                self.pending_history_lines.extend(new_info_block(lines).display_lines());
                                self.app_event_tx.send(AppEvent::RequestRedraw);
                            }
                        },
                        Ok(None) => {
                            lines.push("No project .codex/ directory discovered".to_string());
                            self.pending_history_lines.extend(new_info_block(lines).display_lines());
                            self.app_event_tx.send(AppEvent::RequestRedraw);
                        }
                        Err(e) => {
                            lines.push(format!("Error discovering project: {e}"));
                            self.pending_history_lines.extend(new_info_block(lines).display_lines());
                            self.app_event_tx.send(AppEvent::RequestRedraw);
                        }
                    }
                }
                AppEvent::SwitchToAgent { name, initial_prompt } => {
                    // Discover project and load agent definition.
                    let mut lines: Vec<String> = Vec::new();
                    match agents::discover_project_codex_dir(Some(self.config.cwd.clone())) {
                        Ok(Some(project_dir)) => {
                            // Load project ConfigToml with CLI overrides set to none
                            let codex_home = self.config.codex_home.clone();
                            let config_toml = match codex_core::config::load_config_as_toml_with_cli_overrides(&codex_home, Vec::new()) {
                                Ok(t) => t,
                                Err(e) => {
                                    lines.push(format!("Error loading config.toml: {e}"));
                                    self.pending_history_lines.extend(new_info_block(lines).display_lines());
                                    continue;
                                }
                            };

                            // Try team by name first; if found, pick first member.
                            if let Ok(team_def) = agents::load_team(&project_dir, &name) {
                                if let Some(first_member) = team_def.config.members.first() {
                                    match agents::load_agent(&project_dir, first_member, &config_toml) {
                                        Ok(agent_def) => {
                                            let mut new_cfg = self.config.clone();
                                            if let Some(m) = agent_def.config.model.as_ref() { new_cfg.model = m.clone(); }
                                            if let Some(provider_id) = agent_def.config.model_provider.as_ref() {
                                                if let Some(info) = new_cfg.model_providers.get(provider_id).cloned() {
                                                    new_cfg.model_provider_id = provider_id.clone();
                                                    new_cfg.model_provider = info;
                                                }
                                            }
                                            if let Some(v) = agent_def.config.include_apply_patch_tool { new_cfg.include_apply_patch_tool = v; }
                                            if let Some(v) = agent_def.config.include_plan_tool { new_cfg.include_plan_tool = v; }
                                            // Combine team prompt + agent prompt if present.
                                            let combined_prompt = match (team_def.prompt.as_ref(), agent_def.prompt.as_ref()) {
                                                (Some(t), Some(a)) => Some(format!("{t}\n\n{a}")),
                                                (Some(t), None) => Some(t.clone()),
                                                (None, Some(a)) => Some(a.clone()),
                                                (None, None) => None,
                                            };
                                            if let Some(p) = combined_prompt { new_cfg.base_instructions = Some(p); }
                                            new_cfg.mcp_servers = agent_def.mcp_servers.clone();
                                            let new_widget = Box::new(ChatWidget::new(
                                                new_cfg,
                                                self.server.clone(),
                                                self.app_event_tx.clone(),
                                                initial_prompt,
                                                Vec::new(),
                                                self.enhanced_keys_supported,
                                            ));
                                            self.app_state = AppState::Chat { widget: new_widget };
                                            self.app_event_tx.send(AppEvent::RequestRedraw);
                                            // Activate team context for subsequent @member overrides.
                                            // Extract simple termination.max_turns if present
                                            let max_turns = team_def
                                                .config
                                                .termination
                                                .get("max_turns")
                                                .and_then(|v| v.as_integer())
                                                .map(|i| i as usize);
                                            // Extract selector config
                                            let (selector_model, selector_prompt, allow_repeated_speaker) = {
                                                let m = team_def
                                                    .config
                                                    .selector
                                                    .get("model")
                                                    .and_then(|v| v.as_str())
                                                    .map(|s| s.to_string());
                                                let p = team_def
                                                    .config
                                                    .selector
                                                    .get("prompt_file")
                                                    .and_then(|v| v.as_str())
                                                    .and_then(|s| team_def.file.parent().map(|d| d.join(s)))
                                                    .and_then(|path| std::fs::read_to_string(path).ok());
                                                let ars = team_def
                                                    .config
                                                    .selector
                                                    .get("allow_repeated_speaker")
                                                    .and_then(|v| v.as_bool())
                                                    .unwrap_or(false);
                                                (m, p, ars)
                                            };
                                            self.team_context = Some(TeamContext {
                                                name: name.clone(),
                                                prompt: team_def.prompt.clone(),
                                                members: team_def.config.members.clone(),
                                                mode: team_def.config.mode.clone(),
                                                next_idx: 0,
                                                turns_taken: 0,
                                                max_turns,
                                                selector_model,
                                                selector_prompt,
                                                allow_repeated_speaker,
                                            });
                                        }
                                        Err(e) => {
                                            lines.push(format!("Failed to load first member '{first_member}' of team '{name}': {e}"));
                                            self.pending_history_lines.extend(new_info_block(lines).display_lines());
                                        }
                                    }
                                    continue;
                                } else {
                                    lines.push(format!("Team '{name}' has no members"));
                                    self.pending_history_lines.extend(new_info_block(lines).display_lines());
                                    continue;
                                }
                            }

                            match agents::load_agent(&project_dir, &name, &config_toml) {
                                Ok(agent_def) => {
                                    // Build a new Config by applying agent target on top of current.
                                    let mut new_cfg = self.config.clone();
                                    if let Some(m) = agent_def.config.model.as_ref() { new_cfg.model = m.clone(); }
                                    if let Some(provider_id) = agent_def.config.model_provider.as_ref() {
                                        if let Some(info) = new_cfg.model_providers.get(provider_id).cloned() {
                                            new_cfg.model_provider_id = provider_id.clone();
                                            new_cfg.model_provider = info;
                                        }
                                    }
                                    if let Some(v) = agent_def.config.include_apply_patch_tool { new_cfg.include_apply_patch_tool = v; }
                                    if let Some(v) = agent_def.config.include_plan_tool { new_cfg.include_plan_tool = v; }
                                    // If we are in an active team context, combine team prompt with agent prompt.
                                    if let Some(tc) = &self.team_context {
                                        let combined = match (tc.prompt.as_ref(), agent_def.prompt.as_ref()) {
                                            (Some(t), Some(a)) => Some(format!("{t}\n\n{a}")),
                                            (Some(t), None) => Some(t.clone()),
                                            (None, Some(a)) => Some(a.clone()),
                                            (None, None) => None,
                                        };
                                        if let Some(p) = combined { new_cfg.base_instructions = Some(p); }
                                    } else if let Some(prompt) = agent_def.prompt.as_ref() {
                                        new_cfg.base_instructions = Some(prompt.clone());
                                    }
                                    new_cfg.mcp_servers = agent_def.mcp_servers.clone();

                                    // Spawn a fresh ChatWidget (new session) with optional initial prompt
                                    let new_widget = Box::new(ChatWidget::new(
                                        new_cfg,
                                        self.server.clone(),
                                        self.app_event_tx.clone(),
                                        initial_prompt,
                                        Vec::new(),
                                        self.enhanced_keys_supported,
                                    ));
                                    self.app_state = AppState::Chat { widget: new_widget };
                                    self.app_event_tx.send(AppEvent::RequestRedraw);
                                }
                                Err(e) => {
                                    lines.push(format!("Unknown agent or team '@{name}' (load error: {e})"));
                                    self.pending_history_lines.extend(new_info_block(lines).display_lines());
                                }
                            }
                        }
                        Ok(None) => {
                            lines.push("No project .codex/ directory discovered".to_string());
                            self.pending_history_lines.extend(new_info_block(lines).display_lines());
                        }
                        Err(e) => {
                            lines.push(format!("Error discovering project: {e}"));
                            self.pending_history_lines.extend(new_info_block(lines).display_lines());
                        }
                    }
                }
                AppEvent::CodexOp(op) => match &mut self.app_state {
                    AppState::Chat { widget } => {
                        // Intercept user input when a team context is active to select a member.
                        if let Op::UserInput { items } = &op {
                            if let Some(InputItem::Text { text }) = items.first() {
                                // Skip if the user is explicitly tagging a target at start of line.
                                if let Some(tc) = &mut self.team_context {
                                    if !text.trim_start().starts_with('@') {
                                        // Check termination
                                        if let Some(limit) = tc.max_turns {
                                            if tc.turns_taken >= limit {
                                                let msg = format!("Team '{}' reached max_turns={}", tc.name, limit);
                                                self.pending_history_lines.extend(new_info_block(vec![msg]).display_lines());
                                                self.app_event_tx.send(AppEvent::RequestRedraw);
                                                continue;
                                            }
                                        }
                                        // Selection: if mode == selector, call LLM-based selector; else round-robin.
                                        let mode = tc.mode.clone().unwrap_or_else(|| "round_robin".to_string());
                                        if mode.eq_ignore_ascii_case("selector") {
                                            if tc.selector_model.is_none() {
                                                self.pending_history_lines.extend(new_info_block(vec!["Selector model not configured for team".to_string()]).display_lines());
                                                self.app_event_tx.send(AppEvent::RequestRedraw);
                                                continue;
                                            }
                                            let Some(selector_model) = tc.selector_model.clone() else {
                                                self
                                                    .pending_history_lines
                                                    .extend(new_info_block(vec!["Selector model not configured for team".to_string()]).display_lines());
                                                self.app_event_tx.send(AppEvent::RequestRedraw);
                                                continue;
                                            };
                                            let selector_prompt = tc.selector_prompt.clone();
                                            let team_name = tc.name.clone();
                                            let candidates = tc.members.clone();
                                            let message = text.clone();
                                            let allow_repeat = tc.allow_repeated_speaker;
                                            let last_idx = if tc.next_idx == 0 { tc.members.len().saturating_sub(1) } else { tc.next_idx - 1 };
                                            let last_speaker = tc.members.get(last_idx).cloned();
                                            let app_tx = self.app_event_tx.clone();
                                            let server = self.server.clone();
                                            let mut sel_cfg = self.config.clone();
                                            sel_cfg.model = selector_model;
                                            // Build selection prompt
                                            let built_prompt = build_selector_prompt(&team_name, &candidates, selector_prompt.as_deref(), &message, allow_repeat, last_speaker.as_deref());
                                            tokio::spawn(async move {
                                                match server.new_conversation(sel_cfg).await {
                                                    Ok(NewConversation { conversation, .. }) => {
                                                        let _ = conversation.submit(Op::UserInput { items: vec![InputItem::Text { text: built_prompt }] }).await;
                                                        let mut selected: Option<String> = None;
                                                        while let Ok(ev) = conversation.next_event().await {
                                                            if let codex_core::protocol::EventMsg::AgentMessage(msg) = ev.msg {
                                                                let name = msg.message.trim().to_string();
                                                                selected = Some(name);
                                                                break;
                                                            }
                                                        }
                                                        if let Some(name) = selected {
                                                            app_tx.send(AppEvent::SwitchToAgent { name, initial_prompt: Some(message) });
                                                        } else {
                                                            app_tx.send(AppEvent::InsertHistory(new_info_block(vec!["Selector returned no choice".to_string()]).display_lines()));
                                                            app_tx.send(AppEvent::RequestRedraw);
                                                        }
                                                    }
                                                    Err(e) => {
                                                        app_tx.send(AppEvent::InsertHistory(new_info_block(vec![format!("Selector init failed: {e}")]).display_lines()));
                                                        app_tx.send(AppEvent::RequestRedraw);
                                                    }
                                                }
                                            });
                                            continue;
                                        } else {
                                            if tc.members.is_empty() {
                                                self.pending_history_lines.extend(new_info_block(vec!["Team has no members".to_string()]).display_lines());
                                                self.app_event_tx.send(AppEvent::RequestRedraw);
                                                continue;
                                            }
                                            let idx = tc.next_idx % tc.members.len();
                                            let member = tc
                                                .members
                                                .get(idx)
                                                .cloned()
                                                .unwrap_or_else(|| tc.members[0].clone());
                                            tc.next_idx = (tc.next_idx + 1) % tc.members.len();
                                            tc.turns_taken += 1;
                                            self.app_event_tx.send(AppEvent::SwitchToAgent { name: member, initial_prompt: Some(text.clone()) });
                                            continue;
                                        }
                                    }
                                }
                            }
                        }
                        widget.submit_op(op)
                    }
                    AppState::Onboarding { .. } => {}
                },
                AppEvent::DiffResult(text) => {
                    if let AppState::Chat { widget } = &mut self.app_state {
                        widget.add_diff_output(text);
                    }
                }
                AppEvent::DispatchCommand(command) => match command {
                    SlashCommand::New => {
                        // User accepted – switch to chat view.
                        let new_widget = Box::new(ChatWidget::new(
                            self.config.clone(),
                            self.server.clone(),
                            self.app_event_tx.clone(),
                            None,
                            Vec::new(),
                            self.enhanced_keys_supported,
                        ));
                        self.app_state = AppState::Chat { widget: new_widget };
                        self.app_event_tx.send(AppEvent::RequestRedraw);
                    }
                    SlashCommand::Init => {
                        // Initialize project-scoped .codex/ scaffolding if missing; otherwise advise discovery cmds.
                        let cwd = self.config.cwd.clone();
                        let project_dir = cwd.join(".codex");
                        let mut lines: Vec<String> = Vec::new();
                        if project_dir.exists() {
                            lines.push("Project .codex/ already exists; leaving as-is.".to_string());
                            lines.push("Try: /agents, /teams, /workflows to inspect.".to_string());
                        } else {
                            let mut created: Vec<String> = Vec::new();
                            let _ = std::fs::create_dir_all(project_dir.join("agents").join("dev"));
                            let _ = std::fs::create_dir_all(project_dir.join("teams"));
                            let _ = std::fs::create_dir_all(project_dir.join("workflows"));

                            // .codex/config.toml
                            let cfg = format!(
                                "# Project-scoped Codex config\nmodel = \"{}\"\n",
                                self.config.model
                            );
                            if std::fs::write(project_dir.join("config.toml"), cfg).is_ok() {
                                created.push(".codex/config.toml".to_string());
                            }

                            // .codex/AGENTS.md (project prompt)
                            let proj_agents_md = "You are Codex for this project. Be concise, direct, and safe.";
                            if std::fs::write(project_dir.join("AGENTS.md"), proj_agents_md).is_ok() {
                                created.push(".codex/AGENTS.md".to_string());
                            }

                            // Sample agent: dev
                            let agent_cfg = format!(
                                "name = \"dev\"\nrole = \"General developer\"\nmodel = \"{}\"\ninclude_plan_tool = true\n",
                                self.config.model
                            );
                            let agent_dir = project_dir.join("agents").join("dev");
                            if std::fs::write(agent_dir.join("config.toml"), agent_cfg).is_ok() {
                                created.push(".codex/agents/dev/config.toml".to_string());
                            }
                            let agent_prompt = "You are the Dev agent. Be practical and terse.";
                            if std::fs::write(agent_dir.join("AGENTS.md"), agent_prompt).is_ok() {
                                created.push(".codex/agents/dev/AGENTS.md".to_string());
                            }

                            // Sample team with selector mode
                            let team_toml = format!(
                                "mode = \"selector\"\n\n[selector]\nmodel = \"{model}\"\nallow_repeated_speaker = false\n\n# Members by agent directory name\nmembers = [\"dev\"]\n",
                                model = self.config.model
                            );
                            if std::fs::write(project_dir.join("teams").join("dev-team.toml"), team_toml).is_ok() {
                                created.push(".codex/teams/dev-team.toml".to_string());
                            }
                            let team_md = "Team prompt: collaborative developer team focusing on execution.";
                            if std::fs::write(project_dir.join("teams").join("TEAM.md"), team_md).is_ok() {
                                created.push(".codex/teams/TEAM.md".to_string());
                            }

                            // Sample workflow
                            let wf = r#"name = "sample"
description = "Sample sequential workflow"
steps = ["plan", "implement"]

[step.plan]
type = "team"
id = "dev-team"
prompt = "Draft a short plan."
max_turns = 1

[step.implement]
type = "agent"
id = "dev"
prompt = "Implement the plan with concise steps."
max_turns = 1
"#;
                            if std::fs::write(project_dir.join("workflows").join("sample.toml"), wf).is_ok() {
                                created.push(".codex/workflows/sample.toml".to_string());
                            }

                            if created.is_empty() {
                                lines.push("Failed to create project .codex scaffolding.".to_string());
                            } else {
                                lines.push("Initialized project .codex/ with sample config:".to_string());
                                for c in created { lines.push(format!("- {c}")); }
                                lines.push("Try: @agent dev <task>, @team dev-team <task>, or @workflow sample".to_string());
                            }
                        }
                        self.app_event_tx
                            .send(AppEvent::InsertHistory(new_info_block(lines).display_lines()));
                        self.app_event_tx.send(AppEvent::RequestRedraw);
                    }
                    SlashCommand::Compact => {
                        if let AppState::Chat { widget } = &mut self.app_state {
                            widget.clear_token_usage();
                            self.app_event_tx.send(AppEvent::CodexOp(Op::Compact));
                        }
                    }
                    SlashCommand::Quit => {
                        break;
                    }
                    SlashCommand::Logout => {
                        if let Err(e) = codex_login::logout(&self.config.codex_home) {
                            tracing::error!("failed to logout: {e}");
                        }
                        break;
                    }
                    SlashCommand::Diff => {
                        if let AppState::Chat { widget } = &mut self.app_state {
                            widget.add_diff_in_progress();
                        }

                        let tx = self.app_event_tx.clone();
                        tokio::spawn(async move {
                            let text = match get_git_diff().await {
                                Ok((is_git_repo, diff_text)) => {
                                    if is_git_repo {
                                        diff_text
                                    } else {
                                        "`/diff` — _not inside a git repository_".to_string()
                                    }
                                }
                                Err(e) => format!("Failed to compute diff: {e}"),
                            };
                            tx.send(AppEvent::DiffResult(text));
                        });
                    }
                    SlashCommand::Mention => {
                        if let AppState::Chat { widget } = &mut self.app_state {
                            widget.insert_str("@");
                        }
                    }
                    SlashCommand::Agents => {
                        if let AppState::Chat { .. } = &mut self.app_state {
                            let cwd = self.config.cwd.clone();
                            let mut lines: Vec<String> = Vec::new();
                            match codex_core::agents::discover_project_codex_dir(Some(cwd)) {
                                Ok(Some(dir)) => match codex_core::agents::list_agents(&dir) {
                                    Ok(names) if !names.is_empty() => {
                                        lines.push("Agents:".to_string());
                                        for n in names { lines.push(format!("- {n}")); }
                                    }
                                    Ok(_) => lines.push("No agents found in .codex/agents".to_string()),
                                    Err(e) => lines.push(format!("Error listing agents: {e}")),
                                },
                                Ok(None) => lines.push("No project .codex/ directory discovered".to_string()),
                                Err(e) => lines.push(format!("Error discovering project: {e}")),
                            }
                            self.app_event_tx
                                .send(AppEvent::InsertHistory(new_info_block(lines).display_lines()));
                            self.app_event_tx.send(AppEvent::RequestRedraw);
                        }
                    }
                    SlashCommand::Workflows => {
                        if let AppState::Chat { .. } = &mut self.app_state {
                            let cwd = self.config.cwd.clone();
                            let mut lines: Vec<String> = Vec::new();
                            match codex_core::agents::discover_project_codex_dir(Some(cwd)) {
                                Ok(Some(dir)) => match codex_core::workflows::discover_workflows(&dir) {
                                    Ok(names) if !names.is_empty() => {
                                        lines.push("Workflows:".to_string());
                                        for n in names { lines.push(format!("- {n}")); }
                                    }
                                    Ok(_) => lines.push("No workflows found in .codex/workflows".to_string()),
                                    Err(e) => lines.push(format!("Error listing workflows: {e}")),
                                },
                                Ok(None) => lines.push("No project .codex/ directory discovered".to_string()),
                                Err(e) => lines.push(format!("Error discovering project: {e}")),
                            }
                            self.app_event_tx
                                .send(AppEvent::InsertHistory(new_info_block(lines).display_lines()));
                            self.app_event_tx.send(AppEvent::RequestRedraw);
                        }
                    }
                    SlashCommand::Teams => {
                        if let AppState::Chat { .. } = &mut self.app_state {
                            let cwd = self.config.cwd.clone();
                            let mut lines: Vec<String> = Vec::new();
                            match codex_core::agents::discover_project_codex_dir(Some(cwd)) {
                                Ok(Some(dir)) => match codex_core::agents::list_teams(&dir) {
                                    Ok(names) if !names.is_empty() => {
                                        lines.push("Teams:".to_string());
                                        for n in names { lines.push(format!("- {n}")); }
                                    }
                                    Ok(_) => lines.push("No teams found in .codex/teams".to_string()),
                                    Err(e) => lines.push(format!("Error listing teams: {e}")),
                                },
                                Ok(None) => lines.push("No project .codex/ directory discovered".to_string()),
                                Err(e) => lines.push(format!("Error discovering project: {e}")),
                            }
                            self.app_event_tx
                                .send(AppEvent::InsertHistory(new_info_block(lines).display_lines()));
                            self.app_event_tx.send(AppEvent::RequestRedraw);
                        }
                    }
                    SlashCommand::Status => {
                        if let AppState::Chat { widget } = &mut self.app_state {
                            widget.add_status_output();
                        }
                    }
                    #[cfg(debug_assertions)]
                    SlashCommand::TestApproval => {
                        use codex_core::protocol::EventMsg;
                        use std::collections::HashMap;

                        use codex_core::protocol::ApplyPatchApprovalRequestEvent;
                        use codex_core::protocol::FileChange;

                        self.app_event_tx.send(AppEvent::CodexEvent(Event {
                            id: "1".to_string(),
                            // msg: EventMsg::ExecApprovalRequest(ExecApprovalRequestEvent {
                            //     call_id: "1".to_string(),
                            //     command: vec!["git".into(), "apply".into()],
                            //     cwd: self.config.cwd.clone(),
                            //     reason: Some("test".to_string()),
                            // }),
                            msg: EventMsg::ApplyPatchApprovalRequest(
                                ApplyPatchApprovalRequestEvent {
                                    call_id: "1".to_string(),
                                    changes: HashMap::from([
                                        (
                                            PathBuf::from("/tmp/test.txt"),
                                            FileChange::Add {
                                                content: "test".to_string(),
                                            },
                                        ),
                                        (
                                            PathBuf::from("/tmp/test2.txt"),
                                            FileChange::Update {
                                                unified_diff: "+test\n-test2".to_string(),
                                                move_path: None,
                                            },
                                        ),
                                    ]),
                                    reason: None,
                                    grant_root: Some(PathBuf::from("/tmp")),
                                },
                            ),
                        }));
                    }
                },
                AppEvent::OnboardingAuthComplete(result) => {
                    if let AppState::Onboarding { screen } = &mut self.app_state {
                        screen.on_auth_complete(result);
                    }
                }
                AppEvent::OnboardingComplete(ChatWidgetArgs {
                    config,
                    enhanced_keys_supported,
                    initial_images,
                    initial_prompt,
                }) => {
                    self.app_state = AppState::Chat {
                        widget: Box::new(ChatWidget::new(
                            config,
                            self.server.clone(),
                            self.app_event_tx.clone(),
                            initial_prompt,
                            initial_images,
                            enhanced_keys_supported,
                        )),
                    }
                }
                AppEvent::StartFileSearch(query) => {
                    if !query.is_empty() {
                        self.file_search.on_user_query(query);
                    }
                }
                AppEvent::FileSearchResult { query, matches } => {
                    if let AppState::Chat { widget } = &mut self.app_state {
                        widget.apply_file_search_result(query, matches);
                    }
                }
            }
        }
        terminal.clear()?;

        Ok(())
    }

    #[cfg(unix)]
    fn suspend(&mut self, terminal: &mut tui::Tui) -> Result<()> {
        tui::restore()?;
        // SAFETY: Unix-only code path. We intentionally send SIGTSTP to the
        // current process group (pid 0) to trigger standard job-control
        // suspension semantics. This FFI does not involve any raw pointers,
        // is not called from a signal handler, and uses a constant signal.
        // Errors from kill are acceptable (e.g., if already stopped) — the
        // subsequent re-init path will still leave the terminal in a good state.
        // We considered `nix`, but didn't think it was worth pulling in for this one call.
        unsafe { libc::kill(0, libc::SIGTSTP) };
        *terminal = tui::init(&self.config)?;
        terminal.clear()?;
        self.app_event_tx.send(AppEvent::RequestRedraw);
        Ok(())
    }

    pub(crate) fn token_usage(&self) -> codex_core::protocol::TokenUsage {
        match &self.app_state {
            AppState::Chat { widget } => widget.token_usage().clone(),
            AppState::Onboarding { .. } => codex_core::protocol::TokenUsage::default(),
        }
    }

    fn draw_next_frame(&mut self, terminal: &mut tui::Tui) -> Result<()> {
        if matches!(self.app_state, AppState::Onboarding { .. }) {
            terminal.clear()?;
        }

        let screen_size = terminal.size()?;
        let last_known_screen_size = terminal.last_known_screen_size;
        if screen_size != last_known_screen_size {
            let cursor_pos = terminal.get_cursor_position()?;
            let last_known_cursor_pos = terminal.last_known_cursor_pos;
            if cursor_pos.y != last_known_cursor_pos.y {
                // The terminal was resized. The only point of reference we have for where our viewport
                // was moved is the cursor position.
                // NB this assumes that the cursor was not wrapped as part of the resize.
                let cursor_delta = cursor_pos.y as i32 - last_known_cursor_pos.y as i32;

                let new_viewport_area = terminal.viewport_area.offset(Offset {
                    x: 0,
                    y: cursor_delta,
                });
                terminal.set_viewport_area(new_viewport_area);
                terminal.clear()?;
            }
        }

        let size = terminal.size()?;
        let desired_height = match &self.app_state {
            AppState::Chat { widget } => widget.desired_height(size.width),
            AppState::Onboarding { .. } => size.height,
        };

        let mut area = terminal.viewport_area;
        area.height = desired_height.min(size.height);
        area.width = size.width;
        if area.bottom() > size.height {
            terminal
                .backend_mut()
                .scroll_region_up(0..area.top(), area.bottom() - size.height)?;
            area.y = size.height - area.height;
        }
        if area != terminal.viewport_area {
            terminal.clear()?;
            terminal.set_viewport_area(area);
        }
        if !self.pending_history_lines.is_empty() {
            crate::insert_history::insert_history_lines(
                terminal,
                self.pending_history_lines.clone(),
            );
            self.pending_history_lines.clear();
        }
        terminal.draw(|frame| match &mut self.app_state {
            AppState::Chat { widget } => {
                if let Some((x, y)) = widget.cursor_pos(frame.area()) {
                    frame.set_cursor_position((x, y));
                }
                frame.render_widget_ref(&**widget, frame.area())
            }
            AppState::Onboarding { screen } => frame.render_widget_ref(&*screen, frame.area()),
        })?;
        Ok(())
    }

    /// Dispatch a KeyEvent to the current view and let it decide what to do
    /// with it.
    fn dispatch_key_event(&mut self, key_event: KeyEvent) {
        match &mut self.app_state {
            AppState::Chat { widget } => {
                widget.handle_key_event(key_event);
            }
            AppState::Onboarding { screen } => match key_event.code {
                KeyCode::Char('q') => {
                    self.app_event_tx.send(AppEvent::ExitRequest);
                }
                _ => screen.handle_key_event(key_event),
            },
        }
    }

    fn dispatch_paste_event(&mut self, pasted: String) {
        match &mut self.app_state {
            AppState::Chat { widget } => widget.handle_paste(pasted),
            AppState::Onboarding { .. } => {}
        }
    }

fn dispatch_codex_event(&mut self, event: Event) {
        match &mut self.app_state {
            AppState::Chat { widget } => widget.handle_codex_event(event),
            AppState::Onboarding { .. } => {}
        }
    }
}

fn build_selector_prompt(
    team_name: &str,
    candidates: &[String],
    selector_prompt: Option<&str>,
    user_message: &str,
    allow_repeated: bool,
    last_speaker: Option<&str>,
) -> String {
    let base = selector_prompt.unwrap_or(
        "You are a team orchestrator. Given the user message and the list of candidates, choose exactly one candidate to handle the next step.\n\nReturn ONLY the candidate name, exactly as shown in the list. No explanations."
    );
    let mut out = String::new();
    out.push_str(base);
    out.push_str("\n\nTeam: ");
    out.push_str(team_name);
    out.push_str("\nUser Message:\n");
    out.push_str(user_message);
    out.push_str("\n\nCandidates:\n");
    for c in candidates {
        out.push_str("- ");
        out.push_str(c);
        out.push('\n');
    }
    out.push_str("\nPolicy:\n");
    if !allow_repeated {
        out.push_str("- Do not choose the same speaker twice in a row.\n");
        if let Some(last) = last_speaker {
            out.push_str("- The last speaker was: ");
            out.push_str(last);
            out.push('\n');
        }
    }
    out.push_str("\nAnswer with exactly one candidate name from the list above.\n");
    out
}
fn should_show_onboarding(
    login_status: LoginStatus,
    config: &Config,
    show_trust_screen: bool,
) -> bool {
    if show_trust_screen {
        return true;
    }

    should_show_login_screen(login_status, config)
}

fn should_show_login_screen(login_status: LoginStatus, config: &Config) -> bool {
    match login_status {
        LoginStatus::NotAuthenticated => true,
        LoginStatus::AuthMode(method) => method != config.preferred_auth_method,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_core::config::ConfigOverrides;
    use codex_core::config::ConfigToml;
    use codex_login::AuthMode;

    fn make_config(preferred: AuthMode) -> Config {
        let mut cfg = Config::load_from_base_config_with_overrides(
            ConfigToml::default(),
            ConfigOverrides::default(),
            std::env::temp_dir(),
            None,
        )
        .expect("load default config");
        cfg.preferred_auth_method = preferred;
        cfg
    }

    #[test]
    fn shows_login_when_not_authenticated() {
        let cfg = make_config(AuthMode::ChatGPT);
        assert!(should_show_login_screen(
            LoginStatus::NotAuthenticated,
            &cfg
        ));
    }

    #[test]
    fn shows_login_when_api_key_but_prefers_chatgpt() {
        let cfg = make_config(AuthMode::ChatGPT);
        assert!(should_show_login_screen(
            LoginStatus::AuthMode(AuthMode::ApiKey),
            &cfg
        ))
    }

    #[test]
    fn hides_login_when_api_key_and_prefers_api_key() {
        let cfg = make_config(AuthMode::ApiKey);
        assert!(!should_show_login_screen(
            LoginStatus::AuthMode(AuthMode::ApiKey),
            &cfg
        ))
    }

    #[test]
    fn hides_login_when_chatgpt_and_prefers_chatgpt() {
        let cfg = make_config(AuthMode::ChatGPT);
        assert!(!should_show_login_screen(
            LoginStatus::AuthMode(AuthMode::ChatGPT),
            &cfg
        ))
    }
}
