use std::env;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub telegram_bot_token: String,
    pub telegram_admin_id: i64,
    pub message_check_interval: Duration,
    pub message_check_min: Duration,
    pub message_check_max: Duration,
    pub stats_check_interval: Duration,
    pub orders_check_interval: Duration,
    pub summary_interval: Duration,
    pub state_path: PathBuf,
    pub kwork_login: String,
    pub kwork_password: String,
    pub token_path: PathBuf,
    /// Local hour [0,23] inclusive start of quiet window (notifications suppressed except unread msgs if allow).
    pub quiet_start: Option<u8>,
    pub quiet_end: Option<u8>,
    /// If true, still deliver inbox unread during quiet hours.
    pub quiet_allow_inbox: bool,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let _ = dotenvy::dotenv();

        let telegram_bot_token = required("TELEGRAM_BOT_TOKEN")?;
        let telegram_admin_id = required("TELEGRAM_ADMIN_ID")?
            .parse()
            .map_err(|_| "TELEGRAM_ADMIN_ID must be an integer".to_string())?;
        if telegram_admin_id == 0 {
            return Err("TELEGRAM_ADMIN_ID cannot be zero".into());
        }

        let message_mins = parse_positive("MESSAGE_CHECK_INTERVAL", 3, 1_440)?;
        let message_min = parse_positive("MESSAGE_CHECK_MIN", 1, 1_440)?;
        let message_max = parse_positive("MESSAGE_CHECK_MAX", 10, 1_440)?;
        if !(message_min <= message_mins && message_mins <= message_max) {
            return Err("inbox intervals must satisfy MIN <= INTERVAL <= MAX".into());
        }
        let stats_mins = parse_positive("STATS_CHECK_INTERVAL", 60, 10_080)?;
        let orders_mins = parse_positive("ORDERS_CHECK_INTERVAL", 5, 10_080)?;
        let summary_hours = parse_positive("SUMMARY_INTERVAL_HOURS", 6, 168)?;

        let kwork_login = required("KWORK_LOGIN")?;
        let kwork_password = required("KWORK_PASSWORD")?;

        let (quiet_start, quiet_end) =
            parse_quiet_hours(&env::var("QUIET_HOURS").unwrap_or_else(|_| "".into()))?;
        let quiet_allow_inbox = match env::var("QUIET_ALLOW_INBOX") {
            Ok(value) if value == "1" || value.eq_ignore_ascii_case("true") => true,
            Ok(value) if value == "0" || value.eq_ignore_ascii_case("false") => false,
            Ok(value) => {
                return Err(format!(
                    "QUIET_ALLOW_INBOX must be true/false or 1/0, got {value:?}"
                ))
            }
            Err(_) => true,
        };

        Ok(Self {
            telegram_bot_token,
            telegram_admin_id,
            message_check_interval: Duration::from_secs(message_mins.saturating_mul(60)),
            message_check_min: Duration::from_secs(message_min.saturating_mul(60)),
            message_check_max: Duration::from_secs(message_max.saturating_mul(60)),
            stats_check_interval: Duration::from_secs(stats_mins.saturating_mul(60)),
            orders_check_interval: Duration::from_secs(orders_mins.saturating_mul(60)),
            summary_interval: Duration::from_secs(summary_hours.saturating_mul(3600)),
            state_path: PathBuf::from(
                env::var("STATE_PATH").unwrap_or_else(|_| "kwork-state.json".into()),
            ),
            kwork_login,
            kwork_password,
            token_path: PathBuf::from(
                env::var("KWORK_TOKEN_PATH").unwrap_or_else(|_| ".kwork-token.json".into()),
            ),
            quiet_start,
            quiet_end,
            quiet_allow_inbox,
        })
    }
}

fn required(key: &str) -> Result<String, String> {
    let v = env::var(key).map_err(|_| format!("{key} is required"))?;
    let v = v.trim().to_string();
    if v.is_empty() {
        return Err(format!("{key} is empty"));
    }
    Ok(v)
}

fn parse_positive(key: &str, default: u64, max: u64) -> Result<u64, String> {
    let value = match env::var(key) {
        Ok(v) if !v.trim().is_empty() => v
            .trim()
            .parse()
            .map_err(|_| format!("{key} must be a positive integer, got {v:?}"))?,
        _ => default,
    };
    if value == 0 || value > max {
        return Err(format!("{key} must be between 1 and {max}"));
    }
    Ok(value)
}

/// `22-8` or `22:00-08:00` → (22, 8). Empty → no quiet hours.
fn parse_quiet_hours(raw: &str) -> Result<(Option<u8>, Option<u8>), String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok((None, None));
    }
    let parts: Vec<_> = raw.split('-').map(str::trim).collect();
    if parts.len() != 2 {
        return Err(format!("QUIET_HOURS must look like 22-8, got {raw:?}"));
    }
    let start = parse_hour(parts[0])?;
    let end = parse_hour(parts[1])?;
    Ok((Some(start), Some(end)))
}

fn parse_hour(s: &str) -> Result<u8, String> {
    let (hour, minutes) = s.split_once(':').unwrap_or((s, "00"));
    if minutes != "00" {
        return Err(format!("QUIET_HOURS supports whole hours only, got {s:?}"));
    }
    let h: u8 = hour
        .parse()
        .map_err(|_| format!("invalid hour in QUIET_HOURS: {hour:?}"))?;
    if h > 23 {
        return Err(format!("hour must be 0-23, got {h}"));
    }
    Ok(h)
}

/// Local wall-clock hour 0–23 (uses process TZ / VPS timezone).
pub fn local_hour() -> u8 {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Fallback UTC hour if we cannot read localtime — good enough when systemd sets TZ.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Try libc localtime via external: for portability use UTC offset from env TZ only.
    // Simple approach: `date +%H` is not ideal. Use chrono-free POSIX:
    #[cfg(unix)]
    {
        unsafe {
            let t = secs as libc::time_t;
            let tm = libc::localtime(&t);
            if !tm.is_null() {
                return (*tm).tm_hour as u8;
            }
        }
    }
    ((secs / 3600) % 24) as u8
}

pub fn in_quiet_hours(start: Option<u8>, end: Option<u8>) -> bool {
    let (Some(s), Some(e)) = (start, end) else {
        return false;
    };
    in_quiet_at(s, e, local_hour())
}

fn in_quiet_at(start: u8, end: u8, hour: u8) -> bool {
    if start == end {
        return false;
    }
    if start < end {
        // e.g. 1-6
        hour >= start && hour < end
    } else {
        // e.g. 22-8 overnight
        hour >= start || hour < end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_hours_cross_midnight() {
        assert_eq!(
            parse_quiet_hours("22:00-08:00").unwrap(),
            (Some(22), Some(8))
        );
        assert!(parse_quiet_hours("24-8").is_err());
        assert!(in_quiet_at(22, 8, 23));
        assert!(in_quiet_at(22, 8, 7));
        assert!(!in_quiet_at(22, 8, 12));
    }

    #[test]
    fn hour_parser_rejects_bad_values() {
        assert_eq!(parse_hour("0").unwrap(), 0);
        assert_eq!(parse_hour("23:00").unwrap(), 23);
        assert!(parse_hour("23:59").is_err());
        assert!(parse_hour("x").is_err());
    }
}
