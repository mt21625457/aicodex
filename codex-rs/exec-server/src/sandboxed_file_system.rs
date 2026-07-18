use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use codex_exec_server_protocol::JSONRPCErrorError;
use codex_utils_path_uri::PathUri;
use tokio::io;

use crate::ConditionalWritePrecondition;
use crate::CopyOptions;
use crate::CreateDirectoryOptions;
use crate::ExecServerRuntimePaths;
use crate::ExecutorFileSystem;
use crate::ExecutorFileSystemFuture;
use crate::FILE_READ_CHUNK_SIZE;
use crate::FileMetadata;
use crate::FileSystemReadStream;
use crate::FileSystemResult;
use crate::FileSystemSandboxContext;
use crate::ReadDirectoryEntry;
use crate::RemoveOptions;
use crate::WalkOptions;
use crate::WalkOutcome;
use crate::fs_helper::FsHelperPayload;
use crate::fs_helper::FsHelperRequest;
use crate::fs_sandbox::FileSystemSandboxRunner;
use crate::protocol::FsCanonicalizeParams;
use crate::protocol::FsConditionalWriteFileParams;
use crate::protocol::FsConditionalWritePrecondition;
use crate::protocol::FsCopyParams;
use crate::protocol::FsCreateDirectoryParams;
use crate::protocol::FsGetMetadataParams;
use crate::protocol::FsReadDirectoryParams;
use crate::protocol::FsReadFileBlockParams;
use crate::protocol::FsReadFileParams;
use crate::protocol::FsRemoveParams;
use crate::protocol::FsWalkParams;
use crate::protocol::FsWriteFileParams;

#[derive(Clone)]
pub struct SandboxedFileSystem {
    sandbox_runner: FileSystemSandboxRunner,
}

impl SandboxedFileSystem {
    pub fn new(runtime_paths: ExecServerRuntimePaths) -> Self {
        Self {
            sandbox_runner: FileSystemSandboxRunner::new(runtime_paths),
        }
    }

    async fn run_sandboxed(
        &self,
        sandbox: &FileSystemSandboxContext,
        request: FsHelperRequest,
    ) -> FileSystemResult<FsHelperPayload> {
        self.sandbox_runner
            .run(sandbox, request)
            .await
            .map_err(map_sandbox_error)
    }
}

impl SandboxedFileSystem {
    async fn canonicalize(
        &self,
        path: &PathUri,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<PathUri> {
        let sandbox = require_platform_sandbox(sandbox)?;
        validate_native_path(path)?;
        let response = self
            .run_sandboxed(
                sandbox,
                FsHelperRequest::Canonicalize(FsCanonicalizeParams {
                    path: path.clone(),
                    sandbox: None,
                }),
            )
            .await?
            .expect_canonicalize()
            .map_err(map_sandbox_error)?;
        Ok(response.path)
    }

    async fn read_file(
        &self,
        path: &PathUri,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<u8>> {
        let sandbox = require_platform_sandbox(sandbox)?;
        validate_native_path(path)?;
        let response = self
            .run_sandboxed(
                sandbox,
                FsHelperRequest::ReadFile(FsReadFileParams {
                    path: path.clone(),
                    sandbox: None,
                }),
            )
            .await?
            .expect_read_file()
            .map_err(map_sandbox_error)?;
        STANDARD.decode(response.data_base64).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("fs/readFile returned invalid base64 dataBase64: {err}"),
            )
        })
    }

    pub(crate) async fn read_file_block(
        &self,
        path: &PathUri,
        offset: u64,
        len: usize,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<(Vec<u8>, bool)> {
        let sandbox = require_platform_sandbox(sandbox)?;
        validate_native_path(path)?;
        let response = self
            .run_sandboxed(
                sandbox,
                FsHelperRequest::ReadFileBlock(FsReadFileBlockParams {
                    path: path.clone(),
                    offset,
                    len,
                    sandbox: None,
                }),
            )
            .await?
            .expect_read_file_block()
            .map_err(map_sandbox_error)?;
        Ok((response.chunk.into_inner(), response.eof))
    }

    async fn read_file_stream(
        &self,
        path: &PathUri,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<FileSystemReadStream> {
        let sandbox = require_platform_sandbox(sandbox)?.clone();
        validate_native_path(path)?;
        let file_system = self.clone();
        let path = path.clone();
        Ok(FileSystemReadStream::new(futures::stream::try_unfold(
            Some(0_u64),
            move |offset| {
                let file_system = file_system.clone();
                let sandbox = sandbox.clone();
                let path = path.clone();
                async move {
                    let Some(offset) = offset else {
                        return Ok(None);
                    };
                    let (bytes, eof) = file_system
                        .read_file_block(&path, offset, FILE_READ_CHUNK_SIZE, Some(&sandbox))
                        .await?;
                    let chunk = Bytes::from(bytes);
                    if eof {
                        return if chunk.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some((chunk, None)))
                        };
                    }
                    if chunk.is_empty() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "sandbox helper returned an empty non-terminal file block",
                        ));
                    }
                    let next_offset = offset.checked_add(chunk.len() as u64).ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("sandbox file read offset overflowed after {offset} bytes"),
                        )
                    })?;
                    Ok(Some((chunk, Some(next_offset))))
                }
            },
        )))
    }

    async fn write_file(
        &self,
        path: &PathUri,
        contents: Vec<u8>,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        let sandbox = require_platform_sandbox(sandbox)?;
        validate_native_path(path)?;
        self.run_sandboxed(
            sandbox,
            FsHelperRequest::WriteFile(FsWriteFileParams {
                path: path.clone(),
                data_base64: STANDARD.encode(contents),
                sandbox: None,
            }),
        )
        .await?
        .expect_write_file()
        .map_err(map_sandbox_error)?;
        Ok(())
    }

    async fn write_file_conditional(
        &self,
        path: &PathUri,
        contents: Vec<u8>,
        precondition: ConditionalWritePrecondition,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        let sandbox = require_platform_sandbox(sandbox)?;
        validate_native_path(path)?;
        let precondition = match precondition {
            ConditionalWritePrecondition::MustNotExist => {
                FsConditionalWritePrecondition::MustNotExist
            }
            ConditionalWritePrecondition::MatchSha256(digest) => {
                FsConditionalWritePrecondition::MatchSha256 { digest }
            }
        };
        self.run_sandboxed(
            sandbox,
            FsHelperRequest::ConditionalWriteFile(FsConditionalWriteFileParams {
                path: path.clone(),
                data_base64: STANDARD.encode(contents),
                precondition,
                sandbox: None,
            }),
        )
        .await?
        .expect_conditional_write_file()
        .map_err(map_sandbox_error)?;
        Ok(())
    }

    async fn create_directory(
        &self,
        path: &PathUri,
        options: CreateDirectoryOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        let sandbox = require_platform_sandbox(sandbox)?;
        validate_native_path(path)?;
        self.run_sandboxed(
            sandbox,
            FsHelperRequest::CreateDirectory(FsCreateDirectoryParams {
                path: path.clone(),
                recursive: Some(options.recursive),
                sandbox: None,
            }),
        )
        .await?
        .expect_create_directory()
        .map_err(map_sandbox_error)?;
        Ok(())
    }

    async fn get_metadata(
        &self,
        path: &PathUri,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<FileMetadata> {
        let sandbox = require_platform_sandbox(sandbox)?;
        validate_native_path(path)?;
        let response = self
            .run_sandboxed(
                sandbox,
                FsHelperRequest::GetMetadata(FsGetMetadataParams {
                    path: path.clone(),
                    sandbox: None,
                }),
            )
            .await?
            .expect_get_metadata()
            .map_err(map_sandbox_error)?;
        Ok(FileMetadata {
            is_directory: response.is_directory,
            is_file: response.is_file,
            is_symlink: response.is_symlink,
            size: response.size,
            created_at_ms: response.created_at_ms,
            modified_at_ms: response.modified_at_ms,
        })
    }

    async fn read_directory(
        &self,
        path: &PathUri,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<ReadDirectoryEntry>> {
        let sandbox = require_platform_sandbox(sandbox)?;
        validate_native_path(path)?;
        let response = self
            .run_sandboxed(
                sandbox,
                FsHelperRequest::ReadDirectory(FsReadDirectoryParams {
                    path: path.clone(),
                    sandbox: None,
                }),
            )
            .await?
            .expect_read_directory()
            .map_err(map_sandbox_error)?;
        Ok(response
            .entries
            .into_iter()
            .map(|entry| ReadDirectoryEntry {
                file_name: entry.file_name,
                is_directory: entry.is_directory,
                is_file: entry.is_file,
            })
            .collect())
    }

    async fn walk(
        &self,
        path: &PathUri,
        options: WalkOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<WalkOutcome> {
        let sandbox = require_platform_sandbox(sandbox)?;
        validate_native_path(path)?;
        let response = self
            .run_sandboxed(
                sandbox,
                FsHelperRequest::Walk(FsWalkParams {
                    path: path.clone(),
                    options,
                    sandbox: None,
                }),
            )
            .await?
            .expect_walk()
            .map_err(map_sandbox_error)?;
        Ok(response)
    }

    async fn remove(
        &self,
        path: &PathUri,
        remove_options: RemoveOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        let sandbox = require_platform_sandbox(sandbox)?;
        validate_native_path(path)?;
        self.run_sandboxed(
            sandbox,
            FsHelperRequest::Remove(FsRemoveParams {
                path: path.clone(),
                recursive: Some(remove_options.recursive),
                force: Some(remove_options.force),
                sandbox: None,
            }),
        )
        .await?
        .expect_remove()
        .map_err(map_sandbox_error)?;
        Ok(())
    }

    async fn copy(
        &self,
        source_path: &PathUri,
        destination_path: &PathUri,
        options: CopyOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        let sandbox = require_platform_sandbox(sandbox)?;
        validate_native_path(source_path)?;
        validate_native_path(destination_path)?;
        self.run_sandboxed(
            sandbox,
            FsHelperRequest::Copy(FsCopyParams {
                source_path: source_path.clone(),
                destination_path: destination_path.clone(),
                recursive: options.recursive,
                sandbox: None,
            }),
        )
        .await?
        .expect_copy()
        .map_err(map_sandbox_error)?;
        Ok(())
    }
}

impl ExecutorFileSystem for SandboxedFileSystem {
    fn canonicalize<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, PathUri> {
        Box::pin(SandboxedFileSystem::canonicalize(self, path, sandbox))
    }

    fn read_file<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<u8>> {
        Box::pin(SandboxedFileSystem::read_file(self, path, sandbox))
    }

    fn read_file_stream<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileSystemReadStream> {
        Box::pin(SandboxedFileSystem::read_file_stream(self, path, sandbox))
    }

    fn write_file<'a>(
        &'a self,
        path: &'a PathUri,
        contents: Vec<u8>,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(SandboxedFileSystem::write_file(
            self, path, contents, sandbox,
        ))
    }

    fn write_file_conditional<'a>(
        &'a self,
        path: &'a PathUri,
        contents: Vec<u8>,
        precondition: ConditionalWritePrecondition,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(SandboxedFileSystem::write_file_conditional(
            self,
            path,
            contents,
            precondition,
            sandbox,
        ))
    }

    fn create_directory<'a>(
        &'a self,
        path: &'a PathUri,
        options: CreateDirectoryOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(SandboxedFileSystem::create_directory(
            self, path, options, sandbox,
        ))
    }

    fn get_metadata<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileMetadata> {
        Box::pin(SandboxedFileSystem::get_metadata(self, path, sandbox))
    }

    fn read_directory<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<ReadDirectoryEntry>> {
        Box::pin(SandboxedFileSystem::read_directory(self, path, sandbox))
    }

    fn walk<'a>(
        &'a self,
        path: &'a PathUri,
        options: WalkOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, WalkOutcome> {
        Box::pin(SandboxedFileSystem::walk(self, path, options, sandbox))
    }

    fn remove<'a>(
        &'a self,
        path: &'a PathUri,
        remove_options: RemoveOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(SandboxedFileSystem::remove(
            self,
            path,
            remove_options,
            sandbox,
        ))
    }

    fn copy<'a>(
        &'a self,
        source_path: &'a PathUri,
        destination_path: &'a PathUri,
        options: CopyOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(SandboxedFileSystem::copy(
            self,
            source_path,
            destination_path,
            options,
            sandbox,
        ))
    }
}

fn validate_native_path(path: &PathUri) -> FileSystemResult<()> {
    path.to_abs_path().map(drop)
}

fn require_platform_sandbox(
    sandbox: Option<&FileSystemSandboxContext>,
) -> FileSystemResult<&FileSystemSandboxContext> {
    sandbox
        .filter(|sandbox| sandbox.should_run_in_sandbox())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "sandboxed filesystem operations require ReadOnly or WorkspaceWrite sandbox policy",
            )
        })
}

fn map_sandbox_error(error: JSONRPCErrorError) -> io::Error {
    match error.code {
        -32004 => io::Error::new(io::ErrorKind::NotFound, error.message),
        crate::rpc::FILE_CONFLICT_ERROR_CODE => {
            io::Error::new(io::ErrorKind::InvalidData, error.message)
        }
        -32600 => io::Error::new(io::ErrorKind::InvalidInput, error.message),
        _ => io::Error::other(error.message),
    }
}

#[cfg(all(test, any(unix, windows)))]
#[path = "sandboxed_file_system_path_uri_tests.rs"]
mod path_uri_tests;
