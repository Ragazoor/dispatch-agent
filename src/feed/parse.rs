use crate::models::FeedItem;

/// Deserialise a JSON byte slice as a `Vec<FeedItem>`.
///
/// The SINGLE feed-stdout decode, called by all three feed entry points:
/// [`crate::feed::FeedCycle::run`] (which both the auto-poll path and the
/// manual "r" refresh share) and `cmd_verify_feed` (the `verify-feed` CLI).
/// Three separate
/// `serde_json` call sites were three places a change to the feed wire format
/// could land in one path and not the others — the shape that produced the
/// `reviews_parent` parent-flat bug in the sync layer. See `feeds.allium`'s
/// `FeedItemParse` block.
///
/// It holds no validation of its own: strict `tag`/`url_type` rejection and
/// lenient `signals` dropping are properties of [`FeedItem`]'s `Deserialize`
/// impl, so they apply to every caller identically and cannot diverge per path.
/// Presentation of the error stays with each caller.
pub fn parse_feed_items(bytes: &[u8]) -> anyhow::Result<Vec<FeedItem>> {
    serde_json::from_slice(bytes).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn valid_item_parsed() {
        let json =
            br#"[{"external_id":"1","title":"T","description":"D","status":"backlog","tag":"bug"}]"#;
        let items = parse_feed_items(json).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "T");
        assert_eq!(items[0].external_id, "1");
    }

    #[test]
    fn empty_array_parsed() {
        let items = parse_feed_items(b"[]").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn missing_required_tag_fails() {
        let json = br#"[{"external_id":"1","title":"T","description":"","status":"backlog"}]"#;
        assert!(
            parse_feed_items(json).is_err(),
            "missing tag must fail deserialization"
        );
    }

    #[test]
    fn malformed_json_fails() {
        assert!(
            parse_feed_items(b"not-json").is_err(),
            "malformed JSON must fail"
        );
    }

    #[test]
    fn explicit_url_type_parsed_verbatim() {
        let json = br#"[{
            "external_id": "dependabot:org/repo#7",
            "title": "CVE-2026-1234",
            "description": "",
            "url": "https://github.com/org/repo/security/dependabot/7",
            "url_type": "security_alert",
            "status": "backlog",
            "tag": "fix"
        }]"#;
        let items = parse_feed_items(json).unwrap();
        assert_eq!(
            items[0].url_type,
            Some(crate::models::UrlType::SecurityAlert)
        );
    }

    #[test]
    fn omitted_url_type_defaults_to_none() {
        let json =
            br#"[{"external_id":"1","title":"T","description":"D","status":"backlog","tag":"bug"}]"#;
        let items = parse_feed_items(json).unwrap();
        assert_eq!(items[0].url_type, None, "wire compatibility: absent field");
    }

    #[test]
    fn unknown_url_type_fails() {
        let json = br#"[{"external_id":"1","title":"T","description":"","url_type":"bogus","status":"backlog","tag":"bug"}]"#;
        assert!(
            parse_feed_items(json).is_err(),
            "unknown url_type must fail deserialization, consistent with tag"
        );
    }

    #[test]
    fn unrecognised_signal_dropped_not_fatal() {
        // The single-item invariant lives with the type, in
        // src/models/tasks.rs::feed_item_signals_default_empty_and_unknown_skipped;
        // this asserts it survives the array path every feed caller uses.
        let json = br#"[{
            "external_id": "1",
            "title": "T",
            "description": "",
            "status": "backlog",
            "tag": "pr-review",
            "signals": ["reviewed", "bogus"]
        }]"#;
        let items = parse_feed_items(json).unwrap();
        assert_eq!(items.len(), 1, "an unknown signal must not fail the item");
        assert_eq!(
            items[0].signals,
            vec![crate::models::Signal::Reviewed],
            "the unrecognised signal is dropped, the recognised one kept"
        );
    }

    #[test]
    fn author_label_at_prefix_preserved() {
        let json = br##"[{
            "external_id": "review:org/repo#7",
            "title": "#7 My PR",
            "description": "",
            "url": "https://github.com/org/repo/pull/7",
            "status": "backlog",
            "tag": "pr-review",
            "labels": ["@johndoe", "repo"]
        }]"##;
        let items = parse_feed_items(json).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].labels,
            vec!["@johndoe".to_string(), "repo".to_string()]
        );
    }
}
