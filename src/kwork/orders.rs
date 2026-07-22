use log::{info, warn};

use crate::state::StateStore;

use super::api::{ApiError, KworkApi};

#[derive(Debug, PartialEq, Eq)]
enum OrderChange<'a> {
    None,
    New,
    Status(&'a str),
}

/// Watch worker orders. First run seeds state without notify.
pub fn check_orders(
    api: &mut KworkApi,
    state: &mut StateStore,
    mut notify: impl FnMut(&str) -> bool,
) -> Result<usize, ApiError> {
    let orders = match api.get_worker_orders("all") {
        Ok(o) => o,
        Err(ApiError::Api(msg)) if msg.contains("ничего не заказали") => {
            state.touch_ok("orders");
            state
                .save()
                .map_err(|e| ApiError::Io(format!("save orders state: {e}")))?;
            return Ok(0);
        }
        Err(e) => return Err(e),
    };

    info!("Orders returned: {}", orders.len());
    let seeded = state.orders_seeded();

    let mut sent = 0usize;
    for o in &orders {
        let fingerprint = bounded_chars(&o.status, 80);
        let previous = state.order(o.id).map(str::to_owned);

        let change = classify_order(seeded, previous.as_deref(), &fingerprint);
        let should_count = change != OrderChange::None;
        let delivered = match change {
            OrderChange::New => {
                let msg = format!(
                    "🆕 Новый заказ #{}\n\
                     {}\n\
                     статус: {}\n\
                     {}\n\
                     https://kwork.ru/track?id={}",
                    o.id,
                    o.title,
                    o.status,
                    if o.username.is_empty() {
                        String::new()
                    } else {
                        format!("покупатель: @{}", o.username)
                    },
                    o.id
                );
                notify(&msg)
            }
            OrderChange::Status(old_status) => {
                let msg = format!(
                    "📦 Заказ #{}: {} → {}\n\
                     {}\n\
                     https://kwork.ru/track?id={}",
                    o.id, old_status, o.status, o.title, o.id
                );
                notify(&msg)
            }
            OrderChange::None => true,
        };
        if delivered {
            state.set_order(o.id, fingerprint);
            if should_count {
                sent += 1;
            }
        } else {
            warn!("Telegram delivery failed; order notification will be retried");
        }
    }

    if !seeded {
        state.mark_orders_seeded();
        info!("Orders state seeded (no notifications on first run).");
    } else if sent == 0 {
        info!("No order changes.");
    }

    state.touch_ok("orders");
    state
        .save()
        .map_err(|e| ApiError::Io(format!("save orders state: {e}")))?;
    Ok(sent)
}

fn bounded_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn classify_order<'a>(seeded: bool, previous: Option<&'a str>, current: &str) -> OrderChange<'a> {
    if !seeded {
        return OrderChange::None;
    }
    match previous {
        None => OrderChange::New,
        Some(previous) if previous != current => OrderChange::Status(previous),
        Some(_) => OrderChange::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_seeds_without_notifications() {
        assert_eq!(classify_order(false, None, "active"), OrderChange::None);
        assert_eq!(classify_order(true, None, "active"), OrderChange::New);
        assert_eq!(
            classify_order(true, Some("active"), "done"),
            OrderChange::Status("active")
        );
    }
}
