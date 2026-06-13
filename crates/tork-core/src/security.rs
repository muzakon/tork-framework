//! Small security helpers for application code.

/// Compares two byte strings in constant time relative to their contents.
///
/// Use this to check a secret (an API key, a bearer token, a webhook signature)
/// against an expected value. A naive `a == b` returns as soon as it finds the
/// first differing byte, so the time it takes leaks how many leading bytes matched
/// — enough, over many requests, to recover the secret one byte at a time. This
/// comparison always inspects every byte of an equal-length input, so the timing
/// does not depend on *where* a mismatch occurs.
///
/// The length of the inputs is not itself secret here: unequal lengths return
/// `false` immediately. Secrets are normally fixed-length, so this is the standard
/// trade-off (the same one the `subtle` / `constant_time_eq` crates make).
///
/// # Examples
///
/// ```
/// # use tork_core::security::constant_time_eq;
/// let expected = "s3cr3t-token";
/// assert!(constant_time_eq(expected, "s3cr3t-token"));
/// assert!(!constant_time_eq(expected, "wrong"));
/// ```
pub fn constant_time_eq(a: impl AsRef<[u8]>, b: impl AsRef<[u8]>) -> bool {
    let a = a.as_ref();
    let b = b.as_ref();
    if a.len() != b.len() {
        return false;
    }
    // OR every byte difference together so the loop cannot short-circuit; the
    // result is zero only if every pair of bytes was equal.
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_values_match_and_others_do_not() {
        assert!(constant_time_eq("token-abc", "token-abc"));
        assert!(constant_time_eq(b"\x00\x01\x02".as_slice(), b"\x00\x01\x02".as_slice()));
        assert!(!constant_time_eq("token-abc", "token-abd"));
        // A length mismatch (including a prefix) is not equal.
        assert!(!constant_time_eq("token", "token-abc"));
        assert!(!constant_time_eq("", "x"));
        assert!(constant_time_eq("", ""));
    }
}
