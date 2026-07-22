use log::{info, warn};

use crate::state::StateStore;
use crate::util::fingerprint;

use super::api::{ApiError, KworkApi};

pub fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(2048));
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&mdash;", "—")
        .replace("&middot;", "·");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Returns number of notifications sent (activity signal for adaptive polling).
pub fn check_inbox(
    api: &mut KworkApi,
    state: &mut StateStore,
    mut notify: impl FnMut(&str) -> bool,
) -> Result<usize, ApiError> {
    let dialogs = api.get_dialogs()?;
    info!("Found {} dialog(s).", dialogs.len());

    let mut sent = 0usize;
    for d in &dialogs {
        let text_full = strip_html(&d.last_message);
        let text_short: String = text_full.chars().take(400).collect();
        let username: String = d.username.chars().take(80).collect();
        let fingerprint = fingerprint(&format!(
            "{}|{}|{}|{}",
            d.time, d.unread_count, username, text_short
        ));
        let unchanged = state.dialog(d.user_id) == Some(fingerprint.as_str());

        if !should_notify(d.unread, unchanged) {
            state.set_dialog(d.user_id, fingerprint);
            continue;
        }

        let sender = if d.username.is_empty() {
            format!("id:{}", d.user_id)
        } else {
            format!("@{}", d.username)
        };
        let preview = if text_short.is_empty() {
            "(нет текста)".to_string()
        } else {
            text_short
        };

        let msg = format!(
            "📩 Новое сообщение от {sender}\n\
             непрочитано: {}\n\
             {preview}\n\
             {}",
            d.unread_count.max(1),
            d.link
        );
        info!("Notify inbox {sender}");
        if notify(&msg) {
            state.set_dialog(d.user_id, fingerprint);
            sent += 1;
        } else {
            warn!("Telegram delivery failed; inbox notification will be retried");
        }
    }

    if sent == 0 {
        info!("No new unread dialogs to notify.");
    }
    state.touch_ok("inbox");
    state
        .save()
        .map_err(|e| ApiError::Io(format!("save inbox state: {e}")))?;
    Ok(sent)
}

fn should_notify(unread: bool, unchanged: bool) -> bool {
    unread && !unchanged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_basic() {
        assert_eq!(strip_html("<p>hi &amp; bye</p>"), "hi & bye");
    }

    #[test]
    fn unread_changes_notify_once() {
        assert!(should_notify(true, false));
        assert!(!should_notify(true, true));
        assert!(!should_notify(false, false));
    }
}
