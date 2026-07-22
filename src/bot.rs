use std::time::{Duration, Instant};

use frankenstein::client_ureq::Bot;
use frankenstein::methods::{GetUpdatesParams, SendMessageParams};
use frankenstein::types::AllowedUpdate;
use frankenstein::updates::UpdateContent;
use frankenstein::TelegramApi;
use log::{debug, error, info, warn};

pub struct TgBot {
    bot: Bot,
    admin_id: i64,
    offset: Option<i64>,
    backoff_until: Option<Instant>,
    consecutive_errors: u32,
}

impl TgBot {
    pub fn new(token: &str, admin_id: i64) -> Self {
        let request_config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(35)))
            .http_status_as_error(false)
            .max_redirects(2)
            .build();
        let mut bot = Bot::new(token);
        bot.request_agent = ureq::Agent::new_with_config(request_config);
        Self {
            bot,
            admin_id,
            offset: None,
            backoff_until: None,
            consecutive_errors: 0,
        }
    }

    pub fn admin_id(&self) -> i64 {
        self.admin_id
    }

    pub fn send_notification(&mut self, text: &str) -> bool {
        for part in message_parts(text, 4_000) {
            if !self.send_one(part) {
                return false;
            }
        }
        true
    }

    fn send_one(&mut self, text: &str) -> bool {
        if self.in_backoff() {
            warn!("Telegram backoff active; notification skipped");
            return false;
        }
        let params = SendMessageParams::builder()
            .chat_id(self.admin_id)
            .text(text)
            .build();
        match self.bot.send_message(&params) {
            Ok(_) => {
                info!("Telegram notification sent");
                self.clear_backoff();
                true
            }
            Err(_) => {
                error!("Telegram sendMessage failed");
                self.record_failure();
                false
            }
        }
    }

    pub fn poll_commands(&mut self, timeout_secs: u32) -> Vec<String> {
        if self.in_backoff() {
            std::thread::sleep(Duration::from_millis(200));
            return Vec::new();
        }
        let mut params = GetUpdatesParams::builder()
            .timeout(timeout_secs)
            .limit(20)
            .allowed_updates(vec![AllowedUpdate::Message])
            .build();
        if let Some(offset) = self.offset {
            params.offset = Some(offset);
        }
        let result = match self.bot.get_updates(&params) {
            Ok(result) => {
                self.clear_backoff();
                result
            }
            Err(_) => {
                error!("Telegram getUpdates failed");
                self.record_failure();
                return Vec::new();
            }
        };

        let mut commands = Vec::new();
        for update in result.result {
            self.offset = Some(i64::from(update.update_id) + 1);
            let UpdateContent::Message(message) = update.content else {
                continue;
            };
            if message.chat.id != self.admin_id {
                debug!("Ignoring Telegram update from unauthorized chat");
                continue;
            }
            if let Some(command) =
                parse_admin_command(self.admin_id, message.chat.id, message.text.as_deref())
            {
                commands.push(command);
            }
        }
        commands
    }

    fn in_backoff(&self) -> bool {
        self.backoff_until
            .is_some_and(|deadline| Instant::now() < deadline)
    }

    fn clear_backoff(&mut self) {
        self.consecutive_errors = 0;
        self.backoff_until = None;
    }

    fn record_failure(&mut self) {
        self.consecutive_errors = self.consecutive_errors.saturating_add(1);
        let seconds = 1u64 << self.consecutive_errors.min(6);
        self.backoff_until = Some(Instant::now() + Duration::from_secs(seconds));
    }
}

fn parse_admin_command(admin_id: i64, chat_id: i64, text: Option<&str>) -> Option<String> {
    if chat_id != admin_id {
        return None;
    }
    let text = text?.trim();
    let command = text.strip_prefix('/')?;
    let command = command
        .split_whitespace()
        .next()?
        .split('@')
        .next()?
        .to_ascii_lowercase();
    (!command.is_empty()).then_some(command)
}

fn message_parts(text: &str, max_chars: usize) -> Vec<&str> {
    if text.chars().count() <= max_chars {
        return vec![text];
    }
    let mut parts = Vec::new();
    let mut start = 0;
    let mut count = 0;
    for (index, _) in text.char_indices() {
        count += 1;
        if count > max_chars {
            parts.push(&text[start..index]);
            start = index;
            count = 1;
        }
    }
    parts.push(&text[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_admin_commands_are_accepted() {
        assert_eq!(
            parse_admin_command(7, 7, Some(" /Stats@my_bot now ")),
            Some("stats".into())
        );
        assert_eq!(parse_admin_command(7, 8, Some("/stats")), None);
        assert_eq!(parse_admin_command(7, 7, Some("hello")), None);
    }

    #[test]
    fn long_messages_split_on_utf8_boundaries() {
        let text = "🙂".repeat(9);
        let parts = message_parts(&text, 4);
        assert_eq!(
            parts
                .iter()
                .map(|part| part.chars().count())
                .collect::<Vec<_>>(),
            vec![4, 4, 1]
        );
    }
}
