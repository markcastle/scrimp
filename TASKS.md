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

Findings go in `docs/windows-api.md`.

- [ ] S1: `WofSetFileDataLocation` or `FSCTL_SET_EXTERNAL_BACKING`; minimum access rights; are timestamps preserved; read-only files
- [ ] S2: which API gives the true on-disk size of a WOF file
- [ ] S3: when Windows reports compression as not beneficial, and the state it leaves the file in
- [ ] S4: can a long compression of one large file be cancelled safely
- [ ] S5: behaviour on ReFS, exFAT, 64 KB clusters, mounted VHDX and EFS
- [ ] S6: `IExplorerCommand` from Rust, registered under `HKCU`, on Windows 10 and the Windows 11 full menu; Shift detection; `GetState` when Explorer can't wait (needs a real PC)
- [ ] S7: an installer upgrade while Explorer holds the old DLL (needs a real PC)

### Native layer

- [ ] `scrimp-win`: inspect a file's compression state and sizes
- [ ] `scrimp-win`: compress with each WOF algorithm and with NTFS compression
- [ ] `scrimp-win`: decompress both kinds
- [ ] `scrimp-win`: volume capabilities (filesystem, cluster size, drive type)
- [ ] VHDX fixture volumes created in CI
- [ ] Round-trip tests: every algorithm and every change between algorithms leaves files byte-for-byte identical

## Phase 2: Compression engine

- [ ] Automatic policy table, with change-prone and already-compressed skipping
- [ ] Streaming enumeration that never follows reparse points and counts hardlinks once
- [ ] Scheduler: per-volume worker limits, bounded queue, pause and cancel between files
- [ ] Reports built from outcomes
- [ ] Free-space preflight for decompression

## Phase 3: Command line, released as 0.1

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

- [ ] Nothing to try yet
