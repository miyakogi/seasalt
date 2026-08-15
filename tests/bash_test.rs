use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn init_bash_output_is_valid_bash_syntax() {
    let out = Command::new(env!("CARGO_BIN_EXE_seasalt"))
        .args(["init", "bash"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(!out.stdout.is_empty());

    let mut child = Command::new("bash")
        .args(["-n"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&out.stdout)
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "bash -n が失敗: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn init_bash_survives_eval_command_substitution() {
    let bin = env!("CARGO_BIN_EXE_seasalt");
    let script = format!(
        r#"set -euo pipefail
SEASALT_BIN={bin}
SEASALT_DATA_DIR=$(mktemp -d)
blehook() {{ :; }}
ble/util/idle.push() {{ :; }}
eval $("$SEASALT_BIN" init bash)
declare -F _seasalt_preexec >/dev/null
declare -F _seasalt_precmd >/dev/null
"#
    );
    let out = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(
        out.status.success(),
        "eval に失敗: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn init_bash_reorders_auto_source_after_idle() {
    let bin = env!("CARGO_BIN_EXE_seasalt");
    let script = format!(
        r#"set -euo pipefail
SEASALT_BIN={bin}
SEASALT_DATA_DIR=$(mktemp -d)
BASHER_HOOKS=()
BASHER_IDLE_TASKS=()
blehook() {{ BASHER_HOOKS+=("$*"); }}
ble/util/idle.push() {{ BASHER_IDLE_TASKS+=("$1"); }}
# 本物の ble.sh は idle タスクを関数内で実行するため、テスト側も関数で受ける
fire_idle_task() {{ eval "$1"; }}
unset _ble_complete_auto_source
eval $("$SEASALT_BIN" init bash)
[[ ${{#BASHER_HOOKS[@]}} -eq 2 ]]
[[ ${{#BASHER_IDLE_TASKS[@]}} -eq 1 ]]
# _ble_complete_auto_source が未定義のまま idle タスクが走っても安全に初期化される
fire_idle_task "${{BASHER_IDLE_TASKS[0]}}"
[[ " ${{_ble_complete_auto_source[*]}} " == " seasalt history syntax " ]]
# core-complete ロード(無条件リセット)と atuin の onload 登録が完了した状態で
# idle タスクを実行すると seasalt が先頭に来る
_ble_complete_auto_source=(atuin-history history syntax)
fire_idle_task "${{BASHER_IDLE_TASKS[0]}}"
[[ " ${{_ble_complete_auto_source[*]}} " == " seasalt atuin-history history syntax " ]]
# 起動後の再 eval でも重複せず冪等であること
eval $("$SEASALT_BIN" init bash)
[[ ${{#BASHER_HOOKS[@]}} -eq 2 ]]
[[ ${{#BASHER_IDLE_TASKS[@]}} -eq 2 ]]
fire_idle_task "${{BASHER_IDLE_TASKS[1]}}"
[[ " ${{_ble_complete_auto_source[*]}} " == " seasalt atuin-history history syntax " ]]
"#
    );
    let out = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(
        out.status.success(),
        "自動補完ソースの再整列に失敗: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn init_bash_rejects_unknown_shell() {
    let out = Command::new(env!("CARGO_BIN_EXE_seasalt"))
        .args(["init", "zsh"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}
