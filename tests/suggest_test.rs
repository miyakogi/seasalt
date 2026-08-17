use rusqlite::Connection;
use seasalt::db;
use seasalt::paths;
use seasalt::suggest;

fn temp_dir() -> std::path::PathBuf {
    let name = std::thread::current().name().unwrap_or("t").to_string();
    let dir = std::env::temp_dir().join(format!("seasalt-suggest-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir); // temp dir for the test; safe to delete
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn recorded(conn: &Connection, cwd: &str, cmd: &str, started_at: i64) {
    let paths = paths::required_paths(cwd, cmd).join("\0");
    db::record_history(conn, cwd, cmd, started_at, "s", &paths).unwrap();
}

fn seed(conn: &Connection) {
    // started_at is controlled explicitly to make the "latest" decision deterministic
    db::record_history(conn, "/proj/sub", "cargo build", 5000, "s", "").unwrap();
    db::record_history(conn, "/proj/sub", "cargo test", 4000, "s", "").unwrap();
    db::record_history(conn, "/proj", "cargo check", 3000, "s", "").unwrap();
    db::record_history(conn, "/other", "cargo doc", 2000, "s", "").unwrap();
    db::record_history(conn, "/home", "ls -la", 6000, "s", "").unwrap();
}

#[test]
fn exact_cwd_is_prioritized_over_parent() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    seed(&conn);

    // In /proj/sub the newest /proj/sub match (cargo build) wins over /proj's cargo check
    let got = suggest::suggest(&conn, "/proj/sub", "cargo")
        .unwrap()
        .unwrap();
    assert_eq!(got, "cargo build");
}

#[test]
fn parent_is_prioritized_over_global() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    seed(&conn);

    // /proj/deep/deeper has no exact match, so parents /proj/deep and /proj are searched in order -> cargo check
    let got = suggest::suggest(&conn, "/proj/deep/deeper", "cargo")
        .unwrap()
        .unwrap();
    assert_eq!(got, "cargo check");
}

#[test]
fn global_fallback_works() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    seed(&conn);

    // /nowhere has no cwd/parent match -> the newest global match (ls -la)
    let got = suggest::suggest(&conn, "/nowhere", "ls").unwrap().unwrap();
    assert_eq!(got, "ls -la");
}

#[test]
fn latest_within_scope_wins() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    seed(&conn);

    // Newest cargo-prefixed command within the same directory /proj/sub = cargo build (5000 > 4000)
    let got = suggest::suggest(&conn, "/proj/sub", "cargo")
        .unwrap()
        .unwrap();
    assert_eq!(got, "cargo build");
}

#[test]
fn no_match_returns_none() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    seed(&conn);

    assert!(suggest::suggest(&conn, "/proj/sub", "zzz_not_here")
        .unwrap()
        .is_none());
}

#[test]
fn suggestion_never_equals_current_line() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/proj/sub", "cargo build", 5000, "s", "").unwrap();

    // The same command just run and recorded is not suggested
    assert!(suggest::suggest(&conn, "/proj/sub", "cargo build")
        .unwrap()
        .is_none());
}

#[test]
fn case_insensitive_match() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/proj/sub", "CARGO BUILD", 5000, "s", "").unwrap();

    let got = suggest::suggest(&conn, "/proj/sub", "cargo")
        .unwrap()
        .unwrap();
    assert_eq!(got, "CARGO BUILD");
}

#[test]
fn ancestors_does_not_include_root() {
    assert_eq!(
        suggest::ancestors("/a/b/c"),
        vec!["/a/b".to_string(), "/a".to_string()]
    );
    assert!(suggest::ancestors("/").is_empty());
}

#[test]
fn empty_line_returns_none() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    seed(&conn);

    assert!(suggest::suggest(&conn, "/proj/sub", "").unwrap().is_none());
}

#[test]
fn deleted_file_blocks_suggestion() {
    let dir = temp_dir();
    let file = dir.join("a.txt");
    std::fs::write(&file, "x").unwrap();
    let cwd = dir.to_str().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    recorded(&conn, cwd, "nvim a.txt", 1000);

    // Suggested while the file exists
    assert_eq!(
        suggest::suggest(&conn, cwd, "nvim").unwrap().unwrap(),
        "nvim a.txt"
    );
    // Dropped from the candidates once deleted
    std::fs::remove_file(&file).unwrap();
    assert!(suggest::suggest(&conn, cwd, "nvim").unwrap().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn falls_back_to_next_candidate() {
    let dir = temp_dir();
    let live = dir.join("live.txt");
    std::fs::write(&live, "x").unwrap();
    let cwd = dir.to_str().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    // Even if the newest candidate references a deleted file, the next candidate is used
    db::record_history(&conn, cwd, "nvim gone.txt", 2000, "s", "gone.txt").unwrap();
    recorded(&conn, cwd, "nvim live.txt", 1000);

    assert_eq!(
        suggest::suggest(&conn, cwd, "nvim").unwrap().unwrap(),
        "nvim live.txt"
    );
    // Both gone -> None
    std::fs::remove_file(&live).unwrap();
    assert!(suggest::suggest(&conn, cwd, "nvim").unwrap().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nonexistent_args_do_not_constrain() {
    let dir = temp_dir();
    let cwd = dir.to_str().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    // Arguments that did not exist at record time (hello etc.) are not constraints
    recorded(&conn, cwd, "echo hello world", 1000);

    assert_eq!(
        suggest::suggest(&conn, cwd, "echo").unwrap().unwrap(),
        "echo hello world"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn flag_arguments_are_not_paths() {
    let dir = temp_dir();
    let cwd = dir.to_str().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    recorded(&conn, cwd, "ls -la", 1000);

    // -la is not treated as a path, so it is always suggested
    assert_eq!(
        suggest::suggest(&conn, cwd, "ls").unwrap().unwrap(),
        "ls -la"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn absolute_paths_checked_as_is() {
    let dir = temp_dir();
    let abs = dir.join("abs.txt");
    std::fs::write(&abs, "x").unwrap();
    let cwd = dir.to_str().unwrap();
    let abs_str = abs.to_str().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    recorded(&conn, cwd, &format!("nvim {abs_str}"), 1000);

    assert_eq!(
        suggest::suggest(&conn, cwd, "nvim").unwrap().unwrap(),
        format!("nvim {abs_str}")
    );
    std::fs::remove_file(&abs).unwrap();
    assert!(suggest::suggest(&conn, cwd, "nvim").unwrap().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parent_scope_checks_against_current_cwd() {
    let parent = temp_dir();
    let sub = parent.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(parent.join("p.txt"), "x").unwrap();
    let parent_str = parent.to_str().unwrap();
    let sub_str = sub.to_str().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    recorded(&conn, parent_str, "nvim p.txt", 1000);

    // Parent-scope candidates are also judged against the current directory (sub)
    assert!(suggest::suggest(&conn, sub_str, "nvim").unwrap().is_none());
    std::fs::write(sub.join("p.txt"), "x").unwrap();
    assert_eq!(
        suggest::suggest(&conn, sub_str, "nvim").unwrap().unwrap(),
        "nvim p.txt"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn case_sensitive_match_is_preferred() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    // An older exact-case candidate is preferred over a newer icase-only one (like fish)
    db::record_history(&conn, "/proj/sub", "CARGO BUILD", 2000, "s", "").unwrap();
    db::record_history(&conn, "/proj/sub", "cargo build", 1000, "s", "").unwrap();
    let got = suggest::suggest(&conn, "/proj/sub", "cargo")
        .unwrap()
        .unwrap();
    assert_eq!(got, "cargo build");
}

#[test]
fn case_insensitive_fallback_without_sensitive_match() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/proj/sub", "CARGO BUILD", 2000, "s", "").unwrap();
    let got = suggest::suggest(&conn, "/proj/sub", "cargo")
        .unwrap()
        .unwrap();
    assert_eq!(got, "CARGO BUILD");
}

#[test]
fn uppercase_needle_prefers_exact_case() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/proj/sub", "cargo build", 2000, "s", "").unwrap();
    db::record_history(&conn, "/proj/sub", "Cargo check", 1000, "s", "").unwrap();
    // Exact case is preferred even for a needle containing uppercase (like fish)
    let got = suggest::suggest(&conn, "/proj/sub", "Cargo")
        .unwrap()
        .unwrap();
    assert_eq!(got, "Cargo check");
}

#[test]
fn cwd_icase_beats_parent_exact_case() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/proj/sub", "CARGO BUILD", 2000, "s", "").unwrap();
    db::record_history(&conn, "/proj", "cargo check", 1000, "s", "").unwrap();
    // Scope priority outranks case priority: the cwd's icase candidate wins
    let got = suggest::suggest(&conn, "/proj/sub", "cargo")
        .unwrap()
        .unwrap();
    assert_eq!(got, "CARGO BUILD");
}

#[test]
fn stale_sensitive_candidate_falls_back_to_icase() {
    let dir = temp_dir();
    let cwd = dir.to_str().unwrap();
    std::fs::write(dir.join("live.txt"), "x").unwrap();
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    // The exact-case candidate is stale (gone.txt does not exist)
    db::record_history(&conn, cwd, "nvim gone.txt", 2000, "s", "gone.txt").unwrap();
    // The icase-only candidate is valid
    db::record_history(&conn, cwd, "NVIM live.txt", 1000, "s", "live.txt").unwrap();

    let got = suggest::suggest(&conn, cwd, "nvim").unwrap().unwrap();
    assert_eq!(got, "NVIM live.txt");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn exact_case_preferred_when_line_contains_glob_chars() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/x", "LS *.txt", 2000, "s", "").unwrap();
    db::record_history(&conn, "/x", "ls *.txt", 1000, "s", "").unwrap();
    // Like fish, the exact-case match wins even though it is older;
    // the broken GLOB escape made the sensitive pass miss and the
    // newer icase candidate "LS *.txt" win instead.
    let got = suggest::suggest(&conn, "/x", "ls *").unwrap().unwrap();
    assert_eq!(got, "ls *.txt");
}

#[test]
fn global_scope_prefers_exact_case() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/a", "Cargo build", 2000, "s", "").unwrap();
    db::record_history(&conn, "/a", "cargo check", 1000, "s", "").unwrap();
    // /nowhere has no cwd/parent match; the global scope prefers
    // the exact-case candidate even though it is older.
    let got = suggest::suggest(&conn, "/nowhere", "cargo")
        .unwrap()
        .unwrap();
    assert_eq!(got, "cargo check");
}

#[test]
fn like_escape_handles_underscore_in_icase_fallback() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    // Only a case-mismatched candidate exists: the sensitive GLOB pass
    // misses and the icase LIKE fallback must not treat `_` as a wildcard.
    db::record_history(&conn, "/proj/sub", "LS _x.txt", 2000, "s", "").unwrap();
    let got = suggest::suggest(&conn, "/proj/sub", "ls _")
        .unwrap()
        .unwrap();
    assert_eq!(got, "LS _x.txt");
}

#[test]
fn like_escape_handles_percent_in_icase_fallback() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/proj/sub", "PRINTF %s x", 2000, "s", "").unwrap();
    let got = suggest::suggest(&conn, "/proj/sub", "printf %s")
        .unwrap()
        .unwrap();
    assert_eq!(got, "PRINTF %s x");
}

#[test]
fn zero_budget_aborts_before_any_query() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/proj/sub", "cargo build", 5000, "s", "").unwrap();

    // An expired budget yields no suggestion (mid-query interrupts are
    // swallowed; the pre-query deadline check makes this deterministic).
    let got =
        suggest::suggest_budgeted(&conn, "/proj/sub", "cargo", Some(std::time::Duration::ZERO))
            .unwrap();
    assert!(got.is_none());

    // A generous budget, by contrast, still suggests the match.
    let got = suggest::suggest_budgeted(
        &conn,
        "/proj/sub",
        "cargo",
        Some(std::time::Duration::from_secs(10)),
    )
    .unwrap();
    assert_eq!(got, Some("cargo build".to_string()));
}

/// Exercises the mid-query interrupt path (OperationInterrupted swallow).
///
/// Unlike `zero_budget_aborts_before_any_query` which short-circuits at the
/// pre-query `expired()` check, this test uses a tiny but *positive* budget
/// so the pre-query `expired()` returns false and the SQL query actually
/// starts.  200k rows with a broad prefix ("c*") force the scoped query to
/// scan and sort a large range — easily taking milliseconds, far longer than
/// the 1 µs budget.  The `progress_handler` (fires every 100k VM ops) sees
/// the expired deadline mid-query and SQLite raises `OperationInterrupted`,
/// which `suggest_budgeted` swallows as `Ok(None)`.
///
/// A second assertion with a generous budget on the same data proves the
/// query *would* succeed without the interrupt — if the swallow path were
/// wrongly turned into an error, this test would fail.
#[test]
fn mid_query_interrupt_is_swallowed() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();

    // Seed 200k rows in the same cwd, all starting with "c" so the GLOB
    // "c*" scan covers the entire set.  The broad prefix + ORDER BY
    // started_at DESC forces a full scan + sort before LIMIT kicks in.
    conn.execute_batch("BEGIN").unwrap();
    {
        let mut stmt = conn
            .prepare(
                "INSERT INTO history (cwd, cmd, started_at, session, paths)
                 VALUES ('/big', ?1, ?2, 's', '')",
            )
            .unwrap();
        for i in 0..200_000 {
            stmt.execute(rusqlite::params![format!("cmd-{i}"), i as i64])
                .unwrap();
        }
    }
    conn.execute_batch("COMMIT").unwrap();

    // Positive but tiny budget: 1 µs.  The pre-query expired() check
    // (Instant::now() >= deadline) is false at entry because the deadline
    // is ~1 µs in the future, so the query starts.  But the scan+sort of
    // 200k rows takes milliseconds — long past the deadline.  The
    // progress_handler fires after 100k VM ops, sees the expired deadline,
    // and SQLite raises OperationInterrupted → swallowed as Ok(None).
    let got = suggest::suggest_budgeted(
        &conn,
        "/big",
        "c",
        Some(std::time::Duration::from_micros(1)),
    );
    assert!(got.unwrap().is_none(), "interrupt should be swallowed");

    // With a generous budget the same query succeeds, proving the data is
    // valid and the only reason the short-budget call returned None was
    // the mid-query interrupt.
    let got =
        suggest::suggest_budgeted(&conn, "/big", "c", Some(std::time::Duration::from_secs(10)));
    assert!(
        got.unwrap().is_some(),
        "query should succeed without interrupt"
    );
}
