# Scrimp: Tasks

Work top to bottom. Each task is done when its tests pass, clippy is clean and coverage holds. Tick tasks off in the same commit that completes them. The reasoning behind each phase is in [docs/plan.md](docs/plan.md#11-roadmap).

## Phase 1: Repository, CI and native layer

### Repository and release machinery

- [x] Workspace, toolchain, lints and project conventions, following Keevoro's
- [x] `cargo xtask` release and changelog commands, ported from Keevoro
- [x] CI, Release and Roll back workflows, Windows only
- [x] `install.ps1` that adds Scrimp to the user's PATH correctly
- [x] `scrimp-core` types: algorithms, targets, states and outcomes
- [x] `scrimp --version`
- [ ] A rehearsal run of the Release workflow passes (Actions → Release → tick Rehearse)

### Phase 0 questions, answered on GitHub's Windows runners where possible

Findings are in [docs/windows-api.md](docs/windows-api.md).

- [x] S1: `WofSetFileDataLocation` or `FSCTL_SET_EXTERNAL_BACKING`; minimum access rights; are timestamps preserved; read-only files
- [x] S2: which API gives the true on-disk size of a WOF file
- [x] S3: when Windows reports compression as not beneficial, and the state it leaves the file in
- [x] S4: can a long compression of one large file be cancelled safely
- [x] S5: behaviour on ReFS, exFAT, 64 KB clusters, mounted VHDX and EFS
- [ ] S6: `IExplorerCommand` from Rust, registered under `HKCU`, on Windows 10 and the Windows 11 full menu; Shift detection; `GetState` when Explorer can't wait (needs a real PC)
- [ ] S7: an installer upgrade while Explorer holds the old DLL (needs a real PC)

### Native layer

Each of these follows [docs/windows-api.md](docs/windows-api.md).

- [x] `scrimp-win`: inspect a file's compression state and sizes (`WofIsExternalFile` with `WOF_FILE_COMPRESSION_INFO_V1`, `FSCTL_GET_COMPRESSION`, `GetCompressedFileSizeW`); a file with both WOF and NTFS compression reads as unknown
- [x] `scrimp-win`: compress with each WOF algorithm through `WofSetFileDataLocation`, opening with `GENERIC_READ` only, with a test that reads each algorithm back
- [x] `scrimp-win`: NTFS compression, clearing and restoring read-only, and undoing it when nothing was saved
- [x] `scrimp-win`: remove NTFS compression before applying WOF
- [x] `scrimp-win`: decompress both kinds, with read-only access for WOF
- [x] `scrimp-win`: volume capabilities from the filesystem name and cluster size, never from error codes alone (ReFS answers "would not save space")
- [x] `scrimp-win`: recognise "WOF isn't running on this drive" (error 1 from a WOF call becomes `Error::WofUnavailable`)
- [x] Engine: stop trying WOF on a drive for the rest of the job after `WofUnavailable`, and say NTFS compression still works there
- [x] Round-trip tests on the system drive: every algorithm and every change between algorithms leaves files byte-for-byte identical
- [x] VHDX test drives in CI (64 KB clusters, exFAT, ReFS, and NTFS with and without WOF attached), with `scrimp-win` tests against each

## Phase 2: Compression engine

- [x] Automatic policy table, with change-prone and already-compressed skipping, protected Windows folders and per-extension overrides (`scrimp-core::policy`)
- [x] Streaming enumeration that never follows links and handles a hardlinked file once (`scrimp-core::engine`)
- [x] Scheduler: per-drive worker limits (spinning disk 1, SSD 4), bounded queues, pause and cancel between files
- [x] Reports built from outcomes, in the plain words of the plan, with sizes as Explorer shows them (`scrimp-core::report`)
- [x] Free-space check for decompression, keeping the larger of 1 GB or 2% free (`scrimp-core::space`)
- [x] Engine: check there is room before decompressing each file

## Phase 3: Command line, released as 0.1

- [ ] Settings file: `settings.json` read and validated, feeding the policy and worker limits

- [ ] `scrimp compress`, `decompress`, `inspect` and `analyse`
- [ ] `--json` output and documented exit codes
- [ ] Benchmark against `compact.exe`
- [ ] First release with the Release button

## Phase 4: Explorer menu, released as 0.2

- [ ] `scrimp-shell`: Scrimp Compress and Scrimp Decompress, registered under `HKCU`
- [ ] `scrimpw`: one worker per user, jobs from the menu, task dialog progress
- [ ] Shell test host: menu builds in under 50 ms, and an injected panic leaves Explorer running

## Phase 5: Interface, installer and updates, released as 0.3

- [ ] Results dialog, Save report, Try Again as Administrator, Scrimp Savings…, first-use message
- [ ] NSIS installer, per user, with `--register` and `--unregister`
- [ ] `scrimp update`

## Phase 6: Hardening, released as 1.0

- [ ] Everything in the MVP definition of done, [docs/plan.md section 17](docs/plan.md#17-mvp-definition-of-done)

## Tried by a person

Things CI can't prove. Tick them when someone has tried them on a real PC.

- [ ] Does Windows 10 or 11 attach WOF by itself to a VHDX mounted after boot? (On Windows Server it doesn't.)
- [ ] Is `COMPRESS_ON_WRITE` accepted on Windows 11 24H2 or 25H2, and does a written file stay compressed?
- [ ] After WOF compresses an EFS-encrypted file, is it still encrypted and readable?
- [ ] How fast is LZX on a typical desktop, against the runner's 10 MiB per second?

`spikes/wof-probe` answers most of these; see [docs/windows-api.md](docs/windows-api.md#running-the-probe-yourself).
