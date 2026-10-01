//! Hashing and cache keys.
//!
//! FNV-1a, chosen for the properties a texture cache actually needs:
//! no dependency, no allocation, and **stable across runs** so a cached
//! key computed in one process still means the same thing in the next.
//! It is not a cryptographic hash and must not be used as one.

/// FNV-1a over `bytes`.
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    return hash;
}

/// A cache key from a source path and its content hash: `path:hash`.
///
/// Both halves are load-bearing. The path keeps two different files from
/// colliding, and the hash is what makes an *edited* file with an
/// unchanged path invalidate — the case a path-only key gets wrong, and
/// the one that shows up as a stale image on screen.
///
/// The hash is zero-padded to 16 hex digits so keys sort and compare as
/// strings without a length surprise.
pub fn cache_key(path: &str, content_hash: u64) -> String {
    return format!("{path}:{content_hash:016x}");
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_same_bytes_hash_the_same_way_every_time() {
        // Stability across runs is the property the cache depends on.
        assert_eq!(content_hash(b"abc"), content_hash(b"abc"));
        assert_eq!(content_hash(b""), 0xcbf29ce484222325, "the FNV-1a offset");
        // The published FNV-1a test vector.
        assert_eq!(content_hash(b"a"), 0xaf63dc4c8601ec8c);
    }

    #[test]
    fn one_flipped_byte_changes_the_hash() {
        assert_ne!(content_hash(b"abc"), content_hash(b"abd"));
        assert_ne!(content_hash(b"abc"), content_hash(b"ab"));
        assert_ne!(content_hash(b"abc"), content_hash(b"acb"));
    }

    #[test]
    fn a_key_separates_a_rewritten_file_from_its_old_contents() {
        let old = cache_key("logo.png", content_hash(b"v1"));
        let new = cache_key("logo.png", content_hash(b"v2"));
        assert_ne!(old, new, "an edit must invalidate");
        // And two different files do not collide.
        assert_ne!(
            cache_key("a.png", content_hash(b"x")),
            cache_key("b.png", content_hash(b"x"))
        );
    }

    #[test]
    fn a_key_is_padded_so_it_compares_as_text() {
        assert_eq!(cache_key("x", 0xff), "x:00000000000000ff");
        assert_eq!(cache_key("x", 0), "x:0000000000000000");
        // The colon is a real separator, so a path containing one still
        // yields distinct keys for distinct paths.
        assert_ne!(cache_key("a:b", 1), cache_key("a", 1));
    }
}
