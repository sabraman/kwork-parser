pub const COMMAND_HELP: &str = "👋 Kwork Parser\n\n\
     /inbox — проверить сообщения\n\
     /stats — обновить просмотры и заказы кворков\n\
     /orders — проверить заказы\n\
     /summary — последняя сохранённая сводка по кворкам\n\
     /status — состояние и последние проверки";

pub fn format_ago(unix: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if unix == 0 {
        return "никогда".into();
    }
    let d = now.saturating_sub(unix);
    if d < 60 {
        format!("{d} сек. назад")
    } else if d < 3600 {
        format!("{} мин. назад", d / 60)
    } else if d < 86400 {
        format!("{} ч. назад", d / 3600)
    } else {
        format!("{} дн. назад", d / 86400)
    }
}

pub fn russian_count(count: usize, one: &str, few: &str, many: &str) -> String {
    format!("{count} {}", russian_form(count, one, few, many))
}

fn russian_form<'a>(count: usize, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let last_two = count % 100;
    let last = count % 10;
    if (11..=14).contains(&last_two) {
        many
    } else {
        match last {
            1 => one,
            2..=4 => few,
            _ => many,
        }
    }
}

pub fn bool_ru(value: bool) -> &'static str {
    if value {
        "да"
    } else {
        "нет"
    }
}

pub fn job_label(job: &str) -> &str {
    match job {
        "inbox" => "входящие сообщения",
        "orders" => "заказы",
        "stats" => "статистика",
        "digest" => "дайджест",
        _ => "задача",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_zero_is_russian() {
        assert_eq!(format_ago(0), "никогда");
    }

    #[test]
    fn count_forms_cover_russian_rules() {
        assert_eq!(
            russian_count(1, "уведомление", "уведомления", "уведомлений"),
            "1 уведомление"
        );
        assert_eq!(
            russian_count(2, "уведомление", "уведомления", "уведомлений"),
            "2 уведомления"
        );
        assert_eq!(
            russian_count(5, "уведомление", "уведомления", "уведомлений"),
            "5 уведомлений"
        );
        assert_eq!(
            russian_count(11, "уведомление", "уведомления", "уведомлений"),
            "11 уведомлений"
        );
        assert_eq!(
            russian_count(21, "уведомление", "уведомления", "уведомлений"),
            "21 уведомление"
        );
    }

    #[test]
    fn command_help_is_russian() {
        assert!(COMMAND_HELP.contains("проверить сообщения"));
        assert!(!COMMAND_HELP.contains("check"));
        assert!(!COMMAND_HELP.contains("latest saved"));
    }

    #[test]
    fn status_values_are_russian() {
        assert_eq!(bool_ru(true), "да");
        assert_eq!(bool_ru(false), "нет");
        assert_eq!(job_label("orders"), "заказы");
    }
}
