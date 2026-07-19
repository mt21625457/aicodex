use super::*;
use pretty_assertions::assert_eq;

#[cfg(unix)]
fn create_file_symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_file_symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn matching_write_rejects_symlink_without_replacing_it() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let target = temp_dir.path().join("target.txt");
    let link = temp_dir.path().join("link.txt");
    std::fs::write(&target, b"original")?;
    if let Err(error) = create_file_symlink(&target, &link) {
        if cfg!(windows) {
            eprintln!("skipping symlink test because link creation failed: {error}");
            return Ok(());
        }
        return Err(error);
    }

    write_file_conditional(
        &link,
        b"updated",
        ConditionalWritePrecondition::MatchSha256(Sha256::digest(b"original").into()),
    )
    .await
    .expect_err("conditional write must reject symlinks");

    assert!(std::fs::symlink_metadata(&link)?.file_type().is_symlink());
    assert_eq!(std::fs::read(&target)?, b"original");
    Ok(())
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn matching_write_preserves_existing_hard_link() -> io::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let target = temp_dir.path().join("target.txt");
    let link = temp_dir.path().join("link.txt");
    std::fs::write(&target, b"original")?;
    std::fs::hard_link(&target, &link)?;

    write_file_conditional(
        &link,
        b"updated",
        ConditionalWritePrecondition::MatchSha256(Sha256::digest(b"original").into()),
    )
    .await?;

    assert_eq!(std::fs::read(&target)?, b"updated");
    std::fs::write(&target, b"later")?;
    assert_eq!(std::fs::read(&link)?, b"later");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn matching_write_replaces_read_only_file_when_parent_is_writable() -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::TempDir::new()?;
    let path = temp_dir.path().join("read-only.txt");
    std::fs::write(&path, b"original")?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444))?;

    write_file_conditional(
        &path,
        b"updated",
        ConditionalWritePrecondition::MatchSha256(Sha256::digest(b"original").into()),
    )
    .await?;

    assert_eq!(std::fs::read(&path)?, b"updated");
    Ok(())
}
