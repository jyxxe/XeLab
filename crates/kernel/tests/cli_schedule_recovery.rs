use serde_json::{json, Value};
use std::process::{Command, Output};

fn plan(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xelabctl"))
        .arg("plan-schedule-recovery")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn recovery_cli_returns_a_reproducible_queue_decision() {
    let args = [
        "--cron",
        "*/5 * * * *",
        "--after",
        "2026-01-01T14:00:00+08:00",
        "--through",
        "2026-01-01T14:11:01+08:00",
        "--misfire",
        "run_once",
        "--overlap",
        "queue_one",
        "--has-active-task",
        "--pending-at",
        "2026-01-01T06:00:00Z",
    ];
    let output = plan(&args);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stderr.is_empty());
    assert_eq!(plan(&args).stdout, output.stdout);
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({
            "timezone": "Asia/Shanghai",
            "latest_missed_at": "2026-01-01T06:10:00Z",
            "decision": "queued",
            "scheduled_at": "2026-01-01T06:10:00Z",
            "next_fire_at": "2026-01-01T06:15:00Z",
            "warnings": []
        })
    );
}

#[test]
fn recovery_cli_rejects_incoherent_windows_and_pending_occurrences() {
    let baseline = [
        "--cron",
        "30 1 * * *",
        "--timezone",
        "America/New_York",
        "--after",
        "2026-11-01T07:00:00Z",
        "--through",
        "2026-11-01T08:00:00Z",
        "--overlap",
        "queue_one",
    ];
    for replacements in [
        vec!["--through", "2026-10-31T00:00:00Z"],
        vec!["--pending-at", "2026-11-01T06:30:00Z"], // Second fold is not canonical.
        vec!["--pending-at", "2026-11-02T06:30:00Z"], // Later than the checkpoint.
        vec!["--pending-at", "2026-11-01T05:30:00.1Z"], // Not a fire instant.
        vec!["--overlap", "skip", "--pending-at", "2026-11-01T05:30:00Z"],
        vec!["--misfire", "run_all"],
    ] {
        let mut args = baseline.to_vec();
        for pair in replacements.chunks_exact(2) {
            if let Some(index) = args.iter().position(|flag| *flag == pair[0]) {
                args[index + 1] = pair[1];
            } else {
                args.extend_from_slice(pair);
            }
        }
        let output = plan(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn recovery_cli_distinguishes_search_exhaustion_from_an_idle_interval() {
    let output = plan(&[
        "--cron",
        "0 0 31 2 *",
        "--timezone",
        "UTC",
        "--after",
        "2000-01-01T00:00:00Z",
        "--through",
        "2601-01-01T00:00:00Z",
    ]);
    assert_eq!(output.status.code(), Some(7));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .starts_with("RESOURCE_EXHAUSTED: "));
}
