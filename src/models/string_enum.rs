//! String-conversion boilerplate for fieldless enums.
//!
//! Several domain enums (`TaskStatus`, `SubStatus`, `TaskTag`, `WrapUpMode`)
//! persist as fixed strings (DB columns, MCP wire values) and each used to
//! hand-roll an identical `as_str`/`parse`/`Display`/`FromStr`
//! quartet. [`define_str_enum!`] generates that quartet from a single
//! variant-to-string table, so adding a variant touches one place instead of
//! four. The enum's own `#[derive(...)]` and serde attributes are untouched —
//! this macro only adds impls on top of an already-declared enum.
//!
//! The macro is `#[macro_export]`ed, so it lives at the crate root. Each
//! consuming module brings it into scope with `use crate::define_str_enum;`.

/// Generate `as_str`/`parse`/`Display`/`FromStr` for a fieldless enum whose
/// variants map 1:1 to a canonical string. Extra `| "alias"` strings parse to
/// the same variant but are never produced by `as_str`/`Display` — use this
/// for backward-compatible input aliases (e.g. `TaskStatus`'s `"ready"` ->
/// `Backlog`). `$err_label` names the type in the `FromStr::Err` message
/// (`"unknown $err_label: {s}"`), matching each enum's pre-macro wording.
#[macro_export]
macro_rules! define_str_enum {
    ($name:ident, $err_label:literal { $($variant:ident => $s:literal $(| $alias:literal)*),+ $(,)? }) => {
        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $s,)+
                }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $($s $(| $alias)* => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s).ok_or_else(|| format!(concat!("unknown ", $err_label, ": {}"), s))
            }
        }
    };
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Fixture {
        Foo,
        Bar,
    }

    crate::define_str_enum!(Fixture, "fixture" {
        Foo => "foo" | "legacy-foo",
        Bar => "bar",
    });

    #[test]
    fn as_str_matches_canonical_string() {
        assert_eq!(Fixture::Foo.as_str(), "foo");
        assert_eq!(Fixture::Bar.as_str(), "bar");
    }

    #[test]
    fn parse_canonical_string() {
        assert_eq!(Fixture::parse("foo"), Some(Fixture::Foo));
        assert_eq!(Fixture::parse("bar"), Some(Fixture::Bar));
    }

    #[test]
    fn parse_alias_maps_to_same_variant_as_canonical() {
        assert_eq!(Fixture::parse("legacy-foo"), Some(Fixture::Foo));
    }

    #[test]
    fn alias_is_not_produced_by_as_str_or_display() {
        assert_eq!(Fixture::Foo.as_str(), "foo");
        assert_eq!(Fixture::Foo.to_string(), "foo");
    }

    #[test]
    fn parse_unknown_is_none() {
        assert_eq!(Fixture::parse("unknown"), None);
    }

    #[test]
    fn display_uses_canonical_string() {
        assert_eq!(Fixture::Foo.to_string(), "foo");
    }

    #[test]
    fn from_str_roundtrip() {
        let parsed: Fixture = "bar".parse().unwrap();
        assert_eq!(parsed, Fixture::Bar);
    }

    #[test]
    fn from_str_error_names_the_type() {
        let err = "nope".parse::<Fixture>().unwrap_err();
        assert_eq!(err, "unknown fixture: nope");
    }
}
