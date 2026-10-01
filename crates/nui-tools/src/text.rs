//! Text helpers: edit distance, field validators, and UTF-8 index
//! conversion.
//!
//! The validators are predicates over what a text *field* currently holds,
//! and they say "valid" for the partial states a user passes through while
//! typing. That is the whole design, and it is why they are not spelled
//! `parse::<i64>().is_ok()`.

/// Levenshtein distance in **characters**, not bytes.
///
/// Characters matter: a caller that measures bytes would score a CJK typo
/// as several edits and refuse to suggest the obvious correction. The
/// tests pin this (`按钮` vs `按纽` is 1).
///
/// Classic two-row dynamic programming: `O(left × right)` time,
/// `O(right)` space. No early cutoff — the callers apply their own
/// ceiling afterwards, which keeps this function honest about the true
/// distance.
pub fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.is_empty() {
        return right.len();
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current: Vec<usize> = vec![0; right.len() + 1];
    for (row, &left_char) in left.iter().enumerate() {
        current[0] = row + 1;
        for (column, &right_char) in right.iter().enumerate() {
            let substitute = previous[column] + usize::from(left_char != right_char);
            current[column + 1] = substitute
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    return previous[right.len()];
}

/// An optionally signed run of decimal digits.
///
/// An empty string is valid: "nothing typed yet" is not a mistake, and
/// requiring a value is the document's business. A lone sign is *not*
/// valid — that is a text that has stopped being a number partway
/// through, which is the one partial state that cannot be resumed by
/// typing more digits.
pub fn is_integer(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    let digits = text
        .strip_prefix('-')
        .or_else(|| return text.strip_prefix('+'))
        .unwrap_or(text);
    return !digits.is_empty()
        && digits
            .chars()
            .all(|character| return character.is_ascii_digit());
}

/// An optionally signed decimal with at most one point.
///
/// A trailing or leading point is accepted (`"1."`, `".5"`): both are
/// states a field passes through while being typed, and flagging them
/// mid-keystroke is noise, not information. A bare `"."` is rejected —
/// there is no digit to resume from — and so is exponent notation, which
/// this deliberately does not teach.
pub fn is_float(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    let body = text
        .strip_prefix('-')
        .or_else(|| return text.strip_prefix('+'))
        .unwrap_or(text);
    let mut seen_point = false;
    let mut digits = 0;
    for character in body.chars() {
        if character == '.' {
            if seen_point {
                return false;
            }
            seen_point = true;
            continue;
        }
        if !character.is_ascii_digit() {
            return false;
        }
        digits += 1;
    }
    return digits > 0;
}

/// A pragmatic email shape: exactly one `@`, a non-empty local part, and
/// a dotted domain with no empty labels and no whitespace anywhere.
///
/// Deliberately not RFC 5322. That grammar admits addresses no mail system
/// accepts and its complexity buys nothing for a form hint; what this does
/// catch is the mistakes people actually make — a missing `@`, a domain
/// without a dot, a trailing dot, a stray space from a paste. The local
/// part is not character-checked, so `a+b@c.co` passes.
pub fn is_email(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    if text
        .chars()
        .any(|character| return character.is_whitespace())
    {
        return false;
    }
    let mut parts = text.split('@');
    let local = parts.next().unwrap_or_default();
    let Some(domain) = parts.next() else {
        return false;
    };
    if parts.next().is_some() || local.is_empty() || domain.is_empty() {
        return false;
    }
    return domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..");
}

/// Byte offset of char index `index` in a collected char slice.
///
/// `index` is clamped to the slice length by `take`, so an out-of-range
/// index yields the total byte length rather than a panic. That matters
/// for a caret: a cursor left past the end by a shrink is a normal event,
/// and the answer "at the end of the text" is the right one.
pub fn char_to_byte(chars: &[char], index: usize) -> usize {
    return chars
        .iter()
        .take(index)
        .map(|character| return character.len_utf8())
        .sum();
}

/// Char index of a byte offset.
///
/// The inverse of [`char_to_byte`] for offsets already known to be on a
/// char boundary. `byte` is clamped to the text length, so an offset past
/// the end yields the char count rather than a panic.
///
/// # Panics
///
/// A `byte` that is **inside** a multi-byte character panics: the slice
/// `text[..byte]` is not a valid `str` boundary. The clamp only covers
/// "past the end", not "in the middle". Both callers pass an offset a
/// shaper reported, which is always on a boundary, so this has never
/// fired — but it is the one input this function does not defend against,
/// and the failure is a panic rather than a wrong answer. A caller that
/// cannot promise an aligned offset wants
/// `text[..byte].chars().count()` with its own rounding-down, e.g.
/// `text.char_indices().take_while(|(i, _)| *i < byte).count()`.
pub fn byte_to_char(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    return text[..byte].chars().count();
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_distance_metric_counts_characters_not_bytes() {
        // Type names are ASCII today, but a host may register one that is
        // not, and byte slicing would panic on a multi-byte boundary.
        assert_eq!(edit_distance("ab", "ab"), 0);
        assert_eq!(edit_distance("ab", "ac"), 1);
        assert_eq!(edit_distance("ab", "abc"), 1);
        assert_eq!(edit_distance("abc", "ab"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("按钮", "按纽"), 1);
    }

    #[test]
    fn the_distance_metric_handles_both_empty_sides() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("abc", ""), 3);
    }

    #[test]
    fn the_distance_metric_is_symmetric_on_the_cases_that_matter() {
        for (left, right) in [
            ("counter", "contuer"),
            ("width", "widht"),
            ("", "x"),
            ("按钮", "按纽"),
        ] {
            assert_eq!(
                edit_distance(left, right),
                edit_distance(right, left),
                "{left} vs {right}"
            );
        }
    }

    #[test]
    fn validators_accept_what_they_promise() {
        assert!(is_integer("42"));
        assert!(is_integer("-7"));
        assert!(is_integer("+7"));
        assert!(!is_integer("4.2"));
        assert!(!is_integer("-"));
        assert!(is_integer(""), "empty is not a mistake");

        assert!(is_float("4.2"));
        assert!(is_float("-0.5"));
        assert!(is_float("4."), "still being typed");
        assert!(is_float(".5"));
        assert!(!is_float("1.2.3"));
        assert!(!is_float("1e5"));
        assert!(!is_float("."));
        assert!(is_float(""));

        assert!(is_email("a@b.co"));
        assert!(!is_email("a@b"), "no dotted domain");
        assert!(!is_email("a@b..co"));
        assert!(!is_email("a@.co"));
        assert!(!is_email("a@b.co."));
        assert!(!is_email("a b@c.co"));
        assert!(!is_email("a@@b.co"));
        assert!(
            is_email("a+b@c.co"),
            "the local part is not character-checked"
        );
        assert!(is_email(""));
    }

    #[test]
    fn char_and_byte_offsets_round_trip() {
        let text = "a按b纽";
        let chars: Vec<char> = text.chars().collect();
        for index in 0..=chars.len() {
            let byte = char_to_byte(&chars, index);
            assert_eq!(byte_to_char(text, byte), index, "index {index}");
        }
    }

    #[test]
    fn an_out_of_range_index_clamps_instead_of_panicking() {
        let chars: Vec<char> = "ab".chars().collect();
        assert_eq!(char_to_byte(&chars, 99), 2, "past the end is the end");
        assert_eq!(char_to_byte(&[], 3), 0, "an empty slice has no bytes");
        assert_eq!(byte_to_char("ab", 99), 2);
        assert_eq!(byte_to_char("", 5), 0);
    }

    #[test]
    fn a_byte_inside_a_character_is_a_panic_not_a_round_down() {
        // This is the documented hazard, pinned so the doc comment cannot
        // rot: the clamp covers "past the end" but not "mid-character".
        let result = std::panic::catch_unwind(|| return byte_to_char("按b", 1));
        assert!(result.is_err(), "a mid-character offset must not be silent");
        // The boundary offsets are fine, on both sides of the character.
        assert_eq!(byte_to_char("按b", 0), 0);
        assert_eq!(byte_to_char("按b", 3), 1);
        assert_eq!(byte_to_char("按b", 4), 2);
    }

    #[test]
    fn the_mid_character_offset_is_avoidable_by_rounding_down_first() {
        // The recipe the doc comment offers: index-driven, so it can only
        // produce boundaries. This is what a caller with an unaligned
        // offset should do instead.
        let text = "按b";
        for byte in 0..=text.len() {
            let chars = text
                .char_indices()
                .take_while(|(index, _)| return *index < byte)
                .count();
            // Never panics, and agrees with the slice version wherever the
            // slice version is legal.
            if text.is_char_boundary(byte) {
                assert_eq!(chars, byte_to_char(text, byte), "byte {byte}");
            }
        }
    }
}
