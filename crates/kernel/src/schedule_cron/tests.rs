use super::*;

fn utc(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

fn preview(cron: &str, timezone: &str, after: &str, count: usize) -> SchedulePreview {
    CronSchedule::parse(cron, timezone)
        .unwrap()
        .preview(utc(after), count)
        .unwrap()
}

fn assert_times(preview: &SchedulePreview, expected: &[&str]) {
    assert_eq!(
        preview.fire_at,
        expected.iter().map(|time| utc(time)).collect::<Vec<_>>()
    );
}

#[test]
fn cron_grammar_rejects_out_of_range_and_unsupported_fields() {
    for cron in [
        "",
        "* * * *",
        "* * * * * *",
        "60 * * * *",
        "* 24 * * *",
        "* * 0 * *",
        "* * * 13 *",
        "* * * * 7",
        "* * * JAN *",
        "* * * * MON",
        "* * L * *",
        "* * 15W * *",
        "* * * * 2#1",
        "*/0 * * * *",
        "5/2 * * * *",
        "2-1 * * * *",
        "1-60/2 * * * *",
        "*/ * * * *",
        "* * * * *,",
        "1,,2 * * * *",
        "+2 * * * *",
        "*/-1 * * * *",
        "*/1/2 * * * *",
    ] {
        assert!(CronSchedule::parse(cron, "UTC").is_err(), "{cron:?}");
    }
    for timezone in ["Etc/Unknown", "CST", "+08:00", "Asia/Fake", ""] {
        assert!(CronSchedule::parse("* * * * *", timezone).is_err());
    }
}

#[test]
fn lists_ranges_and_large_steps_produce_unique_ordered_occurrences() {
    let result = preview(
        "5,5,10-14/2,*/99999999999999999999999 8-12/2 * 1-12/2 *",
        "UTC",
        "2026-01-01T08:00:00.001Z",
        6,
    );
    assert_times(
        &result,
        &[
            "2026-01-01T08:05:00Z",
            "2026-01-01T08:10:00Z",
            "2026-01-01T08:12:00Z",
            "2026-01-01T08:14:00Z",
            "2026-01-01T10:00:00Z",
            "2026-01-01T10:05:00Z",
        ],
    );
    assert!(result.warnings.is_empty());
}

#[test]
fn restricted_day_fields_are_combined_with_or() {
    let result = preview("0 9 1 * 1", "UTC", "2026-02-01T08:59:00Z", 3);
    assert_times(
        &result,
        &[
            "2026-02-01T09:00:00Z",
            "2026-02-02T09:00:00Z",
            "2026-02-09T09:00:00Z",
        ],
    );
    assert_eq!(result.warnings, [PreviewWarning::DayOfMonthOrDayOfWeek]);

    let result = preview("0 9 */2 * 0", "UTC", "2026-02-01T09:00:00Z", 1);
    assert_times(&result, &["2026-02-03T09:00:00Z"]);
    assert_eq!(result.warnings, [PreviewWarning::DayOfMonthOrDayOfWeek]);
}

#[test]
fn spring_gap_is_skipped_and_posix_rules_continue_beyond_2100() {
    let result = preview("30 2 * * *", "America/New_York", "2101-03-12T00:00:00Z", 3);
    assert_times(
        &result,
        &[
            "2101-03-12T07:30:00Z",
            "2101-03-14T06:30:00Z",
            "2101-03-15T06:30:00Z",
        ],
    );
}

#[test]
fn fall_fold_uses_first_occurrence_even_with_cutoff_between_repeats() {
    let result = preview("0,30 1 * * *", "US/Eastern", "2026-11-01T04:59:59Z", 3);
    assert_times(
        &result,
        &[
            "2026-11-01T05:00:00Z",
            "2026-11-01T05:30:00Z",
            "2026-11-02T06:00:00Z",
        ],
    );
    let result = preview("30 1 * * *", "US/Eastern", "2026-11-01T05:45:00Z", 1);
    assert_times(&result, &["2026-11-02T06:30:00Z"]);
}

#[test]
fn timezone_resolution_handles_non_hour_offsets_and_calendar_jumps() {
    let result = preview(
        "15 2 * * *",
        "Australia/Lord_Howe",
        "2026-10-02T00:00:00Z",
        2,
    );
    assert_times(&result, &["2026-10-02T15:45:00Z", "2026-10-04T15:15:00Z"]);

    let result = preview("0 9 * * *", "Asia/Kathmandu", "2026-01-01T00:00:00Z", 1);
    assert_times(&result, &["2026-01-01T03:15:00Z"]);

    let result = preview(
        "0 9 * * *",
        "Asia/Kathmandu",
        "1900-01-01T03:18:43.999999Z",
        1,
    );
    assert_times(&result, &["1900-01-01T03:18:44Z"]);
    let result = preview("0 9 * * *", "Asia/Kathmandu", "1900-01-01T03:18:44Z", 1);
    assert_times(&result, &["1900-01-02T03:18:44Z"]);

    let result = preview("0 12 * * *", "Pacific/Apia", "2011-12-29T00:00:00Z", 2);
    assert_times(&result, &["2011-12-29T22:00:00Z", "2011-12-30T22:00:00Z"]);
}

#[test]
fn bounded_search_returns_rare_leap_dates_and_reports_impossible_dates() {
    let result = preview("0 0 29 2 *", "UTC", "2096-02-29T00:00:00Z", 100);
    assert_eq!(result.fire_at.len(), 100);
    assert_eq!(result.fire_at[0], utc("2104-02-29T00:00:00Z"));
    assert_eq!(result.fire_at[99], utc("2512-02-29T00:00:00Z"));
    assert!(result.warnings.is_empty());

    let result = preview("0 0 31 2 *", "UTC", "2026-01-01T00:00:00Z", 2);
    assert!(result.fire_at.is_empty());
    assert_eq!(result.warnings, [PreviewWarning::SearchLimitReached]);
}

#[test]
fn recovery_selects_the_latest_missed_occurrence_across_a_dst_fold() {
    let schedule = CronSchedule::parse("* * * * *", "America/New_York").unwrap();
    let plan = schedule
        .plan_recovery(RecoveryInputs {
            after: utc("2026-10-31T00:00:00Z"),
            through: utc("2026-11-01T06:35:00Z"),
            misfire: MisfirePolicy::RunOnce,
            overlap: OverlapPolicy::Skip,
            has_active_task: false,
            pending_at: None,
        })
        .unwrap();

    let first_fold_last_minute = utc("2026-11-01T05:59:00Z");
    assert_eq!(plan.latest_missed_at, Some(first_fold_last_minute));
    assert_eq!(plan.next_fire_at, Some(utc("2026-11-01T07:00:00Z")));
    assert_eq!(
        plan.decision,
        RecoveryDecision::Ready {
            scheduled_at: first_fold_last_minute,
        }
    );
}

#[test]
fn recovery_composes_misfire_with_overlap_and_an_existing_queue() {
    let schedule = CronSchedule::parse("*/5 * * * *", "UTC").unwrap();
    let after = utc("2026-01-01T14:00:00Z");
    let through = utc("2026-01-01T14:10:00Z");
    let plan = |misfire, overlap, has_active_task, pending_at| {
        schedule
            .plan_recovery(RecoveryInputs {
                after,
                through,
                misfire,
                overlap,
                has_active_task,
                pending_at,
            })
            .unwrap()
    };
    for (misfire, selected) in [
        (MisfirePolicy::Skip, after),
        (MisfirePolicy::RunOnce, through),
    ] {
        assert_eq!(
            plan(misfire, OverlapPolicy::QueueOne, true, Some(after)).decision,
            RecoveryDecision::Queued {
                scheduled_at: selected,
            }
        );
        assert_eq!(
            plan(misfire, OverlapPolicy::QueueOne, false, Some(after)).decision,
            RecoveryDecision::Ready {
                scheduled_at: selected,
            }
        );
    }
    let skipped = plan(MisfirePolicy::Skip, OverlapPolicy::Skip, false, None);
    assert_eq!(skipped.latest_missed_at, Some(through));
    assert_eq!(skipped.next_fire_at, Some(utc("2026-01-01T14:15:00Z")));
    assert_eq!(
        skipped.decision,
        RecoveryDecision::Skipped {
            reason: SkipReason::Misfire,
        }
    );
    assert_eq!(
        plan(MisfirePolicy::RunOnce, OverlapPolicy::Skip, true, None).decision,
        RecoveryDecision::Skipped {
            reason: SkipReason::Overlap,
        }
    );
}

#[test]
fn recovery_skips_dst_gaps_and_respects_subsecond_utc_cutoffs() {
    let schedule = CronSchedule::parse("30 2 * * *", "America/New_York").unwrap();
    let plan = schedule
        .plan_recovery(RecoveryInputs {
            after: utc("2026-03-07T07:30:00Z"),
            through: utc("2026-03-08T08:00:00Z"),
            misfire: MisfirePolicy::RunOnce,
            overlap: OverlapPolicy::Skip,
            has_active_task: false,
            pending_at: None,
        })
        .unwrap();
    assert_eq!(plan.decision, RecoveryDecision::Idle);
    assert_eq!(plan.latest_missed_at, None);
    assert_eq!(plan.next_fire_at, Some(utc("2026-03-09T06:30:00Z")));

    let schedule = CronSchedule::parse("0 9 * * *", "Asia/Kathmandu").unwrap();
    let plan = |after, through| {
        schedule
            .plan_recovery(RecoveryInputs {
                after: utc(after),
                through: utc(through),
                misfire: MisfirePolicy::RunOnce,
                overlap: OverlapPolicy::Skip,
                has_active_task: false,
                pending_at: None,
            })
            .unwrap()
    };
    let fire = "1900-01-01T03:18:44Z";
    assert_eq!(
        plan("1900-01-01T03:18:43.999999Z", fire).latest_missed_at,
        Some(utc(fire))
    );
    assert_eq!(plan(fire, "1900-01-01T03:18:44.1Z").latest_missed_at, None);
}

#[test]
fn recovery_distinguishes_no_occurrence_from_exhausted_lookback() {
    let schedule = CronSchedule::parse("0 0 31 2 *", "UTC").unwrap();
    let plan = |after| {
        schedule.plan_recovery(RecoveryInputs {
            after: utc(after),
            through: utc("2601-01-01T00:00:00Z"),
            misfire: MisfirePolicy::RunOnce,
            overlap: OverlapPolicy::Skip,
            has_active_task: false,
            pending_at: None,
        })
    };
    let no_matches = plan("2600-01-01T00:00:00Z").unwrap();
    assert_eq!(no_matches.decision, RecoveryDecision::Idle);
    assert_eq!(no_matches.latest_missed_at, None);
    assert_eq!(no_matches.next_fire_at, None);
    assert_eq!(no_matches.warnings, [PreviewWarning::SearchLimitReached]);
    assert_eq!(
        plan("2000-01-01T00:00:00Z").unwrap_err().code,
        "RESOURCE_EXHAUSTED"
    );
}
