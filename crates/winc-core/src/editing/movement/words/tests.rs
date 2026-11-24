use ropey::Rope;

use super::*;

#[test]
fn words() {
    let text = Rope::from_str("let foo_bar = baz(1);");
    assert_eq!(next_word(&text, 0), 4);
    assert_eq!(next_word(&text, 4), 12);
    assert_eq!(next_word(&text, 12), 14);
    assert_eq!(prev_word(&text, 12), 4);
    assert_eq!(prev_word(&text, 4), 0);
    assert_eq!(prev_word(&text, 17), 14);
    assert_eq!(word_at(&text, 6), (4, 11));
    assert_eq!(word_at(&text, 11), (4, 11));
    assert_eq!(word_at(&text, 21), (19, 21));
}

#[test]
fn word_operations_keep_combining_marks_and_emoji_intact() {
    let text = Rope::from_str("ae\u{301}b 👩\u{200d}💻 next");
    assert_eq!(word_at(&text, 2), (0, 4));
    assert_eq!(word_at(&text, 7), (5, 8));
    assert_eq!(next_word(&text, 0), 5);
    assert_eq!(prev_word(&text, 5), 0);
    assert_eq!(next_word(&text, 5), 9);
    assert_eq!(prev_word(&text, 9), 5);
    assert_eq!(next_word(&text, 7), 9);
    assert_eq!(prev_word(&text, 7), 5);
}

#[test]
fn word_operations_cross_rope_chunks() {
    let word = "e\u{301}".repeat(2_000);
    let text = Rope::from_str(&format!("{word} next"));
    let end = word.chars().count();
    assert_eq!(word_at(&text, end / 2 + 1), (0, end));
    assert_eq!(next_word(&text, 0), end + 1);
    assert_eq!(prev_word(&text, end + 1), 0);
}
