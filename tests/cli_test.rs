use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_seasalt"))
}

fn temp_data_dir() -> std::path::PathBuf {
    // テストごとにユニークなディレクトリにする(並列実行時に他テストと DB を共有しないため)
    let name = std::thread::current().name().unwrap_or("t").to_string();
    let dir = std::env::temp_dir().join(format!("seasalt-cli-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir); // テスト用の一時ディレクトリなので削除してよい
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn record_then_exit_then_search() {
    let dir = temp_data_dir();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            "/tmp/x",
            "--session",
            "s1",
            "--",
            "echo hello",
        ])
        .status()
        .unwrap();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["exit", "--last-id", "1", "--code", "0"])
        .status()
        .unwrap();

    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--tsv", "hello"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    // tsv 形式: id\tcwd\tcmd\texit_code\tstarted_at (started_at は実時刻のため後半のみ確認)
    let fields: Vec<&str> = text.trim().split('\t').collect();
    assert_eq!(fields[0..4], ["1", "/tmp/x", "echo hello", "0"]);
    assert!(!fields[4].is_empty());
}

#[test]
fn record_dedups_identical_command() {
    let dir = temp_data_dir();
    let first = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            "/x",
            "--session",
            "s1",
            "--",
            "echo hello",
        ])
        .output()
        .unwrap();
    let second = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            "/x",
            "--session",
            "s2",
            "--",
            "echo hello",
        ])
        .output()
        .unwrap();
    let id1 = String::from_utf8(first.stdout).unwrap();
    let id2 = String::from_utf8(second.stdout).unwrap();
    assert_eq!(id1.trim(), id2.trim());

    // 履歴には 1 行だけ残る
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--tsv", "hello"])
        .output()
        .unwrap();
    assert_eq!(out.stdout.iter().filter(|&&b| b == b'\n').count(), 1);
}

#[test]
fn search_filters_by_cwd() {
    let dir = temp_data_dir();
    for (cwd, cmd) in [("/a", "one"), ("/b", "two")] {
        bin()
            .env("SEASALT_DATA_DIR", &dir)
            .args(["record", "--cwd", cwd, "--session", "s1", "--", cmd])
            .status()
            .unwrap();
    }
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--cwd", "/a", "o"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    // デフォルト出力は id<TAB>cmd
    assert_eq!(text.trim(), "1\tone");
}

#[test]
fn suggest_end_to_end() {
    let dir = temp_data_dir();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            "/proj",
            "--session",
            "s1",
            "--",
            "cargo build",
        ])
        .status()
        .unwrap();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["suggest", "--cwd", "/proj", "--", "cargo"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "cargo build");

    // 候補なし → 空出力・成功終了
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["suggest", "--cwd", "/proj", "--", "nothing"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"");
}

#[test]
fn record_prints_row_id() {
    let dir = temp_data_dir();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", "/x", "--session", "s1", "--", "cmd"])
        .output()
        .unwrap();
    let id: i64 = String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(id > 0);
}

#[test]
fn record_stores_existing_paths() {
    let dir = temp_data_dir();
    let name = std::thread::current().name().unwrap_or("t").to_string();
    let files =
        std::env::temp_dir().join(format!("seasalt-cli-files-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&files);
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("a.txt"), "x").unwrap();
    let cwd = files.to_str().unwrap();

    // 実在ファイル → paths に保存される
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            cwd,
            "--session",
            "s1",
            "--",
            "nvim a.txt",
        ])
        .status()
        .unwrap();
    // 存在しないファイル → paths は空
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            cwd,
            "--session",
            "s1",
            "--",
            "echo hello",
        ])
        .status()
        .unwrap();

    let conn = rusqlite::Connection::open(dir.join("history.sqlite3")).unwrap();
    let paths: String = conn
        .query_row(
            "SELECT paths FROM history WHERE cmd = 'nvim a.txt'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(paths, "a.txt");
    let paths: String = conn
        .query_row(
            "SELECT paths FROM history WHERE cmd = 'echo hello'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(paths, "");

    let _ = std::fs::remove_dir_all(&files);
}

#[test]
fn suggest_filters_deleted_files() {
    let dir = temp_data_dir();
    let name = std::thread::current().name().unwrap_or("t").to_string();
    let files =
        std::env::temp_dir().join(format!("seasalt-cli-files-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&files);
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("a.txt"), "x").unwrap();
    let cwd = files.to_str().unwrap();

    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            cwd,
            "--session",
            "s1",
            "--",
            "nvim a.txt",
        ])
        .status()
        .unwrap();

    // 存在する間はサジェストされる
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["suggest", "--cwd", cwd, "--", "nvim"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "nvim a.txt");

    // 削除するとサジェストされない
    std::fs::remove_file(files.join("a.txt")).unwrap();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["suggest", "--cwd", cwd, "--", "nvim"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"");

    let _ = std::fs::remove_dir_all(&files);
}

#[test]
fn delete_removes_history_entries() {
    let dir = temp_data_dir();
    for cmd in ["one", "two", "three"] {
        bin()
            .env("SEASALT_DATA_DIR", &dir)
            .args(["record", "--cwd", "/a", "--session", "s1", "--", cmd])
            .status()
            .unwrap();
    }

    // 複数 id を一括削除。成功時は何も出力しない
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["delete", "1", "3"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"");

    // 削除したコマンドは search にも suggest にも出ない
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "one"])
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"");
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["suggest", "--cwd", "/a", "--", "one"])
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"");

    // 残った行はデフォルト出力 id<TAB>cmd で確認できる
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "two"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "2\ttwo");

    // 存在しない id を指定しても静かに成功する
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["delete", "999"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"");
}

#[test]
fn record_ignores_leading_whitespace_commands() {
    let dir = temp_data_dir();
    // スペース / タブ始まりのコマンドは記録されない (exit 0・出力なし)
    for cmd in ["  ls -la", "\tgit status"] {
        let out = bin()
            .env("SEASALT_DATA_DIR", &dir)
            .args(["record", "--cwd", "/a", "--session", "s1", "--", cmd])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"");
    }
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "ls"])
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"");

    // 通常のコマンドは従来どおり記録される
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", "/a", "--session", "s1", "--", "ls -la"])
        .status()
        .unwrap();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "ls"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "1\tls -la");
}
