//! Performance benchmarks for the suggest pipeline and the hook-facing
//! CLI (run with `cargo bench`; not part of `cargo test`).
//!
//! Two layers are measured:
//! - in-process: `db::open` + `suggest::suggest` — the exact per-keystroke
//!   cost the shell would pay, without process startup
//! - end-to-end: spawning the real binary — process startup + DB open +
//!   query, which is what the snippet actually waits for
//!
//! Databases are seeded into a temporary directory once per run
//! (setup time is not measured) and removed on exit.

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use rusqlite::Connection;
use seasalt::{db, suggest};

const DIRS: &[&str] = &[
    "/proj/alpha",
    "/proj/alpha/sub",
    "/proj/beta",
    "/proj/beta/sub/deep",
    "/home/user",
    "/home/user/notes",
    "/tmp",
    "/var/log",
];

const CMDS: &[&str] = &[
    "cargo build --release",
    "cargo bench --features bench",
    "cargo test --test suggest_test",
    "cargo check --all-targets",
    "cargo clippy --all-targets -- -D warnings",
    "git status --short",
    "git push origin main",
    "git log --oneline -10",
    "nvim src/db.rs",
    "nvim src/suggest.rs",
    "ls -la",
    "echo hello world",
];

const SIZES: &[usize] = &[1_000, 10_000, 100_000, 1_000_000];

/// Temporary directory holding the seeded databases (one subdirectory
/// per row count, each containing history.sqlite3 so that
/// SEASALT_DATA_DIR semantics work for the end-to-end runs).
struct BenchDir(PathBuf);

impl BenchDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "seasalt-bench-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn db_path(&self, rows: usize) -> PathBuf {
        let sub = self.0.join(rows.to_string());
        std::fs::create_dir_all(&sub).unwrap();
        sub.join("history.sqlite3")
    }
}

impl Drop for BenchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Seeds a database with `rows` history rows. The distribution over
/// DIRS/CMDS is deterministic (xorshift64*), started_at increases with
/// the row number, and paths is always empty (no filesystem noise).
fn seed_db(path: &Path, rows: usize) {
    let conn = Connection::open(path).unwrap();
    db::init(&conn).unwrap();
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    conn.execute_batch("BEGIN").unwrap();
    {
        let mut stmt = conn
            .prepare(
                "INSERT INTO history (cwd, cmd, started_at, session, paths)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .unwrap();
        for i in 0..rows {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let cwd = DIRS[(x >> 32) as usize % DIRS.len()];
            let cmd = CMDS[(x >> 33) as usize % CMDS.len()];
            // Append a per-row suffix so every (cwd, cmd) pair is unique,
            // satisfying the UNIQUE(cwd, cmd) index while preserving the
            // original cwd/cmd distribution for realistic cardinality.
            let unique_cmd = format!("{cmd} #{i}");
            stmt.execute(rusqlite::params![cwd, unique_cmd, i as i64, "bench", ""])
                .unwrap();
        }
    }
    conn.execute_batch("COMMIT").unwrap();
}

/// In-process cost of the per-keystroke path: open the DB (schema init
/// and the migration check included) and run the suggest pipeline.
fn bench_in_process(c: &mut Criterion, dir: &BenchDir) {
    let mut group = c.benchmark_group("suggest_in_process");
    group.sample_size(50);
    for &rows in SIZES {
        let path = dir.db_path(rows);
        for (name, cwd, needle) in [
            ("hit", "/proj/alpha", "cargo b"),
            ("miss", "/proj/alpha", "zzz_not_found"),
            // 10 ancestor scopes + cwd + global, sensitive + icase each:
            // 24 queries total, the worst case.
            ("miss_deep", "/a/b/c/d/e/f/g/h/i/j", "zzz_not_found"),
        ] {
            group.bench_function(BenchmarkId::new(name, rows), |b| {
                b.iter(|| {
                    let conn = db::open(&path).unwrap();
                    black_box(suggest::suggest(&conn, cwd, needle).unwrap());
                })
            });
        }
    }
    group.finish();
}

/// Cost of the automatic trim on record: under the limit (the steady
/// state, 0 rows deleted) and over the limit (deleting 10k rows once
/// the limit is crossed).
fn bench_trim(c: &mut Criterion, dir: &BenchDir) {
    let path = dir.db_path(100_000);
    let mut group = c.benchmark_group("trim_history");
    group.sample_size(50);
    group.bench_function("under_limit_100k", |b| {
        b.iter(|| {
            let conn = db::open(&path).unwrap();
            db::trim_history(&conn, 100_000).unwrap();
            black_box(());
        })
    });
    group.sample_size(10);
    // Global monotonic counter ensures every inserted "fresh" command
    // is unique across iterations, avoiding SQLITE_CONSTRAINT_UNIQUE
    // when the same (cwd, cmd) pair survives a trim between iterations.
    static FRESH_COUNTER: AtomicU64 = AtomicU64::new(0);
    group.bench_function("over_limit_delete_10k", |b| {
        b.iter_batched(
            || {
                let conn = db::open(&path).unwrap();
                // Re-grow the table to ~110k rows with fresh commands
                // so each iteration deletes the same 10k again.
                let base = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64;
                conn.execute_batch("BEGIN").unwrap();
                {
                    let mut stmt = conn
                        .prepare(
                            "INSERT INTO history (cwd, cmd, started_at, session, paths)
                             VALUES ('/x', ?1, ?2, 'bench', '')",
                        )
                        .unwrap();
                    for i in 0..10_000 {
                        let n = FRESH_COUNTER.fetch_add(1, Ordering::Relaxed);
                        stmt.execute(rusqlite::params![format!("fresh-{n}"), base + i as i64])
                            .unwrap();
                    }
                }
                conn.execute_batch("COMMIT").unwrap();
                conn
            },
            |conn| {
                db::trim_history(&conn, 100_000).unwrap();
                black_box(());
            },
            criterion::BatchSize::SmallInput,
        )
    });
    group.finish();
}

/// Wall time of spawning the real binary (process startup + DB open +
/// query), which is what the shell waits for on every keystroke.
fn bench_suggest_end_to_end(c: &mut Criterion, dir: &BenchDir) {
    let bin = env!("CARGO_BIN_EXE_seasalt");
    let mut group = c.benchmark_group("suggest_end_to_end");
    group.sample_size(20);
    for &rows in &[10_000, 100_000, 1_000_000] {
        let db = dir.db_path(rows);
        let data_dir = db.parent().unwrap().to_str().unwrap();
        group.bench_function(BenchmarkId::new("hit", rows), |b| {
            b.iter(|| {
                let out = Command::new(bin)
                    .args(["suggest", "--cwd", "/proj/alpha", "--", "cargo b"])
                    .env("SEASALT_DATA_DIR", data_dir)
                    .output()
                    .unwrap();
                assert!(out.status.success());
                assert!(!out.stdout.is_empty());
            })
        });
    }
    group.finish();
}

/// Wall time of one `record` (spawned binary), the cost the preexec
/// hook pays per executed command. A fresh command per iteration so the
/// insert path is measured.
fn bench_record_end_to_end(c: &mut Criterion, dir: &BenchDir) {
    let bin = env!("CARGO_BIN_EXE_seasalt");
    let db = dir.db_path(100_000);
    let data_dir = db.parent().unwrap().to_str().unwrap();
    static N: AtomicU64 = AtomicU64::new(0);
    let mut group = c.benchmark_group("record_end_to_end");
    group.sample_size(20);
    group.bench_function("insert_100k", |b| {
        b.iter(|| {
            let n = N.fetch_add(1, Ordering::Relaxed);
            let cmd = format!("echo bench-record-{n}");
            let out = Command::new(bin)
                .args([
                    "record",
                    "--cwd",
                    "/proj/alpha",
                    "--session",
                    "bench",
                    "--",
                    &cmd,
                ])
                .env("SEASALT_DATA_DIR", data_dir)
                .output()
                .unwrap();
            assert!(out.status.success());
            assert!(!out.stdout.is_empty());
        })
    });
    group.finish();
}

fn run_all(c: &mut Criterion) {
    let dir = BenchDir::new();
    for &rows in SIZES {
        seed_db(&dir.db_path(rows), rows);
    }
    bench_in_process(c, &dir);
    bench_trim(c, &dir);
    bench_suggest_end_to_end(c, &dir);
    bench_record_end_to_end(c, &dir);
}

criterion_group!(benches, run_all);
criterion_main!(benches);
