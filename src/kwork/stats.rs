use log::{info, warn};

use crate::state::StateStore;

use super::api::{ApiError, KworkApi};

#[derive(Debug, PartialEq, Eq)]
enum StatChange {
    New,
    Delta { views: i64, orders: i64 },
    None,
}

pub fn check_stats(
    api: &mut KworkApi,
    state: &mut StateStore,
    mut notify: impl FnMut(&str) -> bool,
) -> Result<usize, ApiError> {
    let (rows, complete) = api.get_my_kworks_snapshot()?;
    info!("Found {} kwork(s).", rows.len());
    if rows.is_empty() {
        warn!("No kworks returned from API.");
        if complete {
            state.retain_kworks(&[]);
        }
        state.touch_ok("stats");
        state
            .save()
            .map_err(|e| ApiError::Io(format!("save stats state: {e}")))?;
        return Ok(0);
    }

    let mut deltas: Vec<String> = Vec::new();
    let mut fresh: Vec<String> = Vec::new();

    for entry in &rows {
        let previous = state.kwork(entry.id).cloned();
        match classify_stat(previous.as_ref(), entry.views, entry.orders) {
            StatChange::New => fresh.push(entry.name.clone()),
            StatChange::Delta {
                views: v_delta,
                orders: o_delta,
            } => {
                let mut parts = vec![format!("📊 {}", entry.name)];
                if v_delta > 0 {
                    parts.push(format!("👁 Просмотры: +{v_delta} (сейчас {})", entry.views));
                }
                if o_delta > 0 {
                    parts.push(format!("📦 Заказы: +{o_delta} (сейчас {})", entry.orders));
                }
                parts.push(format!("https://kwork.ru/{}", entry.id));
                deltas.push(parts.join("\n"));
            }
            StatChange::None => {}
        }
    }

    let mut messages: Vec<String> = Vec::new();
    if !deltas.is_empty() {
        messages.push(format!(
            "📈 Статистика за период:\n\n{}",
            deltas.join("\n\n")
        ));
    }
    if !fresh.is_empty() {
        let list = fresh
            .iter()
            .map(|n| format!("• {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        messages.push(format!("🆕 Новые кворки:\n{list}"));
    }

    let sent = if messages.is_empty() {
        info!("No stats changes detected.");
        0
    } else {
        let text = messages.join("\n\n");
        info!("Stats report:\n{text}");
        if !notify(&text) {
            return Err(ApiError::Io(
                "Telegram delivery failed; stats notification will be retried".into(),
            ));
        }
        1
    };
    if complete {
        let active_ids: Vec<_> = rows.iter().map(|entry| entry.id).collect();
        state.retain_kworks(&active_ids);
    } else {
        warn!("Kwork snapshot may be partial; stale entries were retained");
    }
    for entry in rows {
        state.set_kwork(entry.id, entry.name, entry.views, entry.orders);
    }
    state.touch_ok("stats");
    state
        .save()
        .map_err(|e| ApiError::Io(format!("save stats state: {e}")))?;
    Ok(sent)
}

fn classify_stat(
    previous: Option<&crate::state::KworkSnapshot>,
    views: i64,
    orders: i64,
) -> StatChange {
    let Some(previous) = previous else {
        return StatChange::New;
    };
    let view_delta = views - previous.views;
    let order_delta = orders - previous.orders;
    if view_delta > 0 || order_delta > 0 {
        StatChange::Delta {
            views: view_delta.max(0),
            orders: order_delta.max(0),
        }
    } else {
        StatChange::None
    }
}

pub fn build_summary(state: &StateStore) -> String {
    let mut rows: Vec<_> = state.kworks().collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    if rows.is_empty() {
        return "📊 Сводка: пока нет данных по кворкам.".into();
    }

    let mut total_views = 0i64;
    let mut total_orders = 0i64;
    let mut lines: Vec<String> = Vec::new();

    for row in &rows {
        total_views += row.views;
        total_orders += row.orders;
        lines.push(format!(
            "• {} — 👁 {}  📦 {}",
            row.name, row.views, row.orders
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "Итого: {} кворков, 👁 {total_views}, 📦 {total_orders}",
        rows.len()
    ));
    lines.join("\n")
}

pub fn build_digest(api: &mut KworkApi, state: &StateStore) -> Result<String, ApiError> {
    let actor = api.get_actor()?;
    let connects = api
        .get_connects()
        .map(|(active, all)| format!("{active} / {all}"))
        .unwrap_or_else(|_| "недоступно".into());
    let kwork_part = build_summary(state);

    Ok(format!(
        "📋 Дайджест Kwork\n\
         @{}\n\
         💰 Баланс: {:.0}₽ (холд {:.0}₽)\n\
         ⭐ Рейтинг: {:.2} (+{} / −{})\n\
         📦 Заказов выполнено: {}\n\
         🔗 Коннекты: {connects}\n\
         📩 Непрочитано: {} диал. / {} сообщ.\n\
         🛍 Кворков: {}\n\n\
         {}",
        actor.username,
        actor.free_amount,
        actor.hold_amount,
        actor.rating,
        actor.good_reviews,
        actor.bad_reviews,
        actor.completed_orders_count,
        actor.unread_dialog_count,
        actor.unread_messages_count,
        actor.kworks_count,
        kwork_part
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::KworkSnapshot;

    #[test]
    fn reports_only_positive_deltas() {
        let previous = KworkSnapshot {
            name: "old name".into(),
            views: 10,
            orders: 2,
            updated_at: 0,
        };
        assert_eq!(
            classify_stat(Some(&previous), 12, 2),
            StatChange::Delta {
                views: 2,
                orders: 0
            }
        );
        assert_eq!(classify_stat(Some(&previous), 9, 1), StatChange::None);
        assert_eq!(classify_stat(None, 1, 0), StatChange::New);
    }
}
