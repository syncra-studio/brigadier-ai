//! What a personal memory never keeps (Q78): a password, a key or a code, a card number, a
//! bank account. Health and money details are the thread's call (its instructions say so);
//! these are the ones a cheap check can tell for sure, so Brigadier refuses them itself,
//! whoever saves or edits the memory.

use std::sync::LazyLock;

use regex::Regex;

/// A secret's name followed by its value: "my password is hunter2", "PIN: 1234".
static NAMED_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(password|passcode|passphrase|pin|api[ _-]?key|secret|token|cvv|cvc|security code|account number|routing number|sort code|social security number|ssn)\b\s*(?:is|was|=|:)\s*\S+",
    )
    .unwrap()
});

/// A key or token by its shape: well-known prefixes, or a long run mixing letters and digits.
static KEY_SHAPED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:sk-[A-Za-z0-9_-]{16,}|ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}|xox[abpr]-[A-Za-z0-9-]{10,})|\b[A-Za-z0-9_-]{32,}\b",
    )
    .unwrap()
});

/// A run of digits with spaces or dashes between groups, as a card number is written.
static DIGIT_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d(?:[ -]?\d){12,18}").unwrap());

/// An IBAN, spaced or not.
static IBAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]){11,30}\b").unwrap());

/// Why `text` can't be a memory, in words for the caller's refusal; `None` when it can.
pub(crate) fn not_for_memory(text: &str) -> Option<&'static str> {
    if NAMED_SECRET.is_match(text) {
        return Some("it holds a password, code or account detail");
    }
    if KEY_SHAPED
        .find_iter(text)
        .any(|found| key_like(found.as_str()))
    {
        return Some("it holds a key or token");
    }
    if DIGIT_RUN.find_iter(text).any(|found| luhn(found.as_str())) {
        return Some("it holds a card number");
    }
    if IBAN.find_iter(text).any(|found| iban(found.as_str())) {
        return Some("it holds a bank account number");
    }
    None
}

/// Whether a long run is a key: it mixes letters and digits (a long word or path doesn't).
fn key_like(run: &str) -> bool {
    run.starts_with("sk-")
        || run.starts_with("ghp_")
        || run.starts_with("github_pat_")
        || run.starts_with("AKIA")
        || run.starts_with("xox")
        || (run.chars().any(|c| c.is_ascii_digit()) && run.chars().any(|c| c.is_ascii_alphabetic()))
}

/// The Luhn check every card number passes.
fn luhn(run: &str) -> bool {
    let digits: Vec<u32> = run.chars().filter_map(|c| c.to_digit(10)).collect();
    if !(13..=19).contains(&digits.len()) {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(index, &digit)| match index % 2 {
            1 if digit * 2 > 9 => digit * 2 - 9,
            1 => digit * 2,
            _ => digit,
        })
        .sum();
    sum.is_multiple_of(10)
}

/// The mod-97 check every IBAN passes.
fn iban(found: &str) -> bool {
    let compact: String = found.chars().filter(|c| !c.is_whitespace()).collect();
    if !(15..=34).contains(&compact.len()) {
        return false;
    }
    let (head, tail) = compact.split_at(4);
    let mut remainder: u32 = 0;
    for c in tail.chars().chain(head.chars()) {
        let value = match c.to_digit(36) {
            Some(value) => value,
            None => return false,
        };
        for digit in value.to_string().chars() {
            remainder = (remainder * 10 + digit.to_digit(10).unwrap_or(0)) % 97;
        }
    }
    remainder == 1
}

#[cfg(test)]
mod tests {
    use super::not_for_memory;

    #[test]
    fn stable_personal_facts_are_kept() {
        for fact in [
            "Lives in Chisinau, Moldova",
            "Works as an electrician",
            "Prints on a Bambu Lab P1S with a 0.4 mm nozzle",
            "Uses a password manager",
            "Prefers metric units and prices in EUR",
            "Phone model: Pixel 9",
            "Born in 1988",
            "Has a 2019 Toyota Corolla",
        ] {
            assert_eq!(not_for_memory(fact), None, "{fact}");
        }
    }

    #[test]
    fn secrets_cards_and_accounts_are_refused() {
        for secret in [
            "My password is hunter2",
            "Bank PIN: 4821",
            "API key = sk-proj-abcdefghijklmnop1234",
            "Their token ghp_abcdefghijklmnopqrstuvwxyz0123",
            "Card 4111 1111 1111 1111",
            "Pays with 4111-1111-1111-1111",
            "IBAN MD24 AG00 0225 1000 1310 4168",
            "Account number: 123456789",
        ] {
            assert!(not_for_memory(secret).is_some(), "{secret}");
        }
    }
}
