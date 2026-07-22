use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const STATE_VERSION: u32 = 1;
const MAX_DIALOGS: usize = 1_000;
const MAX_KWORKS: usize = 1_000;
const MAX_ORDERS: usize = 2_000;
const MAX_ERROR_CHARS: usize = 500;
const MAX_STATE_BYTES: usize = 6 * 1024 * 1024;
const MAX_FINGERPRINT_BYTES: usize = 256;
const MAX_KWORK_NAME_BYTES: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fingerprint {
    pub value: String,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KworkSnapshot {
    pub name: String,
    pub views: i64,
    pub orders: i64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorRecord {
    pub message: String,
    pub at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateData {
    pub version: u32,
    #[serde(default)]
    pub dialogs: BTreeMap<String, Fingerprint>,
    #[serde(default)]
    pub kworks: BTreeMap<String, KworkSnapshot>,
    #[serde(default)]
    pub orders: BTreeMap<String, Fingerprint>,
    #[serde(default)]
    pub health: BTreeMap<String, u64>,
    #[serde(default)]
    pub orders_seeded: bool,
    #[serde(default)]
    pub last_error: Option<ErrorRecord>,
}

impl Default for StateData {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            dialogs: BTreeMap::new(),
            kworks: BTreeMap::new(),
            orders: BTreeMap::new(),
            health: BTreeMap::new(),
            orders_seeded: false,
            last_error: None,
        }
    }
}

#[derive(Debug)]
pub struct StateStore {
    path: PathBuf,
    data: StateData,
    dirty: bool,
}

impl StateStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let mut data = match fs::metadata(&path) {
            Ok(metadata) if metadata.len() > MAX_STATE_BYTES as u64 => {
                return Err(format!(
                    "state file {} is too large ({} bytes; max {MAX_STATE_BYTES})",
                    path.display(),
                    metadata.len()
                ))
            }
            Ok(_) => {
                let raw =
                    fs::read(&path).map_err(|e| format!("read state {}: {e}", path.display()))?;
                if raw.len() > MAX_STATE_BYTES {
                    return Err(format!(
                        "state file {} grew beyond {MAX_STATE_BYTES} bytes while reading",
                        path.display()
                    ));
                }
                serde_json::from_slice::<StateData>(&raw)
                    .map_err(|e| format!("invalid state file {}: {e}", path.display()))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StateData::default(),
            Err(error) => return Err(format!("inspect state {}: {error}", path.display())),
        };
        if data.version != STATE_VERSION {
            return Err(format!(
                "unsupported state version {} (expected {STATE_VERSION})",
                data.version
            ));
        }
        let dirty = normalize(&mut data);
        Ok(Self { path, data, dirty })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn dialog(&self, user_id: i64) -> Option<&str> {
        self.data
            .dialogs
            .get(&user_id.to_string())
            .map(|entry| entry.value.as_str())
    }

    pub fn set_dialog(&mut self, user_id: i64, value: String) {
        set_fingerprint(
            &mut self.data.dialogs,
            user_id.to_string(),
            truncate(value, MAX_FINGERPRINT_BYTES),
        );
        prune_oldest(&mut self.data.dialogs, MAX_DIALOGS, |v| v.updated_at);
        self.dirty = true;
    }

    pub fn kwork(&self, id: i64) -> Option<&KworkSnapshot> {
        self.data.kworks.get(&id.to_string())
    }

    pub fn set_kwork(&mut self, id: i64, name: String, views: i64, orders: i64) {
        self.data.kworks.insert(
            id.to_string(),
            KworkSnapshot {
                name: truncate(name, MAX_KWORK_NAME_BYTES),
                views,
                orders,
                updated_at: now(),
            },
        );
        prune_oldest(&mut self.data.kworks, MAX_KWORKS, |v| v.updated_at);
        self.dirty = true;
    }

    pub fn kworks(&self) -> impl Iterator<Item = &KworkSnapshot> {
        self.data.kworks.values()
    }

    pub fn retain_kworks(&mut self, active_ids: &[i64]) {
        let before = self.data.kworks.len();
        self.data.kworks.retain(|id, _| {
            id.parse::<i64>()
                .ok()
                .is_some_and(|id| active_ids.contains(&id))
        });
        self.dirty |= self.data.kworks.len() != before;
    }

    pub fn order(&self, id: i64) -> Option<&str> {
        self.data
            .orders
            .get(&id.to_string())
            .map(|entry| entry.value.as_str())
    }

    pub fn set_order(&mut self, id: i64, value: String) {
        set_fingerprint(
            &mut self.data.orders,
            id.to_string(),
            truncate(value, MAX_FINGERPRINT_BYTES),
        );
        prune_oldest(&mut self.data.orders, MAX_ORDERS, |v| v.updated_at);
        self.dirty = true;
    }

    pub fn orders_seeded(&self) -> bool {
        self.data.orders_seeded
    }

    pub fn mark_orders_seeded(&mut self) {
        if !self.data.orders_seeded {
            self.data.orders_seeded = true;
            self.dirty = true;
        }
    }

    pub fn last_ok(&self, job: &str) -> Option<u64> {
        self.data.health.get(job).copied()
    }

    pub fn touch_ok(&mut self, job: &str) {
        self.data.health.insert(job.to_string(), now());
        self.dirty = true;
    }

    pub fn touch_error(&mut self, message: &str) {
        self.data.last_error = Some(ErrorRecord {
            message: message.chars().take(MAX_ERROR_CHARS).collect(),
            at: now(),
        });
        self.dirty = true;
    }

    pub fn last_error(&self) -> Option<&ErrorRecord> {
        self.data.last_error.as_ref()
    }

    pub fn save(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create state directory {}: {e}", parent.display()))?;
        }

        let tmp = temporary_path(&self.path);
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)
            .map_err(|e| format!("create state temp {}: {e}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("protect state temp {}: {e}", tmp.display()))?;
        }
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, &self.data)
            .map_err(|e| format!("serialize state: {e}"))?;
        writer
            .write_all(b"\n")
            .and_then(|_| writer.flush())
            .map_err(|e| format!("write state temp {}: {e}", tmp.display()))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|e| format!("sync state temp {}: {e}", tmp.display()))?;
        fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            format!(
                "replace state {} from {}: {e}",
                self.path.display(),
                tmp.display()
            )
        })?;
        self.dirty = false;
        Ok(())
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn set_fingerprint(map: &mut BTreeMap<String, Fingerprint>, key: String, value: String) {
    map.insert(
        key,
        Fingerprint {
            value,
            updated_at: now(),
        },
    );
}

fn prune_oldest<T>(map: &mut BTreeMap<String, T>, limit: usize, timestamp: impl Fn(&T) -> u64) {
    if map.len() <= limit {
        return;
    }
    let remove_count = map.len() - limit;
    let mut entries: Vec<_> = map
        .iter()
        .map(|(key, value)| (timestamp(value), key.clone()))
        .collect();
    entries.sort_unstable();
    for (_, key) in entries.into_iter().take(remove_count) {
        map.remove(&key);
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}", std::process::id()));
    path.with_file_name(name)
}

fn normalize(data: &mut StateData) -> bool {
    let before = (data.dialogs.len(), data.kworks.len(), data.orders.len());
    let mut changed = false;
    for entry in data.dialogs.values_mut().chain(data.orders.values_mut()) {
        if entry.value.len() > MAX_FINGERPRINT_BYTES {
            entry.value = truncate(std::mem::take(&mut entry.value), MAX_FINGERPRINT_BYTES);
            changed = true;
        }
    }
    for entry in data.kworks.values_mut() {
        if entry.name.len() > MAX_KWORK_NAME_BYTES {
            entry.name = truncate(std::mem::take(&mut entry.name), MAX_KWORK_NAME_BYTES);
            changed = true;
        }
    }
    if let Some(error) = data.last_error.as_mut() {
        if error.message.chars().count() > MAX_ERROR_CHARS {
            error.message = error.message.chars().take(MAX_ERROR_CHARS).collect();
            changed = true;
        }
    }
    prune_oldest(&mut data.dialogs, MAX_DIALOGS, |value| value.updated_at);
    prune_oldest(&mut data.kworks, MAX_KWORKS, |value| value.updated_at);
    prune_oldest(&mut data.orders, MAX_ORDERS, |value| value.updated_at);
    changed || before != (data.dialogs.len(), data.kworks.len(), data.orders.len())
}

fn truncate(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "kwork-parser-{label}-{}-{}.json",
            std::process::id(),
            now()
        ))
    }

    #[test]
    fn round_trip_and_health() {
        let path = test_path("round-trip");
        let mut store = StateStore::load(&path).unwrap();
        store.set_dialog(7, "fingerprint".into());
        store.set_kwork(9, "Name".into(), 3, 1);
        store.set_order(11, "active".into());
        store.mark_orders_seeded();
        store.touch_ok("inbox");
        store.save().unwrap();

        let loaded = StateStore::load(&path).unwrap();
        assert_eq!(loaded.dialog(7), Some("fingerprint"));
        assert_eq!(loaded.kwork(9).map(|v| v.views), Some(3));
        assert_eq!(loaded.order(11), Some("active"));
        assert!(loaded.orders_seeded());
        assert!(loaded.last_ok("inbox").is_some());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_unknown_version() {
        let path = test_path("version");
        fs::write(&path, r#"{"version":99}"#).unwrap();
        assert!(StateStore::load(&path)
            .unwrap_err()
            .contains("unsupported state version"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn pruning_keeps_bound() {
        let path = test_path("prune");
        let mut store = StateStore::load(&path).unwrap();
        for id in 0..=MAX_DIALOGS as i64 {
            store.set_dialog(id, id.to_string());
        }
        assert_eq!(store.data.dialogs.len(), MAX_DIALOGS);
    }

    #[test]
    fn error_is_bounded() {
        let path = test_path("error");
        let mut store = StateStore::load(&path).unwrap();
        store.touch_error(&"x".repeat(MAX_ERROR_CHARS + 10));
        assert_eq!(store.last_error().unwrap().message.len(), MAX_ERROR_CHARS);
    }

    #[test]
    fn persistent_strings_are_bounded() {
        let path = test_path("strings");
        let mut store = StateStore::load(&path).unwrap();
        store.set_dialog(1, "🙂".repeat(MAX_FINGERPRINT_BYTES));
        store.set_kwork(2, "n".repeat(MAX_KWORK_NAME_BYTES + 1), 0, 0);
        assert_eq!(store.dialog(1).unwrap().len(), MAX_FINGERPRINT_BYTES);
        assert_eq!(store.kwork(2).unwrap().name.len(), MAX_KWORK_NAME_BYTES);
    }
}
