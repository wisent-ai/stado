//! What the marker stream must turn into: an ancestry, a reparented arrival
//! with no useful parent, and a host that cannot run the watch at all.

use super::parse::parse_watch;

#[test]
fn an_arrival_carries_the_parent_the_snapshot_still_held() {
    let stdout = "STADO_WATCH_BASELINE\t3963\t3963 1 Tue Sep  1 16:20:32 2026 /b/stado agent --target mini\n\
         STADO_WATCH_ARRIVAL\t1\t40111\t63\t40111 40109 Tue Sep  1 17:59:01 2026 /b/stado agent --target mini\n\
         STADO_WATCH_ANCESTOR\t1\t0\t40111\tyes\t40111 40109 Tue Sep  1 17:59:01 2026 /b/stado agent --target mini\n\
         STADO_WATCH_ANCESTOR\t1\t1\t40109\tyes\t40109 348 Tue Sep  1 17:59:01 2026 /bin/bash /u/keepalive.sh\n\
         STADO_WATCH_ANCESTOR\t1\t2\t348\tyes\t348 1 Wed Aug 26 21:45:35 2026 /bin/bash /u/supervise.sh\n\
         STADO_WATCH_DONE\t64\t63\n";
    let report = parse_watch("mini", "stado agent", 300, 1000, stdout);
    assert_eq!(report.baseline.len(), 1);
    assert_eq!(report.samples, 64);
    assert_eq!(report.arrivals.len(), 1);
    let arrival = &report.arrivals[0];
    assert_eq!(arrival.after_seconds, 63);
    assert_eq!(arrival.row.ppid, "40109");
    let parent = arrival.parent().expect("the parent was still alive");
    assert!(parent.alive);
    assert_eq!(parent.row.command, "/bin/bash /u/keepalive.sh");
    assert_eq!(arrival.ancestry.len(), 3);
}

#[test]
fn an_already_reparented_arrival_reports_no_parent() {
    let stdout = "STADO_WATCH_ARRIVAL\t1\t40111\t9\t40111 1 Tue Sep  1 17:59:01 2026 /b/stado agent --target mini\n\
         STADO_WATCH_ANCESTOR\t1\t0\t40111\tyes\t40111 1 Tue Sep  1 17:59:01 2026 /b/stado agent --target mini\n\
         STADO_WATCH_ANCESTOR\t1\t1\t1\tyes\t1 0 Wed Aug 26 21:45:29 2026 /sbin/launchd\n\
         STADO_WATCH_DONE\t10\t9\n";
    let report = parse_watch("mini", "stado agent", 300, 1000, stdout);
    let arrival = &report.arrivals[0];
    assert_eq!(arrival.row.ppid, "1");
    assert_eq!(
        arrival.parent().map(|parent| parent.row.command.as_str()),
        Some("/sbin/launchd")
    );
}

#[test]
fn a_non_darwin_host_says_so_instead_of_reporting_nothing() {
    let report = parse_watch(
        "box",
        "stado agent",
        60,
        1000,
        "STADO_WATCH_UNSUPPORTED\tLinux\n",
    );
    assert_eq!(report.unsupported.as_deref(), Some("Linux"));
    assert!(report.arrivals.is_empty());
}
