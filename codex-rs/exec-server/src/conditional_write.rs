use codex_file_system::ConditionalWritePrecondition;
use sha2::Digest;
use sha2::Sha256;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

#[cfg(any(unix, windows))]
use tokio::io::AsyncSeekExt;

#[cfg(any(unix, windows))]
#[derive(Clone, Copy)]
enum FileAccess {
    ReadOnly,
    ReadWrite,
}

#[cfg(any(unix, windows))]
enum HardLinkWriteOutcome {
    Written,
    NoLongerLinked,
}

pub(super) async fn write_file_conditional(
    path: &Path,
    contents: &[u8],
    precondition: ConditionalWritePrecondition,
) -> io::Result<()> {
    match precondition {
        ConditionalWritePrecondition::MustNotExist => {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .await?;
            if let Err(error) = write_and_flush(&mut file, contents).await {
                drop(file);
                let _ = tokio::fs::remove_file(path).await;
                return Err(error);
            }
            Ok(())
        }
        ConditionalWritePrecondition::MatchSha256(expected) => {
            write_file_matching_sha256(path, contents, expected).await
        }
    }
}

async fn write_file_matching_sha256(
    path: &Path,
    contents: &[u8],
    expected: [u8; 32],
) -> io::Result<()> {
    #[cfg(any(unix, windows))]
    {
        let (file, current, metadata) =
            open_and_read_regular_file(path, FileAccess::ReadOnly).await?;
        verify_sha256(&current, expected)?;
        if has_multiple_hard_links(&file, &metadata)? {
            drop(file);
            if let HardLinkWriteOutcome::Written =
                overwrite_hard_link(path, contents, expected).await?
            {
                return Ok(());
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        reject_symlink(path).await?;
        verify_sha256(&tokio::fs::read(path).await?, expected)?;
    }

    let temp_path = sibling_write_temp_path(path)?;
    if let Err(error) = write_exclusive_file(&temp_path, contents).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(error);
    }

    #[cfg(any(unix, windows))]
    {
        let (file, current, metadata) =
            match open_and_read_regular_file(path, FileAccess::ReadOnly).await {
                Ok(current) => current,
                Err(error) => {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    return Err(error);
                }
            };
        if let Err(error) = verify_sha256(&current, expected) {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(error);
        }
        let has_multiple_hard_links = match has_multiple_hard_links(&file, &metadata) {
            Ok(has_multiple_hard_links) => has_multiple_hard_links,
            Err(error) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(error);
            }
        };
        if has_multiple_hard_links {
            drop(file);
            match overwrite_hard_link(path, contents, expected).await {
                Ok(HardLinkWriteOutcome::Written) => {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    return Ok(());
                }
                Ok(HardLinkWriteOutcome::NoLongerLinked) => {}
                Err(error) => {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    return Err(error);
                }
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        if let Err(error) = reject_symlink(path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(error);
        }
        let current = match tokio::fs::read(path).await {
            Ok(current) => current,
            Err(error) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(error);
            }
        };
        if let Err(error) = verify_sha256(&current, expected) {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(error);
        }
    }

    if let Err(error) = tokio::fs::rename(&temp_path, path).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(error);
    }
    Ok(())
}

fn verify_sha256(contents: &[u8], expected: [u8; 32]) -> io::Result<()> {
    if Sha256::digest(contents).as_slice() == expected {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "conditional write conflict: file changed since read",
    ))
}

fn sibling_write_temp_path(path: &Path) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "conditional write path is missing a file name",
        )
    })?;
    let temp_name = format!(
        ".{}.codex-write-{}",
        file_name.to_string_lossy(),
        uuid::Uuid::new_v4()
    );
    Ok(match parent {
        Some(parent) => parent.join(temp_name),
        None => PathBuf::from(temp_name),
    })
}

async fn write_exclusive_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    write_and_flush(&mut file, contents).await
}

async fn write_and_flush(file: &mut tokio::fs::File, contents: &[u8]) -> io::Result<()> {
    file.write_all(contents).await?;
    file.flush().await?;
    Ok(())
}

#[cfg(any(unix, windows))]
async fn open_and_read_regular_file(
    path: &Path,
    access: FileAccess,
) -> io::Result<(tokio::fs::File, Vec<u8>, std::fs::Metadata)> {
    use tokio::io::AsyncReadExt;

    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    if matches!(access, FileAccess::ReadWrite) {
        options.write(true);
    }
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    let mut file = options.open(path).await?;
    let metadata = file.metadata().await?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "conditional write refuses to replace a symbolic link",
        ));
    }
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "conditional write target is not a regular file",
        ));
    }
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut contents).await?;
    Ok((file, contents, metadata))
}

#[cfg(any(unix, windows))]
async fn overwrite_hard_link(
    path: &Path,
    contents: &[u8],
    expected: [u8; 32],
) -> io::Result<HardLinkWriteOutcome> {
    let (file, current, metadata) = open_and_read_regular_file(path, FileAccess::ReadWrite).await?;
    verify_sha256(&current, expected)?;
    if !has_multiple_hard_links(&file, &metadata)? {
        return Ok(HardLinkWriteOutcome::NoLongerLinked);
    }
    overwrite_open_file(file, &current, contents).await?;
    Ok(HardLinkWriteOutcome::Written)
}

#[cfg(unix)]
fn has_multiple_hard_links(
    _file: &tokio::fs::File,
    metadata: &std::fs::Metadata,
) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    Ok(metadata.nlink() > 1)
}

#[cfg(windows)]
fn has_multiple_hard_links(
    file: &tokio::fs::File,
    _metadata: &std::fs::Metadata,
) -> io::Result<bool> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION;
    use windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle;

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    // SAFETY: `information` points to writable storage of the exact structure expected by the
    // Windows API and `file` keeps the supplied handle valid for the duration of the call.
    let result = unsafe {
        GetFileInformationByHandle(file.as_raw_handle() as HANDLE, information.as_mut_ptr())
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful `GetFileInformationByHandle` call initializes the full structure.
    let information = unsafe { information.assume_init() };
    Ok(information.nNumberOfLinks > 1)
}

#[cfg(any(unix, windows))]
async fn overwrite_open_file(
    mut file: tokio::fs::File,
    original: &[u8],
    contents: &[u8],
) -> io::Result<()> {
    file.set_len(0).await?;
    let write_result = async {
        file.seek(io::SeekFrom::Start(0)).await?;
        write_and_flush(&mut file, contents).await
    }
    .await;
    let Err(write_error) = write_result else {
        return Ok(());
    };

    let restore_result = async {
        file.set_len(0).await?;
        file.seek(io::SeekFrom::Start(0)).await?;
        write_and_flush(&mut file, original).await
    }
    .await;
    match restore_result {
        Ok(()) => Err(write_error),
        Err(restore_error) => Err(io::Error::new(
            write_error.kind(),
            format!(
                "conditional hard-link write failed: {write_error}; restoring original contents also failed: {restore_error}"
            ),
        )),
    }
}

#[cfg(not(any(unix, windows)))]
async fn reject_symlink(path: &Path) -> io::Result<()> {
    if tokio::fs::symlink_metadata(path)
        .await?
        .file_type()
        .is_symlink()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "conditional write refuses to replace a symbolic link",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "conditional_write_tests.rs"]
mod tests;
