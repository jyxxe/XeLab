//! Stateless planning for an explicitly supplied missed interval.
use super::{fire_datetime, schedule_timestamp, CronSchedule, PreviewWarning, SEARCH_YEARS};
use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use jiff::{tz::Offset, Timestamp, ToSpan};
use serde::Serialize;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MisfirePolicy {
    Skip,
    RunOnce,
}

impl FromStr for MisfirePolicy {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "skip" => Ok(Self::Skip),
            "run_once" => Ok(Self::RunOnce),
            _ => Err(Error::invalid("Expected misfire policy skip or run_once")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlapPolicy {
    Skip,
    QueueOne,
}

impl FromStr for OverlapPolicy {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "skip" => Ok(Self::Skip),
            "queue_one" => Ok(Self::QueueOne),
            _ => Err(Error::invalid("Expected overlap policy skip or queue_one")),
        }
    }
}

pub struct RecoveryInputs {
    /// The last instant already considered, excluded from the missed window.
    pub after: DateTime<Utc>,
    /// The recovery cutoff, included in the missed window.
    pub through: DateTime<Utc>,
    pub misfire: MisfirePolicy,
    pub overlap: OverlapPolicy,
    pub has_active_task: bool,
    /// A pending UTC occurrence from this exact schedule revision.
    pub pending_at: Option<DateTime<Utc>>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct RecoveryPlan {
    pub timezone: String,
    pub latest_missed_at: Option<DateTime<Utc>>,
    #[serde(flatten)]
    pub decision: RecoveryDecision,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub warnings: Vec<PreviewWarning>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum RecoveryDecision {
    Idle,
    Ready { scheduled_at: DateTime<Utc> },
    Queued { scheduled_at: DateTime<Utc> },
    Skipped { reason: SkipReason },
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    Misfire,
    Overlap,
}

impl CronSchedule {
    pub fn plan_recovery(&self, input: RecoveryInputs) -> Result<RecoveryPlan> {
        if input.after > input.through {
            return Err(Error::invalid("Schedule recovery bounds are reversed"));
        }
        let after = schedule_timestamp(input.after)?;
        let through = schedule_timestamp(input.through)?;
        if let Some(pending) = input.pending_at {
            if input.overlap != OverlapPolicy::QueueOne || pending > input.after {
                return Err(Error::invalid(
                    "Pending occurrence requires queue_one and must be at or before --after",
                ));
            }
            let instant = schedule_timestamp(pending)?;
            let local = self.timezone.to_datetime(instant);
            if pending.timestamp_subsec_nanos() != 0
                || !self.matches_date(local.date())
                || self.times.binary_search(&local.time()).is_err()
                || self.first_instant(local.date(), local.time()) != Some(instant)
            {
                return Err(Error::invalid(
                    "Pending timestamp is not an occurrence of this schedule",
                ));
            }
        }

        let latest_missed_at = self.latest_missed(after, through)?.map(fire_datetime);
        let selected = match input.misfire {
            MisfirePolicy::Skip => input.pending_at,
            MisfirePolicy::RunOnce => input.pending_at.max(latest_missed_at),
        };
        let decision = match (selected, input.has_active_task, input.overlap) {
            (Some(scheduled_at), false, _) => RecoveryDecision::Ready { scheduled_at },
            (Some(scheduled_at), true, OverlapPolicy::QueueOne) => {
                RecoveryDecision::Queued { scheduled_at }
            }
            (Some(_), true, OverlapPolicy::Skip) => RecoveryDecision::Skipped {
                reason: SkipReason::Overlap,
            },
            (None, _, _) if latest_missed_at.is_some() => RecoveryDecision::Skipped {
                reason: SkipReason::Misfire,
            },
            (None, _, _) => RecoveryDecision::Idle,
        };
        let next = self.preview(input.through, 1)?;
        Ok(RecoveryPlan {
            timezone: next.timezone,
            latest_missed_at,
            decision,
            next_fire_at: next.fire_at.into_iter().next(),
            warnings: next.warnings,
        })
    }

    fn latest_missed(&self, after: Timestamp, through: Timestamp) -> Result<Option<Timestamp>> {
        if after == through {
            return Ok(None);
        }
        let start = Offset::MAX.to_datetime(through).date();
        let end = Offset::MIN.to_datetime(after).date();
        let search_limit = start.saturating_add((-SEARCH_YEARS).years());
        let mut latest: Option<Timestamp> = None;
        for date in start.series((-1).day()).take_while(|date| *date >= end) {
            // Cross-date offset jumps can reverse UTC order. Stop only when
            // no earlier local date could contain a later eligible instant.
            if latest.is_some_and(|last| {
                Offset::MIN
                    .to_timestamp(date.at(23, 59, 59, 0))
                    .is_ok_and(|latest_possible| latest_possible <= last)
            }) {
                break;
            }
            if date < search_limit {
                return Err(Error::new(
                    "RESOURCE_EXHAUSTED",
                    "Schedule recovery search limit reached before checking the whole interval",
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                ));
            }
            if self.matches_date(date) {
                latest = self
                    .instants_on(date)
                    .filter(|instant| *instant > after && *instant <= through)
                    .max()
                    .max(latest);
            }
        }
        Ok(latest)
    }
}
