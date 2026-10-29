//! `herdr-toolbar unzip <zip> <member> <folder>`: the installer's fallback for
//! `.zip` tool releases on machines with neither `unzip` nor `python3`
//! (minimal Debian, for one). It extracts one named member, keeping its path.

use std::path::{Path, PathBuf};

/// Extracts `member` from the archive at `zip` to `<folder>/<member>`, making
/// parent folders, and returns the written path. A member whose name would
/// leave `folder` (`..`, an absolute path) is refused.
pub fn extract_member(zip: &Path, member: &str, folder: &Path) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let file = std::fs::File::open(zip).with_context(|| format!("opening {}", zip.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).with_context(|| format!("reading {}", zip.display()))?;
    let mut entry = archive
        .by_name(member)
        .with_context(|| format!("{} has no `{member}`", zip.display()))?;
    let rel = entry
        .enclosed_name()
        .with_context(|| format!("`{member}` would land outside {}", folder.display()))?;
    let out = folder.join(rel);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut w =
        std::fs::File::create(&out).with_context(|| format!("writing {}", out.display()))?;
    std::io::copy(&mut entry, &mut w).with_context(|| format!("unpacking `{member}`"))?;
    if let Some(mode) = entry.unix_mode() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o777))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn archive(dir: &Path, entries: &[(&str, &str)]) -> PathBuf {
        let path = dir.join("t.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap();
        path
    }

    #[test]
    fn extracts_one_member_into_its_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = archive(
            tmp.path(),
            &[("yazi-x/yazi", "the binary"), ("yazi-x/README", "docs")],
        );
        let out = tmp.path().join("out");
        let written = extract_member(&zip, "yazi-x/yazi", &out).unwrap();
        assert_eq!(written, out.join("yazi-x/yazi"));
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "the binary");
        assert!(
            !out.join("yazi-x/README").exists(),
            "only the member asked for"
        );
    }

    #[test]
    fn a_missing_member_is_an_error_that_names_it() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = archive(tmp.path(), &[("procs", "x")]);
        let e = extract_member(&zip, "nope", &tmp.path().join("out")).unwrap_err();
        assert!(format!("{e:#}").contains("nope"), "{e:#}");
    }

    #[test]
    fn a_member_that_would_leave_the_folder_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = archive(tmp.path(), &[("../evil", "x")]);
        let out = tmp.path().join("out");
        assert!(extract_member(&zip, "../evil", &out).is_err());
        assert!(!tmp.path().join("evil").exists());
    }
}
