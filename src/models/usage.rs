#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageCategory {
    Keybinding,
    McpTool,
}

impl UsageCategory {
    pub const ALL: &'static [UsageCategory] = &[UsageCategory::Keybinding, UsageCategory::McpTool];

    pub fn as_str(self) -> &'static str {
        match self {
            UsageCategory::Keybinding => "keybinding",
            UsageCategory::McpTool => "mcp_tool",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "keybinding" => Some(Self::Keybinding),
            "mcp_tool" => Some(Self::McpTool),
            _ => None,
        }
    }
}

impl std::fmt::Display for UsageCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageActor {
    Human,
    Agent,
}

impl UsageActor {
    pub const ALL: &'static [UsageActor] = &[UsageActor::Human, UsageActor::Agent];

    pub fn as_str(self) -> &'static str {
        match self {
            UsageActor::Human => "human",
            UsageActor::Agent => "agent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "human" => Some(Self::Human),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

impl std::fmt::Display for UsageActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An event to record in the usage_events table.
#[derive(Debug, Clone)]
pub struct UsageEvent {
    pub category: UsageCategory,
    /// Snake-case name of the action, e.g. "dispatch_task", "create_task".
    pub action: String,
    /// Key char ('d') for keybindings, tool name for MCP tools. None if not applicable.
    pub detail: Option<String>,
    pub actor: UsageActor,
}

/// Aggregated usage row returned by query_usage.
#[derive(Debug, Clone)]
pub struct UsageSummary {
    pub category: String,
    pub action: String,
    pub detail: Option<String>,
    pub actor: String,
    pub count: i64,
    pub last_used: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `UsageCategory::ALL`/`UsageActor::ALL` back the query_usage MCP
    /// schema's category/actor enums (dispatch.rs) — a variant added there
    /// without updating `ALL` would silently under-advertise it.
    #[test]
    fn usage_category_and_actor_all_have_every_variant() {
        assert_eq!(UsageCategory::ALL.len(), 2);
        assert_eq!(UsageActor::ALL.len(), 2);
    }

    #[test]
    fn usage_category_roundtrip() {
        for (cat, s) in [
            (UsageCategory::Keybinding, "keybinding"),
            (UsageCategory::McpTool, "mcp_tool"),
        ] {
            assert_eq!(cat.as_str(), s);
            assert_eq!(cat.to_string(), s);
            assert_eq!(UsageCategory::parse(s), Some(cat));
        }
        assert_eq!(UsageCategory::parse("nonsense"), None);
    }

    #[test]
    fn usage_actor_roundtrip() {
        for (actor, s) in [(UsageActor::Human, "human"), (UsageActor::Agent, "agent")] {
            assert_eq!(actor.as_str(), s);
            assert_eq!(actor.to_string(), s);
            assert_eq!(UsageActor::parse(s), Some(actor));
        }
        assert_eq!(UsageActor::parse("nonsense"), None);
    }
}
