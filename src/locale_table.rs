//! The locale table as static data (PLAN.md, performance audit #127).
//!
//! `rust_i18n::i18n!("locales")` expands every string into its own
//! `add_translations` statement, all inside one lazy initializer: about
//! 17,400 statements in a single function. LLVM's register allocator and
//! SLP vectorizer are superlinear in a function that size, so that one
//! closure was three quarters of a release build's optimisation time, and
//! every string added made every release build slower (roughly cubic in
//! the key count). At run time it also rebuilt the whole table on the first
//! `t!()`: 17k HashMaps and 35k Strings, ~4.6 ms and ~2 MB.
//!
//! build.rs now reads locales/*.yml through the macro's own loader
//! (rust-i18n-support's `load_locales`, so the flattening and the key order
//! are exactly the macro's) and writes them out as one sorted `static`.
//! This backend answers from it: a locale scan over ten entries, then a
//! binary search on the key. `t!()`, its interpolation, its per-call
//! locale, its fallback chain and `available_locales!` are all unchanged:
//! they live in the macro's generated code, which still owns the lookup
//! and only asks this backend for a string.

include!(concat!(env!("OUT_DIR"), "/locale_table.rs"));

/// The backend `i18n!` extends in main.rs.
pub struct Table;

impl rust_i18n::Backend for Table {
    fn available_locales(&self) -> Vec<&str> {
        LOCALES.iter().map(|(locale, _)| *locale).collect()
    }

    fn translate(&self, locale: &str, key: &str) -> Option<&str> {
        let (_, keys) = LOCALES.iter().find(|(l, _)| *l == locale)?;
        keys.binary_search_by(|(k, _)| (*k).cmp(key)).ok().map(|at| keys[at].1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_i18n::{Backend, t};

    /// The table is what the macro would have built: every locale, every
    /// key and every string, read back through the backend, against the
    /// macro's own loader run over the same folder.
    #[test]
    fn the_table_answers_exactly_what_the_macro_would_have_loaded() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/locales");
        let loaded = rust_i18n_support::load_locales(dir, |_| false);
        assert!(loaded.len() >= 10, "the loader found the locales folder");
        let mut pairs = 0;
        for (locale, keys) in &loaded {
            for (key, value) in keys {
                assert_eq!(Table.translate(locale, key), Some(value.as_str()), "{locale} {key}");
                pairs += 1;
            }
        }
        let table: usize = LOCALES.iter().map(|(_, keys)| keys.len()).sum();
        assert_eq!(table, pairs, "no key in the table that the loader did not find");
        let mut locales = Table.available_locales();
        locales.sort();
        assert_eq!(locales, loaded.keys().map(String::as_str).collect::<Vec<_>>());
    }

    /// Binary search needs each locale's keys in byte order.
    #[test]
    fn every_locale_is_sorted_for_the_binary_search() {
        for (locale, keys) in LOCALES {
            assert!(keys.windows(2).all(|w| w[0].0 < w[1].0), "{locale}");
        }
    }

    #[test]
    fn t_still_falls_back_to_english_and_through_regions() {
        let en = t!("lang.modal_title", locale = "en");
        assert!(!en.is_empty() && en != "lang.modal_title");
        // An unknown locale and an unknown region both land on a table
        // entry, the way the macro's fallback chain always did.
        assert_eq!(t!("lang.modal_title", locale = "xx"), en);
        assert_eq!(t!("lang.modal_title", locale = "de-AT"), t!("lang.modal_title", locale = "de"));
        // An unknown key comes back as itself.
        assert_eq!(t!("no.such.key", locale = "en"), "no.such.key");
    }
}
