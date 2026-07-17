## ADDED Requirements

### Requirement: Codex MUST expose dedicated read_file edit_file write_file tools

When dedicated file tools are enabled for a turn, Codex MUST advertise and
execute first-party function tools named `read_file`, `edit_file`, and
`write_file`. These tools MUST perform filesystem access through Codex-controlled
APIs and MUST NOT require shell execution for ordinary text file read or write
operations.

#### Scenario: read_file returns line-numbered text without shell

- **WHEN** the model calls `read_file` with a workspace file path
- **THEN** Codex reads the file through its filesystem layer
- **AND** the tool result includes file contents with line numbers
- **AND** no `shell_command` or native bash tool is invoked for that read
- **AND** the result reports the 1-based returned range, total line count,
  completeness, and whether the read established a write-eligible receipt

#### Scenario: edit_file applies a unique string replacement

- **WHEN** the model calls `edit_file` with `old_string` that occurs exactly once
  in an existing file and `new_string` that differs
- **THEN** Codex updates the file contents accordingly
- **AND** the mutation is recorded through the same file-change or patch review
  path used by Codex file edits
- **AND** a successful tool result is returned to the model

#### Scenario: write_file creates a new file

- **WHEN** the model calls `write_file` for a path that does not exist with full
  file `content`
- **THEN** Codex creates the file with that content inside allowed workspace
  roots
- **AND** the creation is visible to Codex file-change reporting

#### Scenario: tool path resolves in the selected foreign environment

- **WHEN** a file tool receives a Windows or POSIX path for a selected remote
  environment
- **THEN** Codex resolves it against that environment's `cwd` using `PathUri`
- **AND** filesystem access occurs through that environment's
  `ExecutorFileSystem`
- **AND** Codex does not reinterpret the path using the core host's native
  `PathBuf` convention

### Requirement: edit_file and write_file MUST enforce read-before-write state

Codex MUST maintain bounded, turn-scoped per-path read receipts from successful
`read_file` calls. Receipt identity MUST include the selected environment id and
the executor-canonical `PathUri`.
`edit_file` on an existing file and `write_file` overwrite of an existing file
MUST fail without mutating the file when no prior successful read exists for
that path, or when the file changed since the recorded read.

#### Scenario: edit_file rejects when file was never read

- **WHEN** the model calls `edit_file` on an existing file that has not been
  successfully read with `read_file` in the active turn receipt scope
- **THEN** Codex returns a model-correctable error telling it to read first
- **AND** the file on disk is unchanged

#### Scenario: write_file overwrite rejects stale or missing read

- **WHEN** the model calls `write_file` to overwrite an existing file without a
  current successful `read_file` record for that path
- **THEN** Codex rejects the call without writing
- **AND** the error instructs the model to read the file again

#### Scenario: conflicting mtime with unchanged content still allows edit on Windows-style false dirty

- **WHEN** a file's modification time is newer than the last `read_file` record
  but a full-file read fingerprint still matches the on-disk contents
- **THEN** Codex treats the receipt as still valid and may allow `edit_file`
  or overwrite `write_file`
- **AND** if contents differ, Codex MUST reject the mutation and require a new
  `read_file`

#### Scenario: same mtime with changed content rejects mutation

- **WHEN** the current file content fingerprint differs from the receipt but its
  modification time is unchanged
- **THEN** Codex rejects `edit_file` or overwrite `write_file`
- **AND** no mutation occurs

#### Scenario: receipt scope and storage are bounded

- **WHEN** a new user turn starts or a receipt-store hard limit is reached
- **THEN** receipts from the previous turn are unavailable and store growth
  remains within its configured hard bounds
- **AND** an evicted receipt causes a read-first error rather than bypassing the
  safety check

### Requirement: Successful edit_file and write_file mutations MUST refresh read state

After a successful `edit_file` or `write_file` commit, Codex MUST replace or
create that path's turn-scoped read receipt from the actual committed contents.
The refreshed receipt MUST contain the new fingerprint and full coverage for the
contents produced by Codex. If Codex cannot confirm the committed contents, it
MUST invalidate the old receipt instead of retaining stale authorization.

#### Scenario: edit refreshes receipt for a subsequent edit

- **WHEN** `edit_file` successfully changes an existing file
- **THEN** Codex refreshes the receipt with the committed fingerprint
- **AND** a subsequent `edit_file` in the same user turn can proceed without a
  redundant read when no external change occurred

#### Scenario: create establishes receipt for immediate refinement

- **WHEN** `write_file` successfully creates a new file
- **THEN** Codex creates a full-coverage receipt for the committed contents
- **AND** a following `edit_file` in the same user turn can refine that file
  without first calling `read_file`

#### Scenario: unconfirmed commit invalidates stale receipt

- **WHEN** a mutation reports completion but Codex cannot confirm the actual
  committed contents
- **THEN** Codex invalidates the previous receipt
- **AND** the next mutation requires `read_file` rather than relying on stale
  state

### Requirement: Partial reads MUST support safe targeted edits without enabling blind overwrite

For files below the file-tool editable byte cap, `read_file` MUST compute a
full-file fingerprint even when it returns only an `offset`/`limit` range. A
partial read MAY authorize `edit_file` only for occurrences fully contained in
observed ranges. It MUST NOT authorize whole-file `write_file` overwrite.

#### Scenario: partial read authorizes edit inside observed range

- **WHEN** `read_file` returned a partial range and the unique `old_string`
  occurrence is fully contained in that range
- **AND** the full-file fingerprint still matches
- **THEN** `edit_file` may apply the replacement

#### Scenario: partial read rejects unseen edit or whole-file overwrite

- **WHEN** a requested replacement touches an occurrence outside all observed
  ranges, or `write_file` attempts to overwrite after partial coverage
- **THEN** Codex rejects the mutation and identifies the additional read needed
- **AND** the file remains unchanged

#### Scenario: file above editable cap has non-write-eligible receipt

- **WHEN** `read_file` returns a requested range from a file above the dedicated
  editable byte cap
- **THEN** the output is bounded and the receipt is marked non-write-eligible
- **AND** the error guidance permits a specialized script or shell workflow as
  an explicit exception

### Requirement: edit_file MUST use exact string replacement semantics

`edit_file` MUST replace exact text matches. It MUST NOT accept `apply_patch`
grammar as its primary input contract. Creating brand-new files MUST use
`write_file`, not `edit_file`.

#### Scenario: non-unique old_string fails without replace_all

- **WHEN** `old_string` matches more than once and `replace_all` is false or
  omitted
- **THEN** Codex returns an error that reports the match count
- **AND** no file mutation occurs

#### Scenario: replace_all updates every match

- **WHEN** `old_string` matches multiple times and `replace_all` is true
- **THEN** Codex replaces every match with `new_string`
- **AND** reports success through the normal edit result path

#### Scenario: edit_file does not create missing files

- **WHEN** the model calls `edit_file` for a path that does not exist
- **THEN** Codex returns a model-correctable error
- **AND** suggests using `write_file` to create the file when appropriate

### Requirement: dedicated file writes MUST stay inside Codex safety controls

`edit_file` and `write_file` MUST honor workspace scoping, approval or sandbox
policy, deny rules, and canonical containment equivalent to existing Codex
file-edit controls. Lexical `..` MAY resolve to a legal path inside an allowed
root, but resolved/canonical escape MUST be rejected. Mutations MUST NOT bypass
the reviewable file-change path.

#### Scenario: path outside workspace is rejected

- **WHEN** `edit_file` or `write_file` resolves or canonicalizes outside all
  configured writable roots
- **THEN** Codex rejects the call locally
- **AND** no file mutation occurs

#### Scenario: mutation produces reviewable file change

- **WHEN** a dedicated file tool successfully mutates a workspace file
- **THEN** Codex emits the same class of file-change or patch summary signal
  used by the existing apply-patch edit path for user review and telemetry

#### Scenario: external change during approval prevents stale commit

- **WHEN** a proposed Edit/Write waits for approval and the target changes before
  commit
- **THEN** the commit-time expected-content precondition fails
- **AND** Codex returns a read-again error without overwriting the external change

#### Scenario: concurrent create does not clobber a newly appeared file

- **WHEN** `write_file` planned a create but the target exists by commit time
- **THEN** the no-clobber precondition rejects the create
- **AND** the newly appeared file remains unchanged

### Requirement: Text edits MUST preserve supported encoding and line-ending semantics

`read_file` and `edit_file` MUST share the reviewable mutation path's
round-trip text decoding. Model-visible read text and edit matching MUST use a
documented LF-normalized representation, while successful edits preserve
untouched content, supported encoding, and line endings. Unsupported binary or
non-round-trippable text MUST be rejected rather than silently transcoded.

#### Scenario: edit preserves CRLF and UTF-8 BOM

- **WHEN** `edit_file` changes a uniquely matched string in a UTF-8 BOM file with
  CRLF line endings
- **THEN** the requested content changes
- **AND** the BOM, CRLF convention, and untouched content are preserved

#### Scenario: unsupported encoding is rejected safely

- **WHEN** a file cannot be decoded and encoded losslessly by the shared text
  mutation layer
- **THEN** `read_file` or the requested mutation returns a model-correctable
  unsupported-text error
- **AND** no bytes are modified
