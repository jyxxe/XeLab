use serde_json::{json, Value};
use std::process::{Command, Output};

fn preview(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xelabctl"))
        .arg("preview-schedule")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn schedule_preview_exposes_default_zone_utc_instants_and_day_warning() {
    let output = preview(&[
        "--cron",
        "0 8 1 * 1",
        "--after",
        "2026-02-01T08:00:00+08:00",
        "--count",
        "2",
    ]);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stderr.is_empty());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result,
        json!({
            "timezone": "Asia/Shanghai",
            "fire_at": ["2026-02-02T00:00:00Z", "2026-02-09T00:00:00Z"],
            "warnings": ["DAY_OF_MONTH_OR_DAY_OF_WEEK"],
        })
    );
}

#[test]
fn invalid_schedule_preview_inputs_use_the_cli_argument_error() {
    for args in [
        ["--cron", "0 12 * * MON"],
        ["--timezone", "+08:00"],
        ["--after", "2026-02-01T00:00:00"],
        ["--count", "0"],
        ["--count", "101"],
    ] {
        let mut flags = vec!["--cron", "* * * * *", "--after", "2026-02-01T00:00:00Z"];
        // Replace the default value when testing that option.
        if let Some(index) = flags.iter().position(|flag| *flag == args[0]) {
            flags[index + 1] = args[1];
        } else {
            flags.extend(args);
        }
        let output = preview(&flags);
        assert_eq!(output.status.code(), Some(2), "{flags:?}");
        assert!(output.stdout.is_empty(), "{flags:?}");
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .starts_with("INVALID_ARGUMENT: "),
            "{flags:?}"
        );
    }
}
