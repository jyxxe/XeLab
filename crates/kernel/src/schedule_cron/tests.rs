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
