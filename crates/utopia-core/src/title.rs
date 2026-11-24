use unicode_normalization::UnicodeNormalization;

pub const NORMALIZATION_VERSION: u32 = 1;

pub fn normalize_title(title: &str) -> String {
    let nfkc: String = title.nfkc().collect();
    let folded = caseless::default_case_fold_str(&nfkc);
    let renormalized: String = folded.nfkc().collect();
    renormalized
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn clean_title(title: &str) -> String {
    let nfc: String = title.nfc().collect();
    nfc.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_and_whitespace_collapse() {
        assert_eq!(normalize_title("  Apple   Watch "), "apple watch");
        assert_eq!(
            normalize_title("apple watch"),
            normalize_title("Apple Watch")
        );
        assert_eq!(normalize_title("Apple\tWatch\n"), "apple watch");
    }

    #[test]
    fn ac_4_9_unicode_forms_collide() {
        assert_eq!(normalize_title("Caf\u{e9}"), normalize_title("Cafe\u{301}"));
        assert_eq!(normalize_title("ＡＢＣ"), "abc");
        assert_eq!(normalize_title("Straße"), normalize_title("STRASSE"));
    }

    #[test]
    fn clean_keeps_case() {
        assert_eq!(clean_title("  Modern   Religion "), "Modern Religion");
    }
}
