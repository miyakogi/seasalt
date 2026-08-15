use rusqlite::Connection;
use seasalt::db;
use seasalt::suggest;

fn seed(conn: &Connection) {
    // started_at を明示制御して「最新」の判定を確定させる
    db::insert_history(conn, "/proj/sub", "cargo build", 5000, "s").unwrap();
    db::insert_history(conn, "/proj/sub", "cargo test", 4000, "s").unwrap();
    db::insert_history(conn, "/proj", "cargo check", 3000, "s").unwrap();
    db::insert_history(conn, "/other", "cargo doc", 2000, "s").unwrap();
    db::insert_history(conn, "/home", "ls -la", 6000, "s").unwrap();
}

#[test]
fn exact_cwd_is_prioritized_over_parent() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    seed(&conn);

    // /proj/sub では /proj の cargo check ではなく /proj/sub の最新 (cargo build) を返す
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

    // /proj/deep/deeper は cwd 一致なし → 親 /proj/deep, /proj を順に探索 → /proj の cargo check
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

    // /nowhere は cwd・親に一致なし → グローバルで最新の ls -la
    let got = suggest::suggest(&conn, "/nowhere", "ls").unwrap().unwrap();
    assert_eq!(got, "ls -la");
}

#[test]
fn latest_within_scope_wins() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    seed(&conn);

    // 同一ディレクトリ /proj/sub 内で cargo で始まる最新 = cargo build (5000 > 4000)
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
    db::insert_history(&conn, "/proj/sub", "cargo build", 5000, "s").unwrap();

    // 直前に実行して記録された同じコマンドは候補にしない
    assert!(suggest::suggest(&conn, "/proj/sub", "cargo build")
        .unwrap()
        .is_none());
}

#[test]
fn case_insensitive_match() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::insert_history(&conn, "/proj/sub", "CARGO BUILD", 5000, "s").unwrap();

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
