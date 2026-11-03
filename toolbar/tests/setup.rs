//! `herdr-toolbar setup` the way a person runs it: the program inside an
//! unpacked release runs that release's `install.sh` on its own folder.

use std::fs;
use std::process::Command;

#[test]
fn setup_runs_the_trees_install_sh_and_passes_its_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let tree = tmp.path().join("my tree");
    let bin = tree.join("target/release/herdr-toolbar");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    // fs::copy keeps the permission bits, so the copy stays executable
    fs::copy(env!("CARGO_BIN_EXE_herdr-toolbar"), &bin).unwrap();
    fs::write(
        tree.join("install.sh"),
        "printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/args\"\nexit 7\n",
    )
    .unwrap();

    let status = Command::new(&bin)
        .args(["setup", "--tools"])
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(7));
    let args = fs::read_to_string(tree.join("args")).unwrap();
    let root = tree.canonicalize().unwrap();
    assert_eq!(args, format!("--tree\n{}\n--tools\n", root.display()));
}
