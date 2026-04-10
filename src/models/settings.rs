use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteSettings {
    pub site_name: String,
    pub registration_open: bool,
    pub require_invite_code: bool,
    pub default_role: String,
    pub welcome_message: String,
    pub max_message_length: i64,
    pub motd: String,
    pub maintenance_mode: bool,
    pub rate_limit_messages: i64,
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_user: String,
    pub smtp_pass: String,
}
