//! `herdr-toolbar setup`: the steps after installing (link the plugins,
//! write `tools.toml`, print the key bindings), for a release that someone
//! else unpacked, such as mise. It runs that release's own `install.sh` with
//! `--tree`, so the steps live in one place.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

/// The release folder `exe` runs from: the folder above `target/release/`,
/// canonical, holding the `install.sh` that release shipped.
pub fn release_root(exe: &Path) -> anyhow::Result<PathBuf> {
    let exe = exe
        .canonicalize()
        .with_context(|| format!("cannot find {}", exe.display()))?;
    let named = |p: &Path, name: &str| p.file_name() == Some(OsStr::new(name));
    let root = exe
        .parent()
        .filter(|release| named(release, "release"))
        .and_then(Path::parent)
        .filter(|target| named(target, "target"))
        .and_then(Path::parent);
    let Some(root) = root else {
        bail!(
            "herdr-toolbar setup runs from an unpacked release \
             (<folder>/target/release/herdr-toolbar), not from {}",
            exe.display()
        );
    };
    // a checkout's target/ can be gigabytes: never copy it
    if root.join(".git").exists() {
        bail!(
            "{} is a development checkout: link it with 'herdr plugin link {}/toolbar' instead",
            root.display(),
            root.display()
        );
    }
    if !root.join("install.sh").is_file() {
        bail!("{} has no install.sh", root.display());
    }
    Ok(root.to_path_buf())
}

/// `sh <root>/install.sh --tree <root> <args…>`.
pub fn command(root: &Path, args: &[String]) -> Command {
    let mut c = Command::new("sh");
    c.arg(root.join("install.sh"))
        .arg("--tree")
        .arg(root)
        .args(args);
    c
}

/// Runs the `install.sh` of the release this program came from, on that
/// release's folder. Returns its exit code, or 1 when a signal ended it.
pub fn run(args: &[String]) -> anyhow::Result<u8> {
    let exe = std::env::current_exe().context("cannot tell where herdr-toolbar runs from")?;
    let root = release_root(&exe)?;
    let status = command(&root, args).status().context("cannot run sh")?;
    Ok(status
        .code()
        .and_then(|c| u8::try_from(c).ok())
        .unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// An unpacked release: `target/release/herdr-toolbar`, and `install.sh`
    /// when asked for.
    fn tree(with_install_sh: bool) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("target/release")).unwrap();
        fs::write(tmp.path().join("target/release/herdr-toolbar"), "").unwrap();
        if with_install_sh {
            fs::write(tmp.path().join("install.sh"), "").unwrap();
        }
        tmp
    }

    fn exe(root: &Path) -> PathBuf {
        root.join("target/release/herdr-toolbar")
    }

    #[test]
    fn root_of_an_unpacked_release() {
        let t = tree(true);
        assert_eq!(
            release_root(&exe(t.path())).unwrap(),
            t.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn refuses_a_development_checkout() {
        let t = tree(true);
        fs::create_dir(t.path().join(".git")).unwrap();
        let err = release_root(&exe(t.path())).unwrap_err().to_string();
        assert!(err.contains("is a development checkout"), "{err}");
    }

    #[test]
    fn refuses_a_tree_without_install_sh() {
        let t = tree(false);
        let err = release_root(&exe(t.path())).unwrap_err().to_string();
        assert!(err.contains("has no install.sh"), "{err}");
    }

    #[test]
    fn refuses_a_binary_outside_target_release() {
        let t = tree(true);
        let elsewhere = t.path().join("bin/herdr-toolbar");
        fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        fs::write(&elsewhere, "").unwrap();
        let err = release_root(&elsewhere).unwrap_err().to_string();
        assert!(err.contains("runs from an unpacked release"), "{err}");
    }

    #[test]
    fn command_runs_install_sh_on_the_tree() {
        let root = Path::new("/r");
        let c = command(root, &["--tools".into()]);
        assert_eq!(c.get_program(), "sh");
        let args: Vec<&OsStr> = c.get_args().collect();
        assert_eq!(
            args,
            [
                OsStr::new("/r/install.sh"),
                OsStr::new("--tree"),
                OsStr::new("/r"),
                OsStr::new("--tools"),
            ]
        );
    }
}
