mod bot;
mod config;
mod kwork;
mod state;
mod text;
mod util;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use log::{error, info, LevelFilter, Log, Metadata, Record};

use bot::TgBot;
use config::{in_quiet_hours, Config};
use kwork::{
    build_digest, build_summary, check_inbox, check_orders, check_stats, ApiError, KworkApi,
};
use state::StateStore;
use text::{bool_ru, format_ago, job_label, russian_count, COMMAND_HELP};

static RUNNING: AtomicBool = AtomicBool::new(true);
static LOGGER: SimpleLogger = SimpleLogger;

fn main() {
    init_logging();
    if std::env::args().any(|argument| argument == "--check" || argument == "-c") {
        std::process::exit(run_check());
    }

    let config = Config::from_env().unwrap_or_else(|error| exit_error("config", &error));
    let mut state =
        StateStore::load(&config.state_path).unwrap_or_else(|error| exit_error("state", &error));
    info!("State ready at {}", state.path().display());

    let mut telegram = TgBot::new(&config.telegram_bot_token, config.telegram_admin_id);
    install_signal_handlers();
    let mut api = match KworkApi::connect(
        config.kwork_login.clone(),
        config.kwork_password.clone(),
        config.token_path.clone(),
    ) {
        Ok(api) => Some(api),
        Err(error) => {
            telegram
                .send_notification(&format!("⚠️ Ошибка авторизации Kwork при запуске: {error}"));
            error!("Initial Kwork connection failed: {error}");
            state.touch_error(&format!("startup: {error}"));
            save_or_log(&mut state);
            None
        }
    };

    telegram.send_notification(&format!(
        "✅ Kwork Parser запущен\n\
         📩 входящие: адаптивный интервал {}–{} мин. (база {} мин.)\n\
         📊 статистика: интервал {} мин.\n\
         📦 заказы: интервал {} мин.\n\
         📋 дайджест: интервал {} ч.\n\
         Команды: /stats /inbox /orders /summary /status /start",
        config.message_check_min.as_secs() / 60,
        config.message_check_max.as_secs() / 60,
        config.message_check_interval.as_secs() / 60,
        config.stats_check_interval.as_secs() / 60,
        config.orders_check_interval.as_secs() / 60,
        config.summary_interval.as_secs() / 3_600,
    ));
    let mut initial_digest_delivered = true;
    if let Some(connected) = api.as_mut() {
        let (connection_valid, digest_delivered) =
            run_initial_jobs(connected, &mut state, &mut telegram, &config);
        initial_digest_delivered = digest_delivered;
        if !connection_valid {
            api = None;
        }
    }

    let mut next_inbox = Instant::now() + config.message_check_interval;
    let mut next_stats = Instant::now() + config.stats_check_interval;
    let mut next_orders = Instant::now() + config.orders_check_interval;
    let mut next_digest = Instant::now()
        + if initial_digest_delivered {
            config.summary_interval
        } else {
            Duration::from_secs(300)
        };
    let mut last_activity = Instant::now();
    let mut next_reconnect = Instant::now() + Duration::from_secs(2);
    let mut reconnect_delay = Duration::from_secs(2);

    info!("Main loop started (blocking, single-threaded)");
    while RUNNING.load(Ordering::Relaxed) {
        for command in telegram.poll_commands(20) {
            handle_command(&command, api.as_mut(), &mut state, &mut telegram, &config);
        }
        if !RUNNING.load(Ordering::Relaxed) {
            break;
        }

        let quiet = in_quiet_hours(config.quiet_start, config.quiet_end);
        let now = Instant::now();
        if api.is_none() && now >= next_reconnect {
            match KworkApi::connect(
                config.kwork_login.clone(),
                config.kwork_password.clone(),
                config.token_path.clone(),
            ) {
                Ok(mut connected) => {
                    let (connection_valid, digest_delivered) =
                        run_initial_jobs(&mut connected, &mut state, &mut telegram, &config);
                    if connection_valid {
                        info!("Kwork connection restored");
                        telegram.send_notification("✅ Соединение с Kwork восстановлено.");
                        next_digest = Instant::now()
                            + if digest_delivered {
                                config.summary_interval
                            } else {
                                Duration::from_secs(300)
                            };
                        api = Some(connected);
                        reconnect_delay = Duration::from_secs(2);
                    } else {
                        reconnect_delay = next_reconnect_delay(reconnect_delay);
                        next_reconnect = Instant::now() + reconnect_delay;
                    }
                }
                Err(error) => {
                    error!("Kwork reconnect failed: {error}");
                    state.touch_error(&format!("reconnect: {error}"));
                    save_or_log(&mut state);
                    reconnect_delay = next_reconnect_delay(reconnect_delay);
                    next_reconnect = Instant::now() + reconnect_delay;
                }
            }
        }
        let Some(api_client) = api.as_mut() else {
            continue;
        };
        let mut disconnect = false;
        if now >= next_inbox {
            match check_inbox(api_client, &mut state, |text| {
                if !quiet || config.quiet_allow_inbox {
                    telegram.send_notification(text)
                } else {
                    true
                }
            }) {
                Ok(count) if count > 0 => last_activity = Instant::now(),
                Ok(_) => {}
                Err(error) => {
                    disconnect |= on_error(&mut state, &mut telegram, "inbox", &error);
                }
            }
            let interval = adaptive_inbox_interval(&config, last_activity.elapsed());
            next_inbox = Instant::now() + interval;
        }
        if disconnect {
            api = None;
            reconnect_delay = Duration::from_secs(2);
            next_reconnect = Instant::now() + reconnect_delay;
            continue;
        }
        if now >= next_orders {
            match check_orders(api_client, &mut state, |text| {
                if !quiet {
                    telegram.send_notification(text)
                } else {
                    true
                }
            }) {
                Ok(count) if count > 0 => last_activity = Instant::now(),
                Ok(_) => {}
                Err(error) => {
                    disconnect |= on_error(&mut state, &mut telegram, "orders", &error);
                }
            }
            next_orders = Instant::now() + config.orders_check_interval;
        }
        if disconnect {
            api = None;
            reconnect_delay = Duration::from_secs(2);
            next_reconnect = Instant::now() + reconnect_delay;
            continue;
        }
        if now >= next_stats {
            if let Err(error) = check_stats(api_client, &mut state, |text| {
                if !quiet {
                    telegram.send_notification(text)
                } else {
                    true
                }
            }) {
                disconnect |= on_error(&mut state, &mut telegram, "stats", &error);
            }
            next_stats = Instant::now() + config.stats_check_interval;
        }
        if disconnect {
            api = None;
            reconnect_delay = Duration::from_secs(2);
            next_reconnect = Instant::now() + reconnect_delay;
            continue;
        }
        if now >= next_digest {
            if quiet {
                info!("Quiet hours: scheduled digest suppressed");
            } else {
                match build_digest(api_client, &state) {
                    Ok(text) => {
                        if telegram.send_notification(&text) {
                            state.touch_ok("digest");
                            save_or_log(&mut state);
                            next_digest = Instant::now() + config.summary_interval;
                        } else {
                            state.touch_error("digest: Telegram delivery failed; retry scheduled");
                            save_or_log(&mut state);
                            next_digest = Instant::now() + Duration::from_secs(300);
                        }
                    }
                    Err(error) => {
                        disconnect |= on_error(&mut state, &mut telegram, "digest", &error);
                        next_digest = Instant::now() + Duration::from_secs(300);
                    }
                }
            }
            if quiet {
                next_digest = Instant::now() + config.summary_interval;
            }
        }
        if disconnect {
            api = None;
            reconnect_delay = Duration::from_secs(2);
            next_reconnect = Instant::now() + reconnect_delay;
        }
    }
    save_or_log(&mut state);
    info!("Shutdown complete");
}

fn handle_command(
    command: &str,
    api: Option<&mut KworkApi>,
    state: &mut StateStore,
    telegram: &mut TgBot,
    config: &Config,
) {
    if matches!(command, "inbox" | "orders" | "stats") && api.is_none() {
        telegram.send_notification("⚠️ Kwork отключён; повторное подключение выполняется в фоне.");
        return;
    }
    match command {
        "start" => {
            telegram.send_notification(COMMAND_HELP);
        }
        "inbox" => {
            telegram.send_notification("⏳ Проверяю входящие сообщения…");
            match check_inbox(api.expect("checked above"), state, |text| {
                telegram.send_notification(text)
            }) {
                Ok(0) => {
                    telegram.send_notification("✅ Новых непрочитанных сообщений нет.");
                }
                Ok(count) => {
                    telegram.send_notification(&format!(
                        "✅ Отправлено: {}.",
                        russian_count(count, "уведомление", "уведомления", "уведомлений")
                    ));
                }
                Err(error) => command_error(state, telegram, "inbox", &error),
            }
        }
        "orders" => {
            telegram.send_notification("⏳ Проверяю заказы…");
            match check_orders(api.expect("checked above"), state, |text| {
                telegram.send_notification(text)
            }) {
                Ok(0) => {
                    telegram.send_notification("✅ Изменений в заказах нет.");
                }
                Ok(count) => {
                    telegram.send_notification(&format!(
                        "✅ {}.",
                        russian_count(
                            count,
                            "изменение заказа",
                            "изменения заказа",
                            "изменений заказов"
                        )
                    ));
                }
                Err(error) => command_error(state, telegram, "orders", &error),
            }
        }
        "stats" => {
            telegram.send_notification("⏳ Загружаю статистику Kwork…");
            match check_stats(api.expect("checked above"), state, |text| {
                telegram.send_notification(text)
            }) {
                Ok(_) => {
                    telegram.send_notification(&build_summary(state));
                }
                Err(error) => command_error(state, telegram, "stats", &error),
            }
        }
        "summary" => {
            telegram.send_notification(&build_summary(state));
        }
        "status" => {
            telegram.send_notification(&status_text(api, state, telegram, config));
        }
        _ => {}
    }
}

fn status_text(
    api: Option<&mut KworkApi>,
    state: &StateStore,
    telegram: &TgBot,
    config: &Config,
) -> String {
    let last = |job: &str| {
        state
            .last_ok(job)
            .map(format_ago)
            .unwrap_or_else(|| "никогда".into())
    };
    let error = state
        .last_error()
        .map(|record| record.message.as_str())
        .unwrap_or("—");
    let token_ttl = api
        .map(|api| format!("~{} ч.", api.token_expires_in_secs().max(0) / 3_600))
        .unwrap_or_else(|| "нет подключения".into());
    format!(
        "🩺 Состояние\n\
         чат администратора: {}\n\
         тихие часы сейчас: {}\n\
         срок действия токена: {token_ttl}\n\
         файл состояния: {}\n\
         входящие: {}\n\
         статистика: {}\n\
         заказы: {}\n\
         дайджест: {}\n\
         последняя ошибка: {error}",
        telegram.admin_id(),
        bool_ru(in_quiet_hours(config.quiet_start, config.quiet_end)),
        state.path().display(),
        last("inbox"),
        last("stats"),
        last("orders"),
        last("digest"),
    )
}

fn run_initial_jobs(
    api: &mut KworkApi,
    state: &mut StateStore,
    telegram: &mut TgBot,
    config: &Config,
) -> (bool, bool) {
    let quiet = in_quiet_hours(config.quiet_start, config.quiet_end);
    let mut connection_valid = true;
    if let Err(error) = check_inbox(api, state, |text| {
        if !quiet || config.quiet_allow_inbox {
            telegram.send_notification(text)
        } else {
            true
        }
    }) {
        if on_error(state, telegram, "inbox", &error) {
            return (false, true);
        }
    }
    if let Err(error) = check_orders(api, state, |text| {
        if !quiet {
            telegram.send_notification(text)
        } else {
            true
        }
    }) {
        if on_error(state, telegram, "orders", &error) {
            return (false, true);
        }
    }
    if let Err(error) = check_stats(api, state, |text| {
        if !quiet {
            telegram.send_notification(text)
        } else {
            true
        }
    }) {
        if on_error(state, telegram, "stats", &error) {
            return (false, true);
        }
    }
    let mut digest_delivered = true;
    if !quiet {
        match build_digest(api, state) {
            Ok(text) => {
                digest_delivered = telegram.send_notification(&text);
                if digest_delivered {
                    state.touch_ok("digest");
                } else {
                    state.touch_error("digest: Telegram delivery failed; retry scheduled");
                }
                save_or_log(state);
            }
            Err(error) => {
                connection_valid &= !on_error(state, telegram, "digest", &error);
            }
        }
    }
    (connection_valid, digest_delivered)
}

fn adaptive_inbox_interval(config: &Config, idle: Duration) -> Duration {
    if idle < Duration::from_secs(15 * 60) {
        config.message_check_min
    } else if idle > Duration::from_secs(2 * 3_600) {
        config.message_check_max
    } else {
        config.message_check_interval
    }
}

fn next_reconnect_delay(current: Duration) -> Duration {
    (current * 2).min(Duration::from_secs(300))
}

fn command_error(state: &mut StateStore, telegram: &mut TgBot, job: &str, error: &ApiError) {
    on_error(state, telegram, job, error);
    let label = job_label(job);
    telegram.send_notification(&format!("⚠️ Ошибка ({label}): {error}"));
}

fn on_error(state: &mut StateStore, telegram: &mut TgBot, job: &str, error: &ApiError) -> bool {
    error!("{job} failed: {error}");
    state.touch_error(&format!("{job}: {error}"));
    save_or_log(state);
    if matches!(error, ApiError::Auth(_)) {
        let label = job_label(job);
        telegram.send_notification(&format!("⚠️ Ошибка авторизации Kwork ({label}): {error}"));
    }
    matches!(error, ApiError::Auth(_) | ApiError::Http(_))
}

fn save_or_log(state: &mut StateStore) {
    if let Err(error) = state.save() {
        error!("Could not save state: {error}");
    }
}

fn run_check() -> i32 {
    let _ = dotenvy::dotenv();
    let login = std::env::var("KWORK_LOGIN").unwrap_or_default();
    let password = std::env::var("KWORK_PASSWORD").unwrap_or_default();
    let token_path =
        std::env::var("KWORK_TOKEN_PATH").unwrap_or_else(|_| ".kwork-token-check.json".into());
    if login.trim().is_empty() || password.trim().is_empty() {
        eprintln!("KWORK_LOGIN and KWORK_PASSWORD are required");
        return 1;
    }
    let mut api = match KworkApi::connect(login, password, token_path.into()) {
        Ok(api) => api,
        Err(error) => {
            eprintln!("Kwork connection failed: {error}");
            return 1;
        }
    };

    let mut failures = 0;
    report_check(
        "dialogs",
        api.get_dialogs().map(|items| items.len()),
        &mut failures,
    );
    report_check(
        "kworks",
        api.get_my_kworks().map(|items| items.len()),
        &mut failures,
    );
    report_check(
        "orders",
        api.get_worker_orders("all").map(|items| items.len()),
        &mut failures,
    );
    report_check("actor", api.get_actor().map(|_| 1), &mut failures);
    if failures == 0 {
        println!("Kwork API check passed");
        0
    } else {
        1
    }
}

fn report_check(label: &str, result: Result<usize, ApiError>, failures: &mut u8) {
    match result {
        Ok(count) => println!("{label}: {count}"),
        Err(error) => {
            eprintln!("{label}: {error}");
            *failures = failures.saturating_add(1);
        }
    }
}

fn exit_error(context: &str, error: &str) -> ! {
    eprintln!("{context} error: {error}");
    std::process::exit(1)
}

struct SimpleLogger;

impl Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{} {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

fn init_logging() {
    let level = match std::env::var("RUST_LOG")
        .unwrap_or_else(|_| "info".into())
        .to_ascii_lowercase()
        .as_str()
    {
        "off" => LevelFilter::Off,
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Info,
    };
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(level);
}

#[cfg(unix)]
fn install_signal_handlers() {
    unsafe extern "C" fn stop(_: libc::c_int) {
        RUNNING.store(false, Ordering::Relaxed);
    }
    unsafe {
        let handler = stop as *const () as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}

#[cfg(not(unix))]
fn install_signal_handlers() {
    log::warn!("Graceful signal handling is unavailable on this platform");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            telegram_bot_token: "token".into(),
            telegram_admin_id: 1,
            message_check_interval: Duration::from_secs(180),
            message_check_min: Duration::from_secs(60),
            message_check_max: Duration::from_secs(600),
            stats_check_interval: Duration::from_secs(3_600),
            orders_check_interval: Duration::from_secs(300),
            summary_interval: Duration::from_secs(21_600),
            state_path: "state.json".into(),
            kwork_login: "login".into(),
            kwork_password: "password".into(),
            token_path: "token.json".into(),
            quiet_start: None,
            quiet_end: None,
            quiet_allow_inbox: true,
        }
    }

    #[test]
    fn adaptive_intervals_are_bounded() {
        let config = config();
        assert_eq!(
            adaptive_inbox_interval(&config, Duration::from_secs(1)),
            config.message_check_min
        );
        assert_eq!(
            adaptive_inbox_interval(&config, Duration::from_secs(8_000)),
            config.message_check_max
        );
    }

    #[test]
    fn reconnect_backoff_is_capped() {
        assert_eq!(
            next_reconnect_delay(Duration::from_secs(2)),
            Duration::from_secs(4)
        );
        assert_eq!(
            next_reconnect_delay(Duration::from_secs(200)),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn status_text_is_russian_when_disconnected() {
        let state = StateStore::load(format!(
            "/tmp/kwork-parser-localization-test-state-{}.json",
            std::process::id()
        ))
        .unwrap();
        let telegram = TgBot::new("token", 1);
        let text = status_text(None, &state, &telegram, &config());
        assert!(text.contains("🩺 Состояние"));
        assert!(text.contains("нет подключения"));
        assert!(text.contains("никогда"));
        assert!(!text.contains("admin chat"));
        assert!(!text.contains("disconnected"));
    }
}
