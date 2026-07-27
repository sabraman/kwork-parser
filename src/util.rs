/// Stable FNV-1a fingerprint without pulling in a hashing dependency.
pub fn fingerprint(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_are_stable() {
        assert_eq!(fingerprint("hello"), "a430d84680aabd0b");
        assert_ne!(fingerprint("hello"), fingerprint("hello!"));
    }
}
