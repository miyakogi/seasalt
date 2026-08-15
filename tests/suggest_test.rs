use rusqlite::Connection;
use seasalt::db;
use seasalt::paths;
use seasalt::suggest;

fn temp_dir() -> std::path::PathBuf {
    let name = std::thread::current().name().unwrap_or("t").to_string();
    let dir = std::env::temp_dir().join(format!("seasalt-suggest-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir); // テスト用の一時ディレクトリなので削除してよい
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn recorded(conn: &Connection, cwd: &str, cmd: &str, started_at: i64) {
    let paths = paths::required_paths(cwd, cmd).join("\0");
    db::record_history(conn, cwd, cmd, started_at, "s", &paths).unwrap();
}

fn seed(conn: &Connection) {
    // started_at を明示制御して「最新」の判定を確定させる
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
    db::record_history(&conn, "/proj/sub", "cargo build", 5000, "s", "").unwrap();

    // 直前に実行して記録された同じコマンドは候補にしない
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

    // ファイルが存在する間はサジェストされる
    assert_eq!(
        suggest::suggest(&conn, cwd, "nvim").unwrap().unwrap(),
        "nvim a.txt"
    );
    // 削除すると候補から外れる
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
    // 最新の候補が削除済みファイルを参照していても、次の候補にフォールバックする
    db::record_history(&conn, cwd, "nvim gone.txt", 2000, "s", "gone.txt").unwrap();
    recorded(&conn, cwd, "nvim live.txt", 1000);

    assert_eq!(
        suggest::suggest(&conn, cwd, "nvim").unwrap().unwrap(),
        "nvim live.txt"
    );
    // 両方消えると None
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
    // 記録時点で存在しない引数 (hello など) は制約にならない
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

    // -la はパス扱いされないので常にサジェストされる
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

    // 親スコープの候補もカレントディレクトリ (sub) 基準で判定される
    assert!(suggest::suggest(&conn, sub_str, "nvim").unwrap().is_none());
    std::fs::write(sub.join("p.txt"), "x").unwrap();
    assert_eq!(
        suggest::suggest(&conn, sub_str, "nvim").unwrap().unwrap(),
        "nvim p.txt"
    );
    let _ = std::fs::remove_dir_all(&parent);
}
