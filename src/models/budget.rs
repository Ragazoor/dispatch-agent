//! Claude subscription rate-limit windows — the data behind the top-row budget
//! indicator (see docs/specs/dispatch.allium: TokenBudgetIndicator).
//!
//! Deliberately unrelated to `super::usage`: that module counts keybindings and
//! MCP tool calls. These are subscription budget windows.

use serde::{Deserialize, Serialize};

/// One rolling rate-limit window as reported by the statusLine hook payload.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BudgetWindow {
    pub used_percentage: f64,
    /// Unix epoch seconds at which this window resets.
    pub resets_at: i64,
}

impl BudgetWindow {
    /// Percentage constrained to 0..=100. The upstream field is documented as
    /// 0-100 but is not validated here. NaN values are treated as 0.0 to prevent
    /// nonsense colour or text — a missing/garbage reading should read as "no
    /// information", and 0 is the safe end for a used-percentage.
    pub fn clamped_percentage(&self) -> f64 {
        if self.used_percentage.is_nan() {
            0.0
        } else {
            self.used_percentage.clamp(0.0, 100.0)
        }
    }

    fn from_json(value: &serde_json::Value) -> Option<Self> {
        Some(Self {
            used_percentage: value.get("used_percentage")?.as_f64()?,
            resets_at: value.get("resets_at")?.as_i64()?,
        })
    }
}

/// Latest-wins snapshot of the account-global budget windows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<BudgetWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<BudgetWindow>,
    /// Unix epoch seconds at which this snapshot was captured.
    pub captured_at: i64,
}

impl BudgetSnapshot {
    /// Extract the budget windows from a statusLine hook payload.
    ///
    /// Returns `None` when `rate_limits` is absent or carries no usable window —
    /// the normal steady state for API-key and cloud-provider auth, and for a
    /// session that has not yet had an API response. A partially-specified
    /// window is dropped rather than defaulted, so an unknown percentage never
    /// renders as 0%.
    pub fn from_status_payload(payload: &serde_json::Value, captured_at: i64) -> Option<Self> {
        let limits = payload.get("rate_limits")?;
        let five_hour = limits.get("five_hour").and_then(BudgetWindow::from_json);
        let seven_day = limits.get("seven_day").and_then(BudgetWindow::from_json);
        if five_hour.is_none() && seven_day.is_none() {
            return None;
        }
        Some(Self {
            five_hour,
            seven_day,
            captured_at,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_both_windows() {
        let payload = json!({
            "rate_limits": {
                "five_hour": { "used_percentage": 23.5, "resets_at": 1738425600_i64 },
                "seven_day": { "used_percentage": 41.2, "resets_at": 1738857600_i64 }
            }
        });
        let snap = BudgetSnapshot::from_status_payload(&payload, 1738421000).unwrap();
        assert_eq!(snap.five_hour.unwrap().used_percentage, 23.5);
        assert_eq!(snap.seven_day.unwrap().resets_at, 1738857600);
        assert_eq!(snap.captured_at, 1738421000);
    }

    #[test]
    fn parses_five_hour_only() {
        let payload = json!({
            "rate_limits": {
                "five_hour": { "used_percentage": 10.0, "resets_at": 1_i64 }
            }
        });
        let snap = BudgetSnapshot::from_status_payload(&payload, 0).unwrap();
        assert!(snap.five_hour.is_some());
        assert!(snap.seven_day.is_none());
    }

    #[test]
    fn absent_rate_limits_is_none() {
        // API-key and cloud-provider auth never emit rate_limits at all.
        let payload = json!({ "model": { "display_name": "Opus" } });
        assert!(BudgetSnapshot::from_status_payload(&payload, 0).is_none());
    }

    #[test]
    fn empty_rate_limits_is_none() {
        let payload = json!({ "rate_limits": {} });
        assert!(BudgetSnapshot::from_status_payload(&payload, 0).is_none());
    }

    #[test]
    fn window_missing_fields_is_skipped_not_defaulted() {
        // A window without used_percentage must not become 0% — that would
        // read as "plenty left" when we simply do not know.
        let payload = json!({
            "rate_limits": { "five_hour": { "resets_at": 5_i64 } }
        });
        assert!(BudgetSnapshot::from_status_payload(&payload, 0).is_none());
    }

    #[test]
    fn clamps_percentage_out_of_range() {
        let high = BudgetWindow {
            used_percentage: 137.0,
            resets_at: 0,
        };
        let low = BudgetWindow {
            used_percentage: -4.0,
            resets_at: 0,
        };
        assert_eq!(high.clamped_percentage(), 100.0);
        assert_eq!(low.clamped_percentage(), 0.0);
    }

    #[test]
    fn clamps_nan_to_zero() {
        let nan_window = BudgetWindow {
            used_percentage: f64::NAN,
            resets_at: 0,
        };
        assert_eq!(nan_window.clamped_percentage(), 0.0);
    }

    #[test]
    fn round_trips_through_json() {
        let snap = BudgetSnapshot {
            five_hour: Some(BudgetWindow {
                used_percentage: 1.5,
                resets_at: 2,
            }),
            seven_day: None,
            captured_at: 3,
        };
        let text = serde_json::to_string(&snap).unwrap();
        let back: BudgetSnapshot = serde_json::from_str(&text).unwrap();
        assert_eq!(snap, back);
    }
}
