/// Stable FNV-1a fingerprint without pulling in a hashing dependency.
pub fn fingerprint(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn format_ago(unix: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if unix == 0 {
        return "never".into();
    }
    let d = now.saturating_sub(unix);
    if d < 60 {
        format!("{d}s ago")
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else if d < 86400 {
        format!("{}h ago", d / 3600)
    } else {
        format!("{}d ago", d / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_elapsed_time() {
        assert_eq!(format_ago(0), "never");
    }

    #[test]
    fn fingerprints_are_stable() {
        assert_eq!(fingerprint("hello"), "a430d84680aabd0b");
        assert_ne!(fingerprint("hello"), fingerprint("hello!"));
    }
}
