//! Pure moderation domain: value objects, the deterministic matcher, the transparent
//! statistical signal, and the appeal state machine. No `sqlx`, no `axum` — everything
//! here is synchronous, side-effect-free, and unit-tested (TESTING.md unit layer).
//!
//! Auditability is the whole point (PLAN.md correction #3, principle 11): a decision is
//! always explainable by naming the rule that fired and the value it matched, never an
//! opaque third-party score.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A value that came from outside the system and failed validation (mapped to
/// [`dsoc_core::Error::Validation`]) or that was read back corrupt from storage.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid moderation value for {field}: {value}")]
pub struct ParseError {
    /// The logical field that failed to parse (never sensitive).
    pub field: &'static str,
    /// The offending value (echoed back for the caller's diagnostics).
    pub value: String,
}

impl ParseError {
    fn new(field: &'static str, value: impl Into<String>) -> Self {
        Self {
            field,
            value: value.into(),
        }
    }
}

/// Which matcher a [`Rule`] uses. Both are fully transparent and reproducible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    /// Case-insensitive substring match of `pattern` against the content.
    Keyword,
    /// Statistical signal: flags "shouting" when the uppercase-letter ratio of the
    /// content reaches the threshold stored in `pattern` (a value in `0.0..=1.0`).
    CapsRatio,
}

impl RuleKind {
    /// The stable wire/storage token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            RuleKind::Keyword => "keyword",
            RuleKind::CapsRatio => "caps_ratio",
        }
    }
}

impl FromStr for RuleKind {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "keyword" => Ok(RuleKind::Keyword),
            "caps_ratio" => Ok(RuleKind::CapsRatio),
            other => Err(ParseError::new("kind", other)),
        }
    }
}

impl fmt::Display for RuleKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The prescribed response when a rule matches. Recorded for audit; the evaluation
/// outcome itself is always [`Outcome::Flagged`] on a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleAction {
    /// Soft signal: keep the content but mark it for human review.
    Flag,
    /// Hard signal: the content should be withheld pending appeal.
    Reject,
}

impl RuleAction {
    /// The stable wire/storage token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            RuleAction::Flag => "flag",
            RuleAction::Reject => "reject",
        }
    }
}

impl FromStr for RuleAction {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "flag" => Ok(RuleAction::Flag),
            "reject" => Ok(RuleAction::Reject),
            other => Err(ParseError::new("action", other)),
        }
    }
}

impl fmt::Display for RuleAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The outcome of evaluating content against the ruleset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// At least one rule matched.
    Flagged,
    /// No rule matched.
    Cleared,
}

impl Outcome {
    /// The stable wire/storage token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Outcome::Flagged => "flagged",
            Outcome::Cleared => "cleared",
        }
    }

    /// Whether this outcome flagged the content.
    #[must_use]
    pub const fn is_flagged(self) -> bool {
        matches!(self, Outcome::Flagged)
    }
}

impl FromStr for Outcome {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "flagged" => Ok(Outcome::Flagged),
            "cleared" => Ok(Outcome::Cleared),
            other => Err(ParseError::new("outcome", other)),
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The kind of artifact a decision is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// A proposal (consumes `proposals.created`).
    Proposal,
    /// A comment in a deliberation thread (consumes `comments.created`).
    Comment,
}

impl TargetKind {
    /// The stable wire/storage token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            TargetKind::Proposal => "proposal",
            TargetKind::Comment => "comment",
        }
    }
}

impl FromStr for TargetKind {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proposal" => Ok(TargetKind::Proposal),
            "comment" => Ok(TargetKind::Comment),
            other => Err(ParseError::new("target_kind", other)),
        }
    }
}

impl fmt::Display for TargetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lifecycle of an appeal: `Open -> Granted | Denied`. Terminal states reject further
/// transitions, so an appeal cannot be silently re-decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppealStatus {
    /// Filed, awaiting a human decision.
    Open,
    /// Upheld — the original moderation decision is overturned.
    Granted,
    /// Rejected — the original moderation decision stands.
    Denied,
}

impl AppealStatus {
    /// The stable wire/storage token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            AppealStatus::Open => "open",
            AppealStatus::Granted => "granted",
            AppealStatus::Denied => "denied",
        }
    }

    /// Whether this is a terminal (resolved) state.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, AppealStatus::Granted | AppealStatus::Denied)
    }

    /// Attempt to move to a new status. Only `Open -> Granted` and `Open -> Denied`
    /// are legal; everything else is a [`TransitionError`] so an already resolved
    /// appeal can never be flipped.
    ///
    /// # Errors
    /// Returns [`TransitionError`] when the source state is terminal or the target is
    /// not a resolution.
    pub fn transition(self, to: AppealStatus) -> Result<AppealStatus, TransitionError> {
        match (self, to) {
            (AppealStatus::Open, AppealStatus::Granted | AppealStatus::Denied) => Ok(to),
            (from, to) => Err(TransitionError { from, to }),
        }
    }
}

impl FromStr for AppealStatus {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "open" => Ok(AppealStatus::Open),
            "granted" => Ok(AppealStatus::Granted),
            "denied" => Ok(AppealStatus::Denied),
            other => Err(ParseError::new("status", other)),
        }
    }
}

impl fmt::Display for AppealStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An illegal appeal state transition (mapped to [`dsoc_core::Error::Conflict`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("illegal appeal transition: {from} -> {to}")]
pub struct TransitionError {
    /// The current (source) status.
    pub from: AppealStatus,
    /// The rejected target status.
    pub to: AppealStatus,
}

/// A single deterministic moderation rule, as persisted in `moderation_rule`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Rule id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Which matcher to run.
    pub kind: RuleKind,
    /// The matcher's auditable parameter (keyword text or caps-ratio threshold).
    pub pattern: String,
    /// The prescribed response on a match.
    pub action: RuleAction,
    /// Creation time (from the injected clock).
    pub created_at: DateTime<Utc>,
}

impl Rule {
    /// Whether this rule matches `content`. Pure and reproducible — the basis of audit.
    #[must_use]
    pub fn matches(&self, content: &str) -> bool {
        match self.kind {
            RuleKind::Keyword => {
                let needle = self.pattern.trim().to_lowercase();
                !needle.is_empty() && content.to_lowercase().contains(&needle)
            }
            RuleKind::CapsRatio => match self.pattern.trim().parse::<f64>() {
                // A non-finite or out-of-range threshold can never sensibly fire.
                Ok(threshold) if (0.0..=1.0).contains(&threshold) => {
                    uppercase_ratio(content) >= threshold
                }
                _ => false,
            },
        }
    }
}

/// The transparent statistical signal: the fraction of alphabetic characters that are
/// uppercase, in `0.0..=1.0`. Content with no letters scores `0.0` (nothing to shout).
#[must_use]
pub fn uppercase_ratio(content: &str) -> f64 {
    let mut letters = 0u64;
    let mut upper = 0u64;
    for c in content.chars().filter(|c| c.is_alphabetic()) {
        letters += 1;
        if c.is_uppercase() {
            upper += 1;
        }
    }
    if letters == 0 {
        0.0
    } else {
        // Exact: both counts are <= content length, well within f64's integer range.
        upper as f64 / letters as f64
    }
}

/// Evaluate `content` against an ordered ruleset and return the first matching rule, if
/// any. Order is significant and deterministic (oldest rule wins) so decisions are
/// reproducible from the audit log.
#[must_use]
pub fn first_match<'a>(rules: &'a [Rule], content: &str) -> Option<&'a Rule> {
    rules.iter().find(|rule| rule.matches(content))
}

/// Map a matching rule (or its absence) to a decision outcome. `Some(rule)` ⇒ flagged;
/// `None` ⇒ cleared. Keeping this explicit guarantees every evaluation yields exactly
/// one auditable outcome (decisions are never silently dropped).
#[must_use]
pub fn outcome_for(matched: Option<&Rule>) -> Outcome {
    match matched {
        Some(_) => Outcome::Flagged,
        None => Outcome::Cleared,
    }
}

/// A persisted moderation decision (`moderation_decision`): the audit record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// Decision id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// What kind of artifact was judged.
    pub target_kind: TargetKind,
    /// The judged artifact's id.
    pub target_id: Uuid,
    /// The rule that fired (`None` when cleared).
    pub rule_id: Option<Uuid>,
    /// Flagged or cleared.
    pub outcome: Outcome,
    /// Creation time (from the injected clock).
    pub created_at: DateTime<Utc>,
}

/// A persisted appeal (`moderation_appeal`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appeal {
    /// Appeal id.
    pub id: Uuid,
    /// The decision being challenged.
    pub decision_id: Uuid,
    /// The citizen's stated reason.
    pub reason: String,
    /// Current lifecycle state.
    pub status: AppealStatus,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last transition time.
    pub updated_at: DateTime<Utc>,
}

/// Validate free-text input at the system boundary (coding-style: validate at boundaries).
/// Trims, rejects empty, and caps length to a sane bound to avoid unbounded storage.
///
/// # Errors
/// Returns [`ParseError`] when the trimmed value is empty or exceeds `max_len`.
pub fn validate_nonempty(
    field: &'static str,
    value: &str,
    max_len: usize,
) -> Result<String, ParseError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > max_len {
        return Err(ParseError::new(field, value));
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn rule(kind: RuleKind, pattern: &str) -> Rule {
        Rule {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            kind,
            pattern: pattern.to_owned(),
            action: RuleAction::Flag,
            created_at: at(),
        }
    }

    #[test]
    fn rule_kind_roundtrips() {
        for k in [RuleKind::Keyword, RuleKind::CapsRatio] {
            assert_eq!(k.as_str().parse::<RuleKind>().unwrap(), k);
            assert_eq!(k.to_string(), k.as_str());
        }
        assert!("nope".parse::<RuleKind>().is_err());
    }

    #[test]
    fn rule_action_roundtrips() {
        for a in [RuleAction::Flag, RuleAction::Reject] {
            assert_eq!(a.as_str().parse::<RuleAction>().unwrap(), a);
            assert_eq!(a.to_string(), a.as_str());
        }
        let err = "burn".parse::<RuleAction>().unwrap_err();
        assert_eq!(err.field, "action");
        assert!(err.to_string().contains("burn"));
    }

    #[test]
    fn outcome_roundtrips_and_reports_flagged() {
        assert!(Outcome::Flagged.is_flagged());
        assert!(!Outcome::Cleared.is_flagged());
        for o in [Outcome::Flagged, Outcome::Cleared] {
            assert_eq!(o.as_str().parse::<Outcome>().unwrap(), o);
            assert_eq!(o.to_string(), o.as_str());
        }
        assert!("maybe".parse::<Outcome>().is_err());
    }

    #[test]
    fn target_kind_roundtrips() {
        for t in [TargetKind::Proposal, TargetKind::Comment] {
            assert_eq!(t.as_str().parse::<TargetKind>().unwrap(), t);
            assert_eq!(t.to_string(), t.as_str());
        }
        assert!("vote".parse::<TargetKind>().is_err());
    }

    #[test]
    fn appeal_status_roundtrips_and_terminality() {
        for s in [
            AppealStatus::Open,
            AppealStatus::Granted,
            AppealStatus::Denied,
        ] {
            assert_eq!(s.as_str().parse::<AppealStatus>().unwrap(), s);
            assert_eq!(s.to_string(), s.as_str());
        }
        assert!(!AppealStatus::Open.is_terminal());
        assert!(AppealStatus::Granted.is_terminal());
        assert!(AppealStatus::Denied.is_terminal());
        assert!("pending".parse::<AppealStatus>().is_err());
    }

    #[test]
    fn appeal_transitions_only_from_open() {
        assert_eq!(
            AppealStatus::Open
                .transition(AppealStatus::Granted)
                .unwrap(),
            AppealStatus::Granted
        );
        assert_eq!(
            AppealStatus::Open.transition(AppealStatus::Denied).unwrap(),
            AppealStatus::Denied
        );
    }

    #[test]
    fn appeal_cannot_be_redecided_or_reopened() {
        let err = AppealStatus::Granted
            .transition(AppealStatus::Denied)
            .unwrap_err();
        assert_eq!(err.from, AppealStatus::Granted);
        assert!(err.to_string().contains("granted -> denied"));
        assert!(AppealStatus::Denied
            .transition(AppealStatus::Granted)
            .is_err());
        // Open -> Open is not a resolution.
        assert!(AppealStatus::Open.transition(AppealStatus::Open).is_err());
    }

    #[test]
    fn keyword_rule_matches_case_insensitively() {
        let r = rule(RuleKind::Keyword, "Golpe");
        assert!(r.matches("isto é um GOLPE contra a democracia"));
        assert!(r.matches("golpe"));
        assert!(!r.matches("conteúdo cívico legítimo"));
    }

    #[test]
    fn keyword_rule_with_blank_pattern_never_matches() {
        let r = rule(RuleKind::Keyword, "   ");
        assert!(!r.matches("anything at all"));
    }

    #[test]
    fn caps_ratio_signal_is_exact() {
        assert_eq!(uppercase_ratio(""), 0.0);
        assert_eq!(uppercase_ratio("12345 !!!"), 0.0); // no letters
        assert_eq!(uppercase_ratio("ABCD"), 1.0);
        assert_eq!(uppercase_ratio("abcd"), 0.0);
        assert!((uppercase_ratio("AAbb") - 0.5).abs() < f64::EPSILON);
        // Digits and punctuation are ignored, only letters count.
        assert!((uppercase_ratio("Ab 99!") - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn caps_ratio_rule_flags_shouting() {
        let r = rule(RuleKind::CapsRatio, "0.8");
        assert!(r.matches("PARE COM ISSO AGORA"));
        assert!(!r.matches("uma frase calma e normal"));
    }

    #[test]
    fn caps_ratio_rule_rejects_nonsense_threshold() {
        assert!(!rule(RuleKind::CapsRatio, "not-a-number").matches("ABC"));
        assert!(!rule(RuleKind::CapsRatio, "2.0").matches("ABC")); // out of range
        assert!(!rule(RuleKind::CapsRatio, "-0.5").matches("ABC"));
    }

    #[test]
    fn first_match_respects_order_and_clears_clean_content() {
        let rules = vec![
            rule(RuleKind::Keyword, "spam"),
            rule(RuleKind::Keyword, "scam"),
        ];
        let matched = first_match(&rules, "this is spam and scam").unwrap();
        // Oldest-first precedence: the first rule wins.
        assert_eq!(matched.pattern, "spam");
        assert!(first_match(&rules, "clean civic text").is_none());
    }

    #[test]
    fn outcome_for_maps_match_to_flagged() {
        let r = rule(RuleKind::Keyword, "x");
        assert_eq!(outcome_for(Some(&r)), Outcome::Flagged);
        assert_eq!(outcome_for(None), Outcome::Cleared);
    }

    #[test]
    fn empty_ruleset_always_clears() {
        assert!(first_match(&[], "anything").is_none());
        assert_eq!(outcome_for(first_match(&[], "anything")), Outcome::Cleared);
    }

    #[test]
    fn validate_nonempty_trims_and_bounds() {
        assert_eq!(validate_nonempty("reason", "  hi  ", 10).unwrap(), "hi");
        assert!(validate_nonempty("reason", "   ", 10).is_err());
        assert!(validate_nonempty("reason", "toolong", 3).is_err());
        let err = validate_nonempty("reason", "", 10).unwrap_err();
        assert_eq!(err.field, "reason");
    }
}
