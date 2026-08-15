use rusqlite::Connection;
use seasalt::db;

#[test]
fn insert_and_update_exit_code_roundtrip() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();

    let id = db::insert_history(&conn, "/tmp/a", "echo hello", 1000, "s1", "").unwrap();
    db::update_exit_code(&conn, "s1", id, 42).unwrap();

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
    db::init(&conn).unwrap(); // 2回目もエラーにならない
}

#[test]
fn insert_returns_increasing_ids() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    let a = db::insert_history(&conn, "/x", "a", 1, "s", "").unwrap();
    let b = db::insert_history(&conn, "/x", "b", 2, "s", "").unwrap();
    assert!(b > a);
}

#[test]
fn default_db_path_respects_env_override() {
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
    // 存在しない (session, id) への update はエラーにしない
    db::update_exit_code(&conn, "nope", 999, 0).unwrap();
}

#[test]
fn insert_and_read_paths_roundtrip() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();

    let id = db::insert_history(
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
    // paths 列が無い旧スキーマ
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

    // マイグレーション後も挿入できる
    let id = db::insert_history(&conn, "/tmp/a", "echo hello", 1000, "s1", "x.txt").unwrap();
    let got: String = conn
        .query_row("SELECT paths FROM history WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(got, "x.txt");
}
