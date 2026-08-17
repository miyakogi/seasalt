use rusqlite::Connection;
use seasalt::db;

/// Serializes tests that mutate process-global env vars
/// (std::env::set_var is process-wide; parallel tests would race).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn insert_and_update_exit_code_roundtrip() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();

    let id = db::record_history(&conn, "/tmp/a", "echo hello", 1000, "s1", "").unwrap();
    db::update_exit_code(&conn, id, 42).unwrap();

    let mut stmt = conn
        .prepare("SELECT cwd, cmd, exit_code, started_at, session FROM history WHERE id = ?1")
        .unwrap();
    let row = stmt
        .query_row([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .unwrap();
    assert_eq!(
        row,
        (
            "/tmp/a".into(),
            "echo hello".into(),
            Some(42),
            1000,
            "s1".into()
        )
    );
}

#[test]
fn init_is_idempotent() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::init(&conn).unwrap(); // the second call must not fail either
}

#[test]
fn insert_returns_increasing_ids() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    let a = db::record_history(&conn, "/x", "a", 1, "s", "").unwrap();
    let b = db::record_history(&conn, "/x", "b", 2, "s", "").unwrap();
    assert!(b > a);
}

#[test]
fn default_db_path_respects_env_override() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!("seasalt-test-{}", std::process::id()));
    std::env::set_var("SEASALT_DATA_DIR", &dir);
    let path = db::default_db_path().unwrap();
    assert_eq!(path, dir.join("history.sqlite3"));
    std::env::remove_var("SEASALT_DATA_DIR");
}

#[test]
fn update_exit_code_on_missing_row_is_ok() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    // Updating a nonexistent id is not an error
    db::update_exit_code(&conn, 999, 0).unwrap();
}

#[test]
fn insert_and_read_paths_roundtrip() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();

    let id = db::record_history(
        &conn,
        "/tmp/a",
        "nvim a.txt b.txt",
        1000,
        "s1",
        "a.txt\0b.txt",
    )
    .unwrap();
    let got: String = conn
        .query_row("SELECT paths FROM history WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(got, "a.txt\0b.txt");
}

#[test]
fn init_adds_paths_column_to_old_schema() {
    let conn = Connection::open_in_memory().unwrap();
    // Old schema without the paths column
    conn.execute_batch(
        "CREATE TABLE history (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          cwd TEXT NOT NULL,
          cmd TEXT NOT NULL,
          exit_code INTEGER,
          started_at INTEGER NOT NULL,
          session TEXT
        );",
    )
    .unwrap();

    db::init(&conn).unwrap();

    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(history)")
        .unwrap()
        .query_map([], |r| r.get(1))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(cols.contains(&"paths".to_string()));

    // Insertion works after the migration
    let id = db::record_history(&conn, "/tmp/a", "echo hello", 1000, "s1", "x.txt").unwrap();
    let got: String = conn
        .query_row("SELECT paths FROM history WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(got, "x.txt");
}

#[test]
fn record_dedups_same_command_and_bumps_to_latest() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();

    let id1 = db::record_history(&conn, "/tmp/a", "nvim a.txt", 1000, "s1", "a.txt").unwrap();
    let id2 = db::record_history(&conn, "/tmp/a", "nvim b.txt", 2000, "s1", "b.txt").unwrap();
    // Even non-consecutive duplicates with the same (cwd, cmd) refresh the existing row
    let id3 = db::record_history(&conn, "/tmp/a", "nvim a.txt", 3000, "s2", "").unwrap();

    assert_eq!(id1, id3);
    assert_ne!(id1, id2);
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM history WHERE cwd = '/tmp/a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
    let (started_at, paths, exit_code, session): (i64, String, Option<i64>, String) = conn
        .query_row(
            "SELECT started_at, paths, exit_code, session FROM history WHERE id = ?1",
            [id1],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(started_at, 3000);
    assert_eq!(paths, "");
    assert_eq!(exit_code, None);
    assert_eq!(session, "s2");
}

#[test]
fn record_does_not_dedup_across_directories() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    let a = db::record_history(&conn, "/dir/a", "ls", 1000, "s", "").unwrap();
    let b = db::record_history(&conn, "/dir/b", "ls", 2000, "s", "").unwrap();
    assert_ne!(a, b);
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn deduped_record_resets_exit_code_until_exit() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    let id = db::record_history(&conn, "/tmp/a", "make", 1000, "s1", "").unwrap();
    db::update_exit_code(&conn, id, 0).unwrap();
    // A re-run that dedups resets exit_code to undetermined
    db::record_history(&conn, "/tmp/a", "make", 2000, "s2", "").unwrap();
    let code: Option<i64> = conn
        .query_row("SELECT exit_code FROM history WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(code, None);
    // The row matches by id even after its session was rewritten
    db::update_exit_code(&conn, id, 7).unwrap();
    let code: Option<i64> = conn
        .query_row("SELECT exit_code FROM history WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(code, Some(7));
}

#[cfg(unix)]
#[test]
fn new_data_dir_and_db_get_restricted_permissions() {
    let _guard = ENV_LOCK.lock().unwrap();
    use std::os::unix::fs::PermissionsExt;

    let base = std::env::temp_dir().join(format!(
        "seasalt-perms-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("t")
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::env::set_var("SEASALT_DATA_DIR", &base);

    let path = db::default_db_path().unwrap();
    assert_eq!(
        std::fs::metadata(&base).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(!path.exists());

    db::open(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    std::env::remove_var("SEASALT_DATA_DIR");
    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(unix)]
#[test]
fn existing_data_dir_and_db_permissions_are_left_unchanged() {
    let _guard = ENV_LOCK.lock().unwrap();
    use std::os::unix::fs::PermissionsExt;

    let base = std::env::temp_dir().join(format!(
        "seasalt-perms-existing-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("t")
    ));
    let _ = std::fs::remove_dir_all(&base);
    // Reproduce an existing environment: 0755 directory + 0644 DB
    std::fs::create_dir_all(&base).unwrap();
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = base.join("history.sqlite3");
    std::fs::write(&path, "").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::env::set_var("SEASALT_DATA_DIR", &base);

    let got = db::default_db_path().unwrap();
    assert_eq!(got, path);
    db::open(&got).unwrap();
    assert_eq!(
        std::fs::metadata(&base).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );

    std::env::remove_var("SEASALT_DATA_DIR");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn delete_by_ids_removes_only_requested_rows() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    let id1 = db::record_history(&conn, "/a", "one", 1000, "s", "").unwrap();
    let id2 = db::record_history(&conn, "/a", "two", 2000, "s", "").unwrap();
    let id3 = db::record_history(&conn, "/a", "three", 3000, "s", "").unwrap();

    db::delete_by_ids(&conn, &[id1, id3]).unwrap();

    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let rest: i64 = conn
        .query_row("SELECT id FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rest, id2);
}

#[test]
fn delete_by_ids_ignores_nonexistent_ids() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    let id = db::record_history(&conn, "/a", "one", 1000, "s", "").unwrap();
    // Nonexistent ids are not an error either
    db::delete_by_ids(&conn, &[id, 999]).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn writers_wait_for_busy_database() {
    let dir = std::env::temp_dir().join(format!(
        "seasalt-busy-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("t")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("history.sqlite3");

    let conn = db::open(&path).unwrap();
    let blocker = db::open(&path).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(150));
        blocker.execute_batch("COMMIT").unwrap();
    });
    rx.recv().unwrap();
    // Starts while the other connection holds the write lock; the bounded busy
    // timeout must wait for the lock instead of failing with SQLITE_BUSY.
    let id = db::record_history(&conn, "/x", "echo hi", 1000, "s", "").unwrap();
    handle.join().unwrap();
    assert!(id > 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_sets_bounded_busy_timeout() {
    let dir = std::env::temp_dir().join(format!(
        "seasalt-busy-timeout-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("t")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("history.sqlite3");

    let conn = db::open(&path).unwrap();
    let timeout: i64 = conn
        .query_row("SELECT * FROM pragma_busy_timeout", [], |row| row.get(0))
        .unwrap();
    // rusqlite 0.37 installs a 5000ms default at open; db::open must bound it
    // explicitly so a stuck writer cannot stall a shell hook for seconds.
    assert_eq!(timeout, 300);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn trim_history_keeps_newest_rows() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    for i in 0..12 {
        db::record_history(&conn, "/x", &format!("cmd {i}"), 1000 + i as i64, "s", "").unwrap();
    }
    db::trim_history(&conn, 10).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 10);
    let cmds: Vec<String> = conn
        .prepare("SELECT cmd FROM history ORDER BY started_at")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        cmds,
        (2..12).map(|i| format!("cmd {i}")).collect::<Vec<_>>()
    );
}

#[test]
fn trim_history_at_limit_deletes_nothing() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    for i in 0..10 {
        db::record_history(&conn, "/x", &format!("cmd {i}"), 1000 + i as i64, "s", "").unwrap();
    }
    db::trim_history(&conn, 10).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 10);
}

#[test]
fn trim_history_under_limit_deletes_nothing() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    for i in 0..5 {
        db::record_history(&conn, "/x", &format!("cmd {i}"), 1000 + i as i64, "s", "").unwrap();
    }
    db::trim_history(&conn, 10).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 5);
}

#[test]
fn trim_history_protects_refreshed_rows() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    // "make" is the oldest row, then refreshed by a re-run (dedup)
    db::record_history(&conn, "/x", "make", 1000, "s", "").unwrap();
    for i in 1..12 {
        db::record_history(&conn, "/x", &format!("cmd {i}"), 1000 + i as i64, "s", "").unwrap();
    }
    db::record_history(&conn, "/x", "make", 9999, "s", "").unwrap();
    db::trim_history(&conn, 10).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 10);
    let has_make: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM history WHERE cmd = 'make')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(has_make);
}

#[test]
fn clear_removes_all_rows() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    for i in 0..5 {
        db::record_history(&conn, "/x", &format!("cmd {i}"), 1000 + i as i64, "s", "").unwrap();
    }
    db::clear(&conn).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn init_creates_started_at_index() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    let name: String = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_history_started_at'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "idx_history_started_at");
}

#[test]
fn init_records_schema_version() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 2);
}

#[test]
fn init_dedupes_legacy_duplicates_and_adds_unique_index() {
    let conn = Connection::open_in_memory().unwrap();
    // Simulate a legacy DB: no paths column, duplicate (cwd, cmd) rows
    conn.execute_batch(
        "CREATE TABLE history (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          cwd TEXT NOT NULL,
          cmd TEXT NOT NULL,
          exit_code INTEGER,
          started_at INTEGER NOT NULL,
          session TEXT
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO history (cwd, cmd, started_at) VALUES ('/x', 'make', 1000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO history (cwd, cmd, started_at) VALUES ('/x', 'make', 2000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO history (cwd, cmd, started_at) VALUES ('/x', 'make', 3000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO history (cwd, cmd, started_at) VALUES ('/x', 'test', 2500)",
        [],
    )
    .unwrap();

    db::init(&conn).unwrap();

    let (count, latest): (i64, i64) = conn
        .query_row("SELECT count(*), max(started_at) FROM history", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    // 4 rows collapse to the newest 2 (one per (cwd, cmd))
    assert_eq!(count, 2);
    assert_eq!(latest, 3000);
    let name: String = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'index'
             AND name = 'idx_history_cwd_cmd_unique'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "idx_history_cwd_cmd_unique");
}
