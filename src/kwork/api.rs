//! Mobile API client for `api.kwork.ru` (same surface as kwork-mcp).
//! Auth: HTTP Basic `mobile_api` + user token from signIn.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const API_BASE: &str = "https://api.kwork.ru";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TOKEN_FILE_BYTES: u64 = 16 * 1024;
const MAX_ORDER_PAGES: u32 = 20;
const MAX_ORDERS: usize = 2_000;
/// Public mobile API basic auth (shipped in Kwork mobile clients / kwork-mcp).
const BASIC_USER: &str = "mobile_api";
const BASIC_PASS: &str = "qFvfRl7w";

#[derive(Debug)]
pub enum ApiError {
    Auth(String),
    Http(String),
    Api(String),
    Io(String),
    Parse(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth(e) => write!(f, "auth: {e}"),
            Self::Http(e) => write!(f, "http: {e}"),
            Self::Api(e) => write!(f, "api: {e}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Parse(e) => write!(f, "parse: {e}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenFile {
    token: String,
    expires: f64,
}

pub struct KworkApi {
    agent: ureq::Agent,
    token: String,
    token_expires: f64,
    login: String,
    password: String,
    token_path: PathBuf,
    user_id: Option<i64>,
    expected_kworks: Option<usize>,
}

impl KworkApi {
    pub fn connect(login: String, password: String, token_path: PathBuf) -> Result<Self, ApiError> {
        if login.is_empty() || password.is_empty() {
            return Err(ApiError::Auth(
                "KWORK_LOGIN and KWORK_PASSWORD are required".into(),
            ));
        }

        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(std::time::Duration::from_secs(10)))
            .timeout_global(Some(std::time::Duration::from_secs(45)))
            .http_status_as_error(false)
            .user_agent("kwork-parser/0.1")
            .build();
        let agent = ureq::Agent::new_with_config(config);

        let mut api = Self {
            agent,
            token: String::new(),
            token_expires: 0.0,
            login,
            password,
            token_path,
            user_id: None,
            expected_kworks: None,
        };
        api.load_or_refresh_token()?;
        Ok(api)
    }

    fn now() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    fn load_or_refresh_token(&mut self) -> Result<(), ApiError> {
        let token_file_is_small = fs::metadata(&self.token_path)
            .map(|metadata| metadata.len() <= MAX_TOKEN_FILE_BYTES)
            .unwrap_or(false);
        if token_file_is_small {
            let raw = fs::read_to_string(&self.token_path).unwrap_or_default();
            if let Ok(tf) = serde_json::from_str::<TokenFile>(&raw) {
                // reuse if > 1h left
                if tf.expires > Self::now() + 3600.0 && !tf.token.is_empty() {
                    self.token = tf.token;
                    self.token_expires = tf.expires;
                    info!("Loaded cached API token from {:?}", self.token_path);
                    return Ok(());
                }
            }
        }
        self.sign_in()
    }

    fn sign_in(&mut self) -> Result<(), ApiError> {
        let body = self.post_form(
            "signIn",
            &[
                ("login", self.login.as_str()),
                ("password", self.password.as_str()),
            ],
            None,
        )?;
        if body.get("success").and_then(|v| v.as_bool()) != Some(true) {
            let err = body
                .get("error")
                .map(|e| self.sanitize_text(&e.to_string()))
                .unwrap_or_else(|| "unexpected response".into());
            return Err(ApiError::Auth(format!("signIn failed: {err}")));
        }
        let resp = body
            .get("response")
            .ok_or_else(|| ApiError::Auth("signIn: no response".into()))?;
        let token = resp
            .get("token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| ApiError::Auth("signIn: no token".into()))?
            .to_string();
        let expired = resp
            .get("expired")
            .and_then(|e| e.as_f64())
            .unwrap_or(2_592_000.0);
        self.token = token.clone();
        self.token_expires = Self::now() + expired;

        let tf = TokenFile {
            token,
            expires: self.token_expires,
        };
        let encoded = serde_json::to_vec(&tf)
            .map_err(|e| ApiError::Io(format!("serialize token cache: {e}")))?;
        write_private_atomic(&self.token_path, &encoded)?;
        info!("API token refreshed → {:?}", self.token_path);
        Ok(())
    }

    fn ensure_token(&mut self) -> Result<(), ApiError> {
        if !self.token.is_empty() && self.token_expires > Self::now() + 86_400.0 {
            return Ok(());
        }
        self.sign_in()
    }

    fn post_form(
        &self,
        endpoint: &str,
        fields: &[(&str, &str)],
        token: Option<&str>,
    ) -> Result<Value, ApiError> {
        let mut url = format!("{API_BASE}/{endpoint}");
        if let Some(t) = token {
            url.push_str(&format!("?token={}", urlencoding_minimal(t)));
        }

        let body = encode_form(fields);
        let response = self
            .agent
            .post(&url)
            .header("Authorization", &ureq_basic_auth(BASIC_USER, BASIC_PASS))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(&body)
            .map_err(|e| {
                let message = redact(
                    &e.to_string(),
                    &[token.unwrap_or(""), &self.password, &self.login],
                );
                ApiError::Http(format!("request {endpoint} failed: {message}"))
            })?;

        let status = response.status();
        let text = read_capped(response.into_body().into_reader(), endpoint)?;

        if !(200..300).contains(&status.as_u16()) {
            return Err(ApiError::Http(format!("HTTP {status} for {endpoint}")));
        }

        serde_json::from_str(&text)
            .map_err(|e| ApiError::Parse(format!("invalid JSON from {endpoint}: {e}")))
    }

    fn post(&mut self, endpoint: &str, fields: &[(&str, &str)]) -> Result<Value, ApiError> {
        self.ensure_token()?;
        let token = self.token.clone();
        let body = self.post_form(endpoint, fields, Some(&token))?;

        // auth retry once
        if body.get("success").and_then(|v| v.as_bool()) == Some(false) {
            let err = body.get("error").map(|e| e.to_string()).unwrap_or_default();
            let code = body.get("error_code").and_then(|c| c.as_i64()).unwrap_or(0);
            let lower = err.to_lowercase();
            if code == 401 || code == 403 || lower.contains("token") || lower.contains("авторизац")
            {
                warn!("API auth error on /{endpoint}, refreshing token…");
                self.token.clear();
                self.sign_in()?;
                let token = self.token.clone();
                return self.post_form(endpoint, fields, Some(&token));
            }
        }

        Ok(body)
    }

    /// Unwrap `{success, response}` or return body as-is.
    fn response_field(&self, mut body: Value) -> Result<Value, ApiError> {
        if body.get("success").and_then(|v| v.as_bool()) == Some(false) {
            if let Some(err) = body.get("error") {
                return Err(ApiError::Api(self.sanitize_text(&err.to_string())));
            }
        }
        Ok(match body.get_mut("response") {
            Some(response) => response.take(),
            None => body,
        })
    }

    pub fn get_dialogs(&mut self) -> Result<Vec<Dialog>, ApiError> {
        let body = self.post("dialogs", &[])?;
        let resp = self.response_field(body)?;
        parse_dialogs(&resp)
    }

    pub fn get_actor_id(&mut self) -> Result<i64, ApiError> {
        if let Some(id) = self.user_id {
            return Ok(id);
        }
        let body = self.post("actor", &[])?;
        let resp = self.response_field(body)?;
        let id = resp
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| ApiError::Parse("actor: no id".into()))?;
        self.user_id = Some(id);
        self.expected_kworks = resp
            .get("kworks_count")
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok());
        Ok(id)
    }

    pub fn get_my_kworks(&mut self) -> Result<Vec<KworkActivity>, ApiError> {
        let uid = self.get_actor_id()?;
        let uid_s = uid.to_string();
        let body = self.post("userKworks", &[("user_id", uid_s.as_str())])?;
        let resp = self.response_field(body)?;
        parse_kworks(&resp)
    }

    pub fn get_my_kworks_snapshot(&mut self) -> Result<(Vec<KworkActivity>, bool), ApiError> {
        self.get_actor()?;
        let kworks = self.get_my_kworks()?;
        let complete = snapshot_complete(self.expected_kworks, kworks.len());
        Ok((kworks, complete))
    }

    pub fn token_expires_in_secs(&self) -> i64 {
        (self.token_expires - Self::now()) as i64
    }

    pub fn get_actor(&mut self) -> Result<ActorInfo, ApiError> {
        let body = self.post("actor", &[])?;
        let resp = self.response_field(body)?;
        if let Some(id) = resp.get("id").and_then(|v| v.as_i64()) {
            self.user_id = Some(id);
        }
        self.expected_kworks = resp
            .get("kworks_count")
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok());
        Ok(ActorInfo {
            username: resp
                .get("username")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            free_amount: resp
                .get("free_amount")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            hold_amount: resp
                .get("hold_amount")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            rating: resp.get("rating").and_then(|v| v.as_f64()).unwrap_or(0.0),
            good_reviews: resp
                .get("good_reviews")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            bad_reviews: resp
                .get("bad_reviews")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            unread_dialog_count: resp
                .get("unread_dialog_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            unread_messages_count: resp
                .get("unread_messages_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            kworks_count: resp
                .get("kworks_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            completed_orders_count: resp
                .get("completed_orders_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
        })
    }

    /// Worker orders with bounded pagination. Empty list if none / error 151.
    pub fn get_worker_orders(&mut self, filter: &str) -> Result<Vec<OrderInfo>, ApiError> {
        let mut orders = Vec::new();
        let mut seen = HashSet::new();
        for page in 1..=MAX_ORDER_PAGES {
            let page_text = page.to_string();
            let mut body = self.post(
                "workerOrders",
                &[("filter", filter), ("page", page_text.as_str())],
            )?;
            if body.get("success").and_then(Value::as_bool) == Some(false) {
                let code = body.get("error_code").and_then(Value::as_i64).unwrap_or(0);
                if code == 151 {
                    break;
                }
                if let Some(error) = body.get("error") {
                    return Err(ApiError::Api(self.sanitize_text(&error.to_string())));
                }
            }
            let body_last_page = last_page(&body);
            let response = match body.get_mut("response") {
                Some(value) => value.take(),
                None => body,
            };
            let response_last_page = last_page(&response);
            let page_orders = parse_orders(&response);
            let mut added = 0;
            for order in page_orders {
                if seen.insert(order.id) {
                    orders.push(order);
                    added += 1;
                    if orders.len() == MAX_ORDERS {
                        return Ok(orders);
                    }
                }
            }
            if added == 0
                || body_last_page
                    .or(response_last_page)
                    .is_some_and(|last| page >= last)
            {
                break;
            }
        }
        Ok(orders)
    }

    pub fn get_connects(&mut self) -> Result<(i64, i64), ApiError> {
        let body = self.post("projects", &[])?;
        let connects = body.get("connects").unwrap_or(&Value::Null);
        let active = connects
            .get("active_connects")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let all = connects
            .get("all_connects")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        Ok((active, all))
    }

    fn sanitize_text(&self, message: &str) -> String {
        bounded(
            redact(message, &[&self.login, &self.password, &self.token]),
            200,
        )
    }
}

fn parse_dialogs(value: &Value) -> Result<Vec<Dialog>, ApiError> {
    let list = value
        .as_array()
        .ok_or_else(|| ApiError::Parse("dialogs: expected an array".into()))?;
    let mut dialogs = Vec::with_capacity(list.len());
    for item in list {
        let user_id = item.get("user_id").and_then(Value::as_i64).unwrap_or(0);
        let unread_count = item
            .get("unread_count")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        dialogs.push(Dialog {
            username: item
                .get("username")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            user_id,
            unread: item.get("unread").and_then(Value::as_bool).unwrap_or(false)
                || unread_count > 0,
            unread_count,
            last_message: item
                .get("last_message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            time: item.get("time").and_then(Value::as_i64).unwrap_or(0),
            link: item
                .get("link")
                .and_then(Value::as_str)
                .map(|link| {
                    if link.starts_with("http") {
                        link.to_string()
                    } else {
                        format!("https://kwork.ru{link}")
                    }
                })
                .unwrap_or_else(|| format!("https://kwork.ru/inbox/{user_id}")),
        });
    }
    Ok(dialogs)
}

fn parse_kworks(value: &Value) -> Result<Vec<KworkActivity>, ApiError> {
    let list = value
        .as_array()
        .ok_or_else(|| ApiError::Parse("userKworks: expected an array".into()))?;
    let mut kworks = Vec::with_capacity(list.len());
    for item in list {
        let activity = item.get("activity");
        kworks.push(KworkActivity {
            id: item.get("id").and_then(Value::as_i64).unwrap_or(0),
            name: item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string(),
            views: activity
                .and_then(|value| value.get("views"))
                .and_then(Value::as_i64)
                .unwrap_or(0),
            orders: activity
                .and_then(|value| value.get("orders"))
                .and_then(Value::as_i64)
                .unwrap_or(0),
        });
    }
    Ok(kworks)
}

fn parse_orders(value: &Value) -> Vec<OrderInfo> {
    let list = value
        .as_array()
        .or_else(|| value.get("orders").and_then(Value::as_array))
        .or_else(|| value.get("list").and_then(Value::as_array));
    let Some(list) = list else {
        return Vec::new();
    };
    let mut orders = Vec::with_capacity(list.len());
    for item in list {
        let id = item
            .get("order_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if id == 0 {
            continue;
        }
        let status = item
            .get("status")
            .or_else(|| item.get("status_name"))
            .map(|value| match value {
                Value::String(value) => value.clone(),
                Value::Number(value) => value.to_string(),
                _ => value.to_string(),
            })
            .unwrap_or_else(|| "?".into());
        let title = item
            .get("kwork_title")
            .or_else(|| item.get("title"))
            .or_else(|| item.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("order")
            .to_string();
        let username = item
            .get("username")
            .or_else(|| item.get("payer_username"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        orders.push(OrderInfo {
            id,
            status,
            title,
            username,
        });
    }
    orders
}

fn last_page(value: &Value) -> Option<u32> {
    const CONTAINERS: [&str; 3] = ["pagination", "paging", "pager"];
    const KEYS: [&str; 4] = ["last_page", "total_pages", "pages", "page_count"];
    for key in KEYS {
        if let Some(page) = value
            .get(key)
            .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
            .and_then(|page| u32::try_from(page).ok())
        {
            return Some(page.max(1));
        }
    }
    for container in CONTAINERS {
        if let Some(page) = value.get(container).and_then(last_page) {
            return Some(page);
        }
    }
    None
}

fn snapshot_complete(expected: Option<usize>, actual: usize) -> bool {
    expected.is_some_and(|expected| actual >= expected)
}

#[derive(Debug, Clone)]
pub struct Dialog {
    pub username: String,
    pub user_id: i64,
    pub unread: bool,
    pub unread_count: i64,
    pub last_message: String,
    pub time: i64,
    pub link: String,
}

#[derive(Debug, Clone)]
pub struct KworkActivity {
    pub id: i64,
    pub name: String,
    pub views: i64,
    pub orders: i64,
}

#[derive(Debug, Clone)]
pub struct OrderInfo {
    pub id: i64,
    pub status: String,
    pub title: String,
    pub username: String,
}

#[derive(Debug, Clone)]
pub struct ActorInfo {
    pub username: String,
    pub free_amount: f64,
    pub hold_amount: f64,
    pub rating: f64,
    pub good_reviews: i64,
    pub bad_reviews: i64,
    pub unread_dialog_count: i64,
    pub unread_messages_count: i64,
    pub kworks_count: i64,
    pub completed_orders_count: i64,
}

fn ureq_basic_auth(user: &str, pass: &str) -> String {
    use base64::Engine;
    let raw = format!("{user}:{pass}");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(raw.as_bytes())
    )
}

fn encode_form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding_minimal(k), urlencoding_minimal(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Minimal application/x-www-form-urlencoded (enough for tokens & ascii forms).
fn urlencoding_minimal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(hex(*b >> 4));
                out.push(hex(*b & 0xf));
            }
        }
    }
    out
}

fn hex(n: u8) -> char {
    b"0123456789ABCDEF"[n as usize] as char
}

fn read_capped(mut reader: impl Read, endpoint: &str) -> Result<String, ApiError> {
    let mut bytes = Vec::with_capacity(8 * 1024);
    let mut buffer = [0u8; 8 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|e| ApiError::Http(format!("read {endpoint}: {e}")))?;
        if read == 0 {
            break;
        }
        if bytes.len() + read > MAX_RESPONSE_BYTES {
            return Err(ApiError::Http(format!(
                "response from {endpoint} exceeds {MAX_RESPONSE_BYTES} bytes"
            )));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(bytes)
        .map_err(|e| ApiError::Parse(format!("non-UTF-8 response from {endpoint}: {e}")))
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), ApiError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|e| ApiError::Io(format!("create token directory: {e}")))?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}", std::process::id()));
    let temporary = path.with_file_name(name);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|e| ApiError::Io(format!("create token cache: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .map_err(|e| ApiError::Io(format!("protect token cache: {e}")))?;
    }
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| ApiError::Io(format!("write token cache: {e}")))?;
    fs::rename(&temporary, path).map_err(|e| {
        let _ = fs::remove_file(&temporary);
        ApiError::Io(format!("replace token cache: {e}"))
    })
}

fn bounded(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn redact(message: &str, secrets: &[&str]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(message.to_string(), |clean, secret| {
            clean.replace(secret, "[redacted]")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_encoding_is_correct() {
        assert_eq!(encode_form(&[("a b", "x+y")]), "a+b=x%2By");
    }

    #[test]
    fn secrets_are_redacted() {
        assert_eq!(
            redact("token=abc password=pw", &["abc", "pw"]),
            "token=[redacted] password=[redacted]"
        );
    }

    #[test]
    fn response_limit_is_enforced() {
        let oversized = vec![b'x'; MAX_RESPONSE_BYTES + 1];
        assert!(read_capped(std::io::Cursor::new(oversized), "test").is_err());
    }

    #[test]
    fn sanitized_fixtures_deserialize() {
        let dialogs = serde_json::json!([{
            "username": "buyer",
            "user_id": 42,
            "unread_count": 2,
            "last_message": "hello",
            "time": 123,
            "link": "/inbox/42"
        }]);
        let parsed = parse_dialogs(&dialogs).unwrap();
        assert!(parsed[0].unread);
        assert_eq!(parsed[0].link, "https://kwork.ru/inbox/42");

        let kworks = serde_json::json!([{
            "id": 7,
            "title": "Rust bot",
            "activity": {"views": 11, "orders": 3}
        }]);
        let parsed = parse_kworks(&kworks).unwrap();
        assert_eq!(
            (parsed[0].id, parsed[0].views, parsed[0].orders),
            (7, 11, 3)
        );

        let orders = serde_json::json!({"orders": [{
            "order_id": 9,
            "status_name": "active",
            "kwork_title": "Rust bot",
            "payer_username": "buyer"
        }]});
        assert_eq!(parse_orders(&orders)[0].id, 9);
        assert_eq!(
            last_page(&serde_json::json!({"pagination": {"last_page": 3}})),
            Some(3)
        );
        assert!(snapshot_complete(Some(2), 2));
        assert!(!snapshot_complete(Some(2), 1));
        assert!(!snapshot_complete(None, 2));
    }
}
