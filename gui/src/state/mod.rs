#![allow(dead_code)]

use std::rc::Rc;

use yew::Reducible;

use crate::api::chat::{ChatMessage, SkillCallInfo};
use crate::api::sessions::Session;
use crate::api::skills::Skill;

#[derive(Debug, Clone, PartialEq)]
pub enum View {
    Chat,
    Skills,
    Knowledge,
    Settings,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamStatus {
    Idle,
    Streaming,
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub current_view: View,
    pub current_session_id: Option<String>,
    pub sessions: Vec<Session>,
    pub messages: Vec<ChatMessage>,
    pub stream_status: StreamStatus,
    pub is_sidebar_open: bool,
    pub settings: Settings,
    pub skills: Vec<Skill>,
    pub current_skill_id: Option<String>,
    pub toast: Option<(String, ToastType)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub api_base_url: String,
    pub theme: Theme,
    pub font_size: FontSize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Theme {
    Light,
    Dark,
    System,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FontSize {
    Small,
    Medium,
    Large,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            current_view: View::Chat,
            current_session_id: None,
            sessions: Vec::new(),
            messages: Vec::new(),
            stream_status: StreamStatus::Idle,
            is_sidebar_open: true,
            settings: Settings::default(),
            skills: Vec::new(),
            current_skill_id: None,
            toast: None,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_base_url: "http://localhost:3000".to_string(),
            theme: Theme::System,
            font_size: FontSize::Medium,
        }
    }
}

pub enum AppAction {
    SetView(View),
    SetCurrentSession(Option<String>),
    SetSessions(Vec<Session>),
    AddSession(Session),
    RemoveSession(String),
    SetMessages(Vec<ChatMessage>),
    AddMessage(ChatMessage),
    UpdateLastMessage(String),
    AppendSkillCalls(Vec<SkillCallInfo>),
    SetStreamStatus(StreamStatus),
    ToggleSidebar,
    SetSidebarOpen(bool),
    UpdateSettings(Settings),
    ClearMessages,
    SetSkills(Vec<Skill>),
    SetCurrentSkill(Option<String>),
    RegenerateFrom(usize),
    EditMessage { index: usize, new_content: String },
    DeleteMessagesFrom(usize),
    ShowToast { message: String, toast_type: ToastType },
    HideToast,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToastType {
    Error,
    Success,
    Info,
}

impl Reducible for AppState {
    type Action = AppAction;

    fn reduce(self: Rc<Self>, action: Self::Action) -> Rc<Self> {
        let mut new_state = (*self).clone();

        match action {
            AppAction::SetView(view) => {
                new_state.current_view = view;
            }
            AppAction::SetCurrentSession(session_id) => {
                new_state.current_session_id = session_id;
            }
            AppAction::SetSessions(sessions) => {
                new_state.sessions = sessions;
            }
            AppAction::AddSession(session) => {
                new_state.sessions.push(session);
            }
            AppAction::RemoveSession(session_id) => {
                new_state.sessions.retain(|s| s.id != session_id);
                if new_state.current_session_id.as_ref() == Some(&session_id) {
                    new_state.current_session_id = None;
                    new_state.messages.clear();
                }
            }
            AppAction::SetMessages(messages) => {
                new_state.messages = messages;
            }
            AppAction::AddMessage(message) => {
                new_state.messages.push(message);
            }
            AppAction::UpdateLastMessage(content) => {
                if let Some(last) = new_state.messages.last_mut() {
                    last.content.push_str(&content);
                }
            }
            AppAction::AppendSkillCalls(calls) => {
                // 将 skill_calls 附加到最后一条助手消息
                if let Some(last) = new_state.messages.last_mut() {
                    if matches!(last.role, crate::api::chat::MessageRole::Assistant) {
                        last.skill_calls = Some(calls);
                    }
                }
            }
            AppAction::SetStreamStatus(status) => {
                new_state.stream_status = status;
            }
            AppAction::ToggleSidebar => {
                new_state.is_sidebar_open = !new_state.is_sidebar_open;
            }
            AppAction::SetSidebarOpen(open) => {
                new_state.is_sidebar_open = open;
            }
            AppAction::UpdateSettings(settings) => {
                new_state.settings = settings;
            }
            AppAction::ClearMessages => {
                new_state.messages.clear();
            }
            AppAction::SetSkills(skills) => {
                new_state.skills = skills;
            }
            AppAction::SetCurrentSkill(skill_id) => {
                new_state.current_skill_id = skill_id;
            }
            AppAction::RegenerateFrom(index) => {
                // 删除从指定索引+1开始的所有消息（保留到该用户消息为止）
                if index < new_state.messages.len() {
                    new_state.messages.truncate(index + 1);
                }
                // 重置流状态
                new_state.stream_status = StreamStatus::Idle;
            }
            AppAction::EditMessage { index, new_content } => {
                if let Some(msg) = new_state.messages.get_mut(index) {
                    msg.content = new_content;
                }
            }
            AppAction::DeleteMessagesFrom(index) => {
                if index < new_state.messages.len() {
                    new_state.messages.truncate(index);
                }
            }
            AppAction::ShowToast { message, toast_type } => {
                new_state.toast = Some((message, toast_type));
            }
            AppAction::HideToast => {
                new_state.toast = None;
            }
        }

        new_state.into()
    }
}
