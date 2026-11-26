# Scrimp: Implementation Plan

> The missing native interface to Windows filesystem compression, built into Explorer.

Plan version 0.2 · 26 November 2026 · Builds on the original "Native Windows Compression Utility" development plan and reuses Keevoro's build and release setup

## How to read this

This plan keeps the vision, principles and most of the architecture from the original plan. It changes the parts where Windows' own behaviour should shape the design, cuts work that Windows already does for us, and turns the open questions into a short list of spikes we answer before writing production code.

The build, test and release machinery comes from Keevoro, which already works: the Release button, changelog-driven version numbers, checksums, rollback, the coverage gate and the project conventions. Section 13 lists exactly what is copied and what changes.

Each section says what we're building, why, and what we don't know yet. Section 2 is the most important one. It covers how WOF and NTFS compression behave in practice, and those behaviours drive most of the decisions that follow.

## 1. What changed from the original plan

| Area | Original plan | Revised plan | Why |
| --- | --- | --- | --- |
| Explorer integration | Static registry menu first, COM extension later | One `IExplorerCommand` DLL, registered two ways | Static verbs start one process per selected file. A single COM handler covers the Windows 10 menu, Windows 11 "Show more options" and the Windows 11 modern menu. |
| Worker processes | One worker per invocation | One worker per user, accepting many jobs | Two workers on the same HDD would break the one-worker-per-HDD rule |
| Menu | Cascading "Compression" menu listing algorithms | Two top-level items, "Scrimp Compress" and "Scrimp Decompress"; algorithms behind Shift | On brand, distinct from Windows' archive menu, one click instead of two, no algorithm jargon |
| Progress UI | Custom Win32 windows | TaskDialog in the MVP; Explorer's own copy-progress dialog later | Native look, accessibility and DPI scaling for free, and far less code |
| Binaries | Worker and CLI | `scrimp.exe` (console) and `scrimpw.exe` (GUI), like `python` / `pythonw` | GUI-subsystem worker never flashes a console window |
| Install | Machine-wide WiX or NSIS | Per-user NSIS installer, no admin, as Keevoro does; the app registers its own menu | Matches the "no admin for ordinary use" principle and reuses what already works in Keevoro |
| Build and release | To be designed | Keevoro's workflows and `cargo xtask` commands, adapted | Proven release plumbing on day one |
| Automatic policy | File-type table | File-type table plus "change-prone" exclusions | WOF files silently decompress when written to |
| Windows 10 | Compatibility target | First target for the MVP, together with the Windows 11 full menu | The registry route needs no signing, so it ships first; the Windows 11 modern menu follows in 1.1 once signing is in place |
| First release | 1.0 with everything | 0.1 ships the CLI | Real users test the compression engine before any Explorer code exists |
| Updates | Background update checks | `scrimp update`, run by the user, never in the background | Same approach as `keevoro update`; no network access unless asked |
| PATH | No changes | One entry on the user's PATH, added and removed correctly; system PATH never touched | `scrimp` works in any terminal, as `keevoro` does |
| Licence | MIT proposed | MIT | Decided |

## 2. How Windows compression behaves

These behaviours aren't obvious from the API docs and each one affects the design. Phase 0 tested them on real Windows; the full findings, including several surprises, are in [windows-api.md](windows-api.md).

### WOF-compressed files decompress when written to

WOF (XPRESS and LZX) is designed for files that are written once and read many times. When an application writes to a WOF-compressed file, Windows decompresses the whole file and it stays uncompressed.

What this means for Scrimp:

- WOF suits programs, games, documentation, archives of old projects and photo libraries.
- WOF is a poor fit for databases, virtual disks, mail stores and log files. Compressing them wastes time and gives the user a false picture of their savings.
- Automatic mode skips change-prone files by default (see section 6).
- Game updates and app updates will undo compression on the files they replace. The UI should say so in plain words rather than letting users discover it.

### LZNT1 (classic NTFS compression) handles writes but has costs

LZNT1 compresses in 64 KB units, so files stay compressed when modified. It is the right choice for files that change.

- It requires a cluster size of 4 KB or smaller. Volumes formatted with 64 KB clusters can't use it.
- Large files compressed with LZNT1 fragment badly, which hurts on HDDs.
- The ratio is usually worse than XPRESS8K.

### Folders inherit LZNT1 but not WOF

Setting LZNT1 on a folder marks it so new files are compressed automatically. WOF has no such flag. "Compress this folder with LZX" compresses the files that exist today and nothing added later. The completion message must say this. A folder watcher is a future feature (section 18).

### Only NTFS supports this

WOF and LZNT1 work on NTFS only. Scrimp must detect and explain the unsupported cases:

- ReFS, including Windows 11 **Dev Drives**, which many developers now use for source code.
- FAT32 and exFAT, which covers most USB sticks and SD cards.
- Network shares (verify how SMB to an NTFS server behaves).

### Hardlinks share one copy of the data

Compressing one name of a hardlinked file compresses all of them. Count the data once in reports, and don't compress the same file twice when two links are in the same selection.

### Decompressing needs room

Decompressing 50 GB of LZX data that occupies 20 GB on disk needs 30 GB of free space. The preflight check sums logical sizes and refuses to start a decompression that would fill the volume. It also keeps a reserve (for example the larger of 1 GB or 2%) so we never leave a system drive full.

### System files may already be compressed by Windows

CompactOS and WIM-backed files appear as WOF files with a different provider. Scrimp reports them and never changes them automatically.

### Compression is a blocking call per file

LZX ran at about 10 MiB per second on GitHub's runner, so a 1 GiB file is one call of well over a minute. Phase 0 found no way to cancel it part-way: overlapped I/O and `CancelSynchronousIo` both let it run to the end. Cancellation takes effect between files, and the UI says which file it is finishing.

### Measuring "size on disk"

`GetCompressedFileSizeW` matched the free space actually used, to the byte, for every method.

### Opening a WOF file for writing undoes it

Phase 0 found that simply opening a WOF-compressed file with write access decompresses it. Scrimp opens files read-only for all WOF work, which is all WOF needs.

### WOF needs its driver attached to the volume

On Windows Server, a VHDX mounted after boot had no WOF driver attached, and every WOF call failed, `compact.exe`'s included. Scrimp detects this and explains it rather than failing every file. Whether Windows 10 and 11 desktops behave the same is still to be tried.

## 3. Architecture

```text
explorer.exe
  └── scrimp_shell.dll        IExplorerCommand: reads the selection, builds the menu,
          │                   writes a job file, starts the worker. Never compresses.
          │ job file + CreateProcess
          ▼
scrimpw.exe                   GUI subsystem, one instance per user
  ├── job intake              later invocations hand their jobs over a named pipe
  ├── scheduler               one queue and worker limit per physical disk
  ├── progress and results UI
  └── scrimp-core ──► scrimp-win ──► Windows (WOF, NTFS, storage IOCTLs)

scrimp.exe                    console CLI, same engine
  └── scrimp-core ──► scrimp-win
```

### Crates

| Crate | Builds | Responsibility | `unsafe` |
| --- | --- | --- | --- |
| `scrimp-win` | lib | The only crate that calls Windows APIs. Safe wrappers, RAII handles, UTF-16 paths, error translation, user PATH and registry helpers. | Allowed, each block documented with its safety invariant |
| `scrimp-core` | lib | Algorithms, compression state, policy, traversal planning, scheduler, outcomes, progress events, reports | Forbidden |
| `scrimp-shell` | cdylib | COM exports, class factory, `IExplorerCommand`, selection reading, job launch | Allowed, only at the COM boundary |
| `scrimp-update` | lib | Checks GitHub Releases, compares versions, downloads and verifies the installer. Adapted from `keevoro-update`. | Forbidden |
| `scrimpw` | bin | Worker: single instance, job intake, UI, settings, `--register` and `--unregister` | Forbidden; calls go through `scrimp-win` |
| `scrimp-cli` | bin `scrimp.exe` | Command-line interface, including `scrimp update` | Forbidden |
| `xtask` | bin | Keevoro's release and changelog commands, plus dev registration, test fixtures and the installer build | Forbidden |

`unsafe_code` is set to `deny` for the whole workspace, as strict as Keevoro's `forbid` everywhere it matters. The two crates that must call Windows directly opt out with a crate-level `#![allow(unsafe_code)]`, and `clippy::undocumented_unsafe_blocks` makes every block carry its safety comment.

`scrimp-core` talks to the filesystem through one small trait. The real implementation lives in `scrimp-win`; tests use an in-memory fake. That lets policy, scheduling and reporting logic be tested on any OS, including the Linux machines some contributors will use.

### Core types (sketch, not final signatures)

```rust
pub enum Algorithm { Lznt1, Xpress4k, Xpress8k, Xpress16k, Lzx }

pub enum Target { Auto, Algorithm(Algorithm), Uncompressed }

pub enum CompressionState {
    Uncompressed,
    Ntfs,
    Wof(Algorithm),
    WimBacked,
    Unsupported(UnsupportedReason),
    Unknown,
}

pub enum Outcome {
    Compressed { logical: u64, before: u64, after: u64 },
    Decompressed { logical: u64, before: u64, after: u64 },
    AlreadyInTarget,
    NotBeneficial,                 // Windows tried and found no saving
    Skipped(SkipReason),           // policy, protected path, cloud file, hardlink seen
    Failed(FailReason),            // access denied, locked, no space, gone, Windows error
    Cancelled,
}
```

A job is a list of `Outcome`s plus totals. The UI and CLI render outcomes; neither decides them.

### Scheduler

The scheduler turns a selection into file tasks and runs them with storage-aware limits.

1. **Resolve physical disks.** For each volume: `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` gives the disk number(s). For a mounted VHDX, `GetStorageDependencyInformation` gives the host volume, and we resolve that in turn. Two VHDX files on the same NVMe drive share one queue.
2. **Classify each disk.** `IOCTL_STORAGE_QUERY_PROPERTY` gives seek penalty (HDD or SSD) and bus type (NVMe, SATA, USB).
3. **Set the worker limit** to the smaller of the storage limit (HDD 1, USB 1–2, SATA SSD 2–4, NVMe 4–8, unknown 1) and the CPU limit (logical cores minus one; LZX is CPU-bound).
4. **Stream the work.** One enumerator thread per root walks directories and pushes tasks into a bounded `std::sync::mpsc::sync_channel` (capacity about twice the worker count). Memory stays flat whether the selection has ten files or ten million.
5. **Enumerate cheaply.** Open each directory once and read entries in batches with `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)`. That gives attributes, reparse tags, file IDs and sizes, so most skip decisions happen without opening files. Directory reparse points are never followed.

Pause stops dispatching new files. Cancel stops dispatching and waits for in-flight files to finish.

### One worker per user

The first `scrimpw.exe` creates a named mutex and a named pipe, both scoped to the user's SID with an ACL that only that user can open. Later invocations send their job path through the pipe and exit. The running worker queues the job behind any work already on the same disk and shows it in the same progress UI.

### Shell extension rules

The DLL runs inside Explorer. If it crashes, the taskbar and every Explorer window go with it.

- Wrap every exported function and COM method in `catch_unwind` and return `E_UNEXPECTED` on panic.
- Keep `panic = "unwind"` in every Cargo profile. Panic strategy is set per profile for the whole workspace, so `panic = "abort"` anywhere would remove the safety net from the DLL.
- In `GetState`, return `E_PENDING` when `fOkToBeSlow` is false. Explorer calls back later when slow work is acceptable.
- Only query compression state for a single item on a local fixed disk. Decide "network" from the path string alone; never touch the network from the menu.
- No logging framework or background threads in the DLL. Use `OutputDebugStringW` in debug builds.
- Find `scrimpw.exe` relative to the DLL's own path (`GetModuleFileNameW` on the module handle). No registry lookups, no PATH.
- Implement `DllCanUnloadNow` with a live-object count so Explorer can release the DLL.

### Job files

The DLL writes `%LOCALAPPDATA%\Scrimp\jobs\<random>.json` with `CREATE_NEW`, then starts the worker with `--job <path>`. The worker validates the file (schema version, absolute paths, no device paths), deletes it, and runs the job. Paths never pass through a command line or a shell.

```json
{
  "schemaVersion": 1,
  "operation": "compress",
  "target": "auto",
  "paths": ["D:\\Games\\Example", "D:\\Archive\\2019.dat"]
}
```

When the user chooses "Retry as administrator", the elevated worker shows the paths it is about to process before starting, rather than trusting a file a non-elevated process wrote.

## 4. Explorer menu

### Design rules

People don't think in algorithms. They want their files to take up less room, and they want to be able to undo it. The menu speaks in those terms, makes a good choice on the user's behalf, and keeps the technical options out of sight but within reach.

- **Branded, top-level items with no submenu:** "Scrimp Compress" and "Scrimp Decompress", with the Scrimp icon. The brand prefix sets them apart from Windows 11's own "Compress to ▸" archive menu (ZIP, 7z, TAR). The words match the CLI commands (`scrimp compress`, `scrimp decompress`).
- **Show only what makes sense.** The items change with the state of the selection.
- **No algorithm names where everyday users look.** XPRESS, LZX and LZNT1 appear only in the CLI, in Settings, and behind a "Details" disclosure.
- **Hide what can't work.** On a drive that can't save space this way, Scrimp adds nothing to the menu instead of showing greyed-out items.
- **Power options appear when Shift is held**, like Option-click menus on a Mac.
### What people see

```text
Scrimp Compress
Scrimp Decompress
```

| Selection | Menu items |
| --- | --- |
| Nothing compressed yet | **Scrimp Compress** |
| Everything already compressed | **Scrimp Decompress** |
| A mix, or state not cheaply known (folders, drives, large selections) | **Scrimp Compress** and **Scrimp Decompress** |
| FAT32, exFAT, ReFS or Dev Drive, network location | No Scrimp items at all |

"Scrimp Compress" uses Automatic mode (section 6).

### Shift + right-click

```text
Scrimp Compress ▸
    Automatic                  default
    Faster to Open             XPRESS4K
    Smallest                   LZX
    For Files That Change      NTFS (LZNT1)
Scrimp Decompress
Scrimp Savings…
```

The right-hand column is for developers reading this plan; users see only the labels. XPRESS8K and XPRESS16K have no menu entry. Automatic uses them, and they remain available in Settings and the CLI.

On Windows 11, Shift + right-click opens the classic menu directly. The handler is registered there too, so it checks the Shift key state when building its commands.

### Words people see

| Situation | Scrimp says |
| --- | --- |
| First use, once only | "Scrimp keeps your files exactly as they are, just smaller on disk. Apps open them as normal. Use Scrimp Decompress to undo it at any time." |
| Finished | "Saved 35.2 GB. Games now takes up 51.2 GB instead of 86.4 GB." |
| Not beneficial, or already-compressed media | "862 files are already as small as they can be." |
| Change-prone file skipped | "14 files change often, so Windows would undo the saving." |
| Locked | "3 files are in use by another app." |
| Access denied | "Windows didn't allow changes to 12 files." with a **Try Again as Administrator** button |
| Folder compressed with WOF | "New files added to this folder later won't be compressed automatically." |
| Dev Drive or other ReFS volume | "Dev Drives can't save space this way." |

Technical detail (method, provider, Windows error codes) lives behind a **Details** disclosure and in saved reports.

### Open questions for Phase 0

- Can "Scrimp Compress" be a plain item normally and a submenu when Shift is held? If not, add the Shift options as flat items such as "Scrimp Compress: Smallest".
- Is reading the compression state cheap enough to choose between Compress and Decompress for a single local file? If not, show both.

For 1.1: can one sparse package add two top-level items to the Windows 11 modern menu (two verbs, two CLSIDs), and can each hide itself based on the selection? If not, the modern menu gets a single "Scrimp ▸" submenu holding Compress and Decompress.

### One handler, two registrations, Windows 10 first

The same COM class serves both menus. The MVP ships the registry route, which needs no signing and works on Windows 10 and in Windows 11's full menu. The Windows 11 modern menu follows in 1.1, once code signing is in place.

| Where it appears | How it's registered | Needs signing | When |
| --- | --- | --- | --- |
| Windows 10 menu; Windows 11 "Show more options" and Shift + right-click | `HKCU\Software\Classes\*\shell\ScrimpCompress` and `ScrimpDecompress`, plus the same under `Directory\shell` and `Drive\shell`, each with an `ExplorerCommandHandler` CLSID | No | MVP |
| Windows 11 modern menu | Sparse MSIX package with `desktop4:FileExplorerContextMenus` pointing at the same CLSIDs | Yes | 1.1 |

On Windows 11 the MVP's Scrimp items sit under "Show more options", one click deeper than they will in 1.1. Shift + right-click opens that menu directly.

The registry route is also the development loop: build the DLL, run `cargo xtask dev-register`, restart Explorer, test. No package, no certificate.

## 5. Native UI

There's no main window. Every window Scrimp shows is a response to something the user started.

### Progress

MVP: **`TaskDialogIndirect`** with `TDF_SHOW_PROGRESS_BAR` and a timer callback that updates the text with current file, files done, space saved and speed. The bar is indeterminate until enumeration has counted the total.

After the MVP: try **`IOperationsProgressDialog`**, the dialog Explorer uses for copying files. It has pause, cancel, item counts, a speed graph and "More details", and it follows the system theme including dark mode.

### Results

A TaskDialog summary in the plain language of section 4: "Saved 35.2 GB. Games now takes up 51.2 GB instead of 86.4 GB." An expandable Details section lists skipped and failed files by reason. A "Save report" button writes a CSV or JSON report. When files failed with "access denied", a "Retry as administrator" button relaunches the worker elevated with only those files.

### Settings and analysis (after the MVP)

In the MVP, settings are edited in `settings.json` and Scrimp Savings… uses a task dialog. When dialogs are added, build them from `DIALOGEX` templates in a `.rc` file, compiled with `embed-resource`. Dialog templates give keyboard navigation, tab order, DPI scaling and screen reader support without hand-placing controls. Use `WC_LISTVIEW` for the analysis breakdown.

### Every executable gets a manifest

Common Controls v6 (visual styles), `PerMonitorV2` DPI awareness, `longPathAware`, and `asInvoker` execution level.

### Dark mode

Classic Win32 controls have no supported dark mode. The shell progress dialog and TaskDialog follow the system theme, which is another reason to lean on them. Custom dialogs will look like Control Panel dialogs, which is consistent with Windows itself.

## 6. Automatic compression

Automatic mode applies rules in order and stops at the first match.

1. **Never touch.** WIM-backed and CompactOS files, protected system paths, cloud placeholders (`RECALL_ON_DATA_ACCESS`, `OFFLINE`), EFS-encrypted files, reparse points, files below the minimum size.
2. **Change-prone files.** Skipped by default because WOF would undo itself: `.vhd`, `.vhdx`, `.vmdk`, `.qcow2`, `.pst`, `.ost`, `.db`, `.sqlite`, `.mdf`, `.ldf`, `.log`, `.etl`. A setting can switch these to LZNT1 instead.
3. **Already compressed.** Skipped: JPEG, PNG, WebP, AVIF, HEIC, MP3, AAC, Opus, MP4, MKV, MOV, WebM, ZIP, 7z, RAR, gz, zst, xz, `.cab`, `.msi`, `.docx`/`.xlsx`/`.pptx` (ZIP containers).
4. **Text, source code, CSV, JSON, XML, uncompressed images (BMP, TIFF):** LZX.
5. **Executables and libraries (`.exe`, `.dll`, `.sys`, `.pdb`):** XPRESS16K.
6. **Everything else:** XPRESS8K.

If Windows reports `ERROR_COMPRESSION_NOT_BENEFICIAL`, record `NotBeneficial` and move on. That's a normal result.

The rules live in code as a versioned table. Users can override extensions in `settings.json`. An explicit algorithm choice from the menu or CLI always wins over the policy, except for rule 1.

**Version 1.1 adds sampling.** Before compressing an unknown file, compress a few 64 KB samples in memory using the Windows Compression API (`compressapi.h`, part of Windows) and skip files that don't shrink. This keeps the "no external engines" principle and catches incompressible files with unhelpful extensions.

## 7. Command-line interface

`scrimp.exe` exposes every operation available in Explorer.

```powershell
scrimp compress "D:\Example.dat" --algorithm lzx
scrimp compress "R:\Documents" --recursive --algorithm xpress16k
scrimp compress "R:\Archives" --recursive --algorithm auto --dry-run
scrimp decompress "R:\Documents" --recursive
scrimp inspect "R:\Documents\Example.dat"
scrimp analyse "R:\Documents" --recursive --json
scrimp compress "R:\Documents" --recursive --algorithm lzx --workers 1 --verify
```

### JSON output

`--json` writes newline-delimited JSON: one event per line while running, then a summary object. Each object carries `schemaVersion`. This makes progress usable from scripts and from other applications such as Keevoro.

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Every file reached its target, or was legitimately skipped |
| 1 | Finished, but some files failed |
| 2 | Invalid arguments |
| 3 | Nothing could be done (for example, unsupported filesystem) |
| 4 | Cancelled by the user |
| 5 | Fatal error before work began |

## 8. Safety and reliability

The original risk table stands. These additions come from section 2 and from the single-worker design.

| Risk | Mitigation |
| --- | --- |
| Changed timestamps upset backup and sync tools | Confirm in Phase 0 that compression preserves `LastWriteTime`; test it in CI |
| Read-only files | Clear the attribute, compress, restore it, using an RAII guard so it's restored on every exit path |
| Writes silently undoing WOF | Change-prone exclusions; explain in the results dialog |
| Decompression filling a drive | Preflight check plus free-space reserve, re-checked between files |
| Dev Drive and other ReFS volumes | Capability check with a clear message |
| 64 KB cluster volumes | Disable LZNT1 options; WOF still offered if Phase 0 confirms it works |
| Two jobs racing on one disk | One worker per user, one queue per physical disk |
| Elevated worker trusting a non-elevated file | Show paths before running elevated |
| Panic inside Explorer | `catch_unwind` at every boundary, tested by deliberately injecting a panic |

### Protected paths by default

`%WINDIR%`, `%ProgramFiles%\WindowsApps`, `%ProgramData%\Microsoft`, `System Volume Information`, `$Recycle.Bin`, and the page, swap and hibernation files.

### Integrity

The test suite hashes every fixture with BLAKE3 before and after every operation and algorithm change. Production runs don't re-read files by default; `--verify` and the matching setting read each file back after compressing it.

### VHDX

As in the original plan: report space saved inside the mounted filesystem, and state clearly that the VHDX file itself doesn't shrink until it is compacted. Compaction stays out of 1.0.

## 9. Repository layout and conventions

The layout follows Keevoro's, so moving between the two projects feels the same.

```text
scrimp/
├── Cargo.toml                 workspace: shared version, dependencies, lints, profiles
├── Cargo.lock
├── rust-toolchain.toml        stable, with rustfmt and clippy (as Keevoro)
├── rustfmt.toml               max_width = 120 (as Keevoro)
├── .cargo/config.toml         the `cargo xtask` alias (as Keevoro)
├── AI_AGENT_PROMPT.md         rules for AI coding agents (Keevoro's, adapted)
├── PLANNING.md                the engineering summary of this plan
├── TASKS.md                   the order of work, ticked off commit by commit
├── CHANGELOG.md               user-facing, Keep a Changelog; decides version numbers
├── LICENSE                    MIT
├── TRADEMARKS.md              the Scrimp name and icon (Keevoro's, adapted)
├── README.md
├── install.ps1                one-line install of the CLI from the latest release
├── crates/
│   ├── scrimp-win/            wof.rs, ntfs.rs, handle.rs, volume.rs, storage.rs,
│   │                          enumerate.rs, user_path.rs, error.rs
│   ├── scrimp-core/           algorithm.rs, state.rs, policy.rs, plan.rs,
│   │                          scheduler.rs, outcome.rs, events.rs, report.rs, fs.rs
│   ├── scrimp-shell/          lib.rs (exports), factory.rs, command.rs, menu.rs,
│   │                          selection.rs, launch.rs
│   ├── scrimp-update/         lib.rs (from keevoro-update)
│   ├── scrimpw/               main.rs, instance.rs, job.rs, settings.rs, register.rs, ui/
│   ├── scrimp-cli/            main.rs
│   └── xtask/                 release, changelog, licences (from Keevoro),
│                              dev-register, fixtures, installer
├── installer/
│   └── scrimp.nsi             NSIS script
├── scripts/
│   └── build-installer.sh     same role as Keevoro's: one build for CI and release
├── res/                       icons, scrimp.rc, app.manifest
├── tests/
│   ├── integration/
│   └── shell-host/            test program that loads the DLL like Explorer does
├── docs/
│   ├── developing.md          as Keevoro: building, testing, cutting a release
│   ├── windows-api.md         Phase 0 findings live here
│   └── command-line.md
└── .github/workflows/         ci.yml, release.yml, rollback.yml
```

Test fixtures are generated from a fixed seed by `cargo xtask fixtures` rather than committed as binaries.

### Rules carried over from Keevoro's AI_AGENT_PROMPT.md

- **Never lose or alter a file's contents.** For Scrimp this is rule one, as "never write to a source" is for Keevoro.
- **No `unwrap` or `expect` outside tests.** One file's problem never stops a job; it goes in the report.
- **TDD, and coverage gates:** 90% overall, 95% for compress, decompress, verify and state detection.
- **Say what changed, for the user.** Every noticeable change goes under Unreleased in CHANGELOG.md in the same commit.
- **Nothing a person set may be lost by an update.** Settings changes come with a conversion and a test.
- **Examples are always made up.** Public repository: invented paths and figures only, one consistent set.
- **Plain words, British spelling, no em dashes** in anything a user reads. The "very Apple" bar applies.
- **Do not add a crate before a task needs it.**

Two Keevoro rules don't carry over: Scrimp is Windows only, so "cross-platform from the start" applies only to `scrimp-core`, and `unsafe` is allowed in the two crates in section 3.

## 10. Dependencies

Keep the list short and justify each addition in the pull request that adds it.

| Crate | Used by | Purpose |
| --- | --- | --- |
| `windows`, `windows-core` | win, shell, worker | Windows API bindings and COM |
| `thiserror` | libraries | Typed errors |
| `serde`, `serde_json` | core, worker, CLI | Settings, jobs, JSON output |
| `clap` (derive) | CLI | Argument parsing |
| `tracing`, `tracing-subscriber`, `tracing-appender` | worker, CLI only | Logging to `%LOCALAPPDATA%\Scrimp\logs` |
| `embed-resource` | build scripts | Manifests, icons, dialog templates, version info |
| `ureq` (with `platform-verifier`), `sha2` | `scrimp-update` only | Reading GitHub Releases and checking downloads; the same versions Keevoro uses |
| `anyhow` | `xtask` only | Errors in developer tooling |
| `blake3`, `proptest`, `tempfile` | dev-dependencies | Integrity and property tests |

Versions are declared once in `[workspace.dependencies]`, as Keevoro does, with a comment on each saying why it's there.

Deliberately not used: `tokio` or any async runtime, `crossbeam` (std channels are enough), `walkdir` (we need file IDs and reparse tags from our own enumerator), `anyhow` in libraries, and any GUI framework.

`cargo-deny` enforces licences and security advisories in CI.

## 11. Roadmap

### MVP scope

The MVP cuts anything that is easy to add later. Where adding something later would mean reworking the core, the MVP builds the underlying support now and leaves out only the visible feature.

| Cut from the MVP | What the MVP does instead | Built now so it's easy to add later |
| --- | --- | --- |
| Custom progress window | Task dialog with a progress bar, current file, files done, space saved and Cancel | The UI only consumes progress events, so a richer window plugs into the same stream |
| Custom results window | Task dialog summary, expandable Details by reason, **Save report** (CSV/JSON) | Every file gets a typed `Outcome`; reports are built from those, not from UI text |
| Settings dialog | `settings.json`, opened in Notepad from the results dialog | Versioned settings schema with validation, so a dialog only reads and writes it |
| Compression properties dialog | **Scrimp Savings…** shows the facts in a task dialog | The same `inspect` call the CLI uses |
| Pause button | Cancel only, taking effect between files | The scheduler supports pause and resume between files from the start; only the button is missing |
| Physical-disk detection (VHDX hosts, Storage Spaces) | One queue per volume: HDD 1, SSD 4, unknown 1, with `--workers` to override | Queues are keyed by a `DeviceKey` type, so switching from volume to physical disk changes one function |
| Benchmarks against CompactGUI, Compactor, CompactRS | Benchmark against `compact.exe` only | Benchmark harness takes any command line |
| Files over 100 GB in automated tests | One manual run before release; CI covers files over 4 GB | Fixture generator accepts any size |

| Windows 11 modern menu and code signing | Windows 10 menu, and Windows 11's "Show more options" and Shift + right-click menus, through `HKCU` registration | The same COM handler; 1.1 adds only the sparse package and the signature |

Kept in the MVP:

- **The NSIS installer** (section 15): per user, no admin prompt, upgrades while Explorer holds the DLL, uninstall that removes everything Scrimp added. Getting install and upgrade right from the first release avoids migrating early users later.
- **`scrimp update`** (section 15), because without it early users stay on old builds.
- Everything in section 17.

### Phases

Each phase has an exit test. We don't start the next phase until it passes. Sizes are relative (S, M, L) because they depend on how much time goes in.

### Phase 0: Spikes (S)

Small throwaway programs, each answering one question. Findings go in `docs/windows-api.md`.

| # | Question |
| --- | --- |
| S1 | `WofSetFileDataLocation` or `FSCTL_SET_EXTERNAL_BACKING`? Minimum access rights? Are timestamps preserved? How do read-only files behave? |
| S2 | Which API gives the true on-disk size of a WOF file? |
| S3 | When does WOF return `ERROR_COMPRESSION_NOT_BENEFICIAL`, and what state is the file left in? |
| S4 | Can overlapped I/O plus `CancelIoEx` safely cancel compression of a large file? |
| S5 | Behaviour on ReFS/Dev Drive, exFAT, 64 KB clusters, SMB shares, mounted VHDX, BitLocker and EFS |
| S6 | `IExplorerCommand` in Rust via `HKCU` registration on Windows 10 and in the Windows 11 full menu; Shift detection; `GetState` with `fOkToBeSlow` |
| S7 | NSIS silent upgrade while Explorer holds the old DLL: does pointing the CLSID at a new versioned folder take effect without restarting Explorer? |

Moved after the MVP: `IOperationsProgressDialog` from our own process, sparse package registration and the Windows 11 modern menu, and physical-disk identity for VHDX and Storage Spaces.

**Exit:** every question has a written answer and a go or no-go on the design choice it affects.

### Phase 1: Repository, CI and native layer (M)

Set up the repository from Keevoro's: workspace files, conventions, `xtask` release and changelog commands, and the CI, release and rollback workflows (section 13). Then build `scrimp-win`: compress, decompress and inspect for all five algorithms, and volume capabilities. Build the VHDX fixture generator and the hash-and-roundtrip harness.

**Exit:** every algorithm, and every change between algorithms, round-trips byte-for-byte on fixture volumes in Windows CI. Every Windows error maps to a typed error. A rehearsal run of the Release workflow passes.

### Phase 2: Compression engine (M)

Build `scrimp-core`: state model, policy, streaming enumeration, scheduler, outcomes, events and reports.

**Exit:** unit tests pass on Linux and Windows against the fake filesystem. Integration tests cover hardlinks, junction loops, locked files, a full volume and a VHDX detached mid-job.

### Phase 3: CLI, released as 0.1 (S)

**Exit:** every command works with text and JSON output and documented exit codes. Benchmarked against `compact.exe` on the same data. Published with the Release button; `install.ps1` installs it in one line, as Keevoro's does.

### Phase 4: Worker and shell extension, released as 0.2 (L)

Build `scrimpw` (single instance, job intake, minimal progress) and `scrimp-shell` with `HKCU` registration.

**Exit:** right-click compression works on Windows 10 and in the Windows 11 full menu. Menu builds in under 50 ms in the shell test host. An injected panic leaves Explorer running.

### Phase 5: Native UI, installer and updates, released as 0.3 (M)

Task dialog progress and results, Save report, Try Again as Administrator, Scrimp Savings…, first-use message, the NSIS installer and `scrimp update`.

**Exit:** usable with keyboard only and with Narrator; correct at 100%, 150% and 200% scaling; cancel behaves as documented; install, upgrade (including through `scrimp update`) and uninstall pass on Windows 10 and in Windows Sandbox, and uninstall leaves the user's PATH exactly as it was before install.

### Phase 6: Hardening, released as 1.0, the MVP (M)

Benchmark worker limits per storage class, fix what testing on real Windows 10 and Windows 11 machines finds, finish the documentation.

**Exit:** everything in section 17; throughput at or above `compact.exe` on SSD and NVMe.

### After the MVP: 1.1, signed with the Windows 11 modern menu (M)

Code signing in the release workflow, the sparse package, its registration from `--register`, and the modern menu. Then a winget manifest and the sampling step for automatic mode.

**Exit:** the menu appears in the modern Windows 11 menu on a clean machine without developer mode, survives an upgrade, and disappears completely on uninstall.

## 12. Testing

| Layer | Runs on | What it covers |
| --- | --- | --- |
| Unit tests | Linux and Windows CI | Policy, scheduler, outcome aggregation, settings parsing, job validation |
| Property tests | Windows CI | Random sequences of algorithm changes on random content always leave files byte-identical |
| Integration tests | Windows CI | Real compression on VHDX fixture volumes: NTFS 4 KB, NTFS 64 KB, exFAT, and ReFS where available |
| Shell host | Windows CI | Loads the DLL through `CoCreateInstance`, builds a shell item array, times menu construction, injects panics |
| Manual matrix | Real PCs, VMs and Windows Sandbox | Windows 10 22H2 first, then Windows 11 24H2 and 25H2: clean installs, upgrades, uninstalls, the user's PATH before and after |

GitHub's Windows runners run as administrator, so tests can create and mount VHDX volumes with `diskpart`. They run Windows Server, so they can't test the Explorer menu as people see it. That part of the matrix stays manual, recorded in TASKS.md under "Tried by a person" as Keevoro does.

**Windows Sandbox** is the "clean machine without developer mode" from the original plan: a fresh Windows install every launch. Use it for every installer test.

Additions to the original test matrix:

- Write to a WOF file after compressing and confirm it decompresses and the data is correct.
- Timestamps and attributes are unchanged after every operation.
- Dev Drive (ReFS) shows the unsupported message.
- LZNT1 is disabled on 64 KB cluster volumes.
- BitLocker-encrypted volumes.
- Paths over 260 characters.
- Two jobs started on the same disk share one queue.

## 13. CI and releases

Copied from Keevoro and trimmed to Windows. A release is one button press, as it is for Keevoro.

### What carries over unchanged

- **The Release button.** Actions → Release → Run workflow. Leave the version blank and the changelog decides: a minor release if anything is listed under Added or Removed, otherwise a patch. "Rehearse" shows what would happen without pushing anything.
- **`cargo xtask release`** dates the changelog, sets the version, commits and tags. The button and the terminal run the same command, so they can't drift apart.
- **CI must be green** on the commit before it can be released.
- **Draft, then publish.** The release is created as a draft, every build is uploaded to it, `SHA256SUMS` goes up last, and only then is it published. "Latest" never points at a half-uploaded release.
- **The built program must say the right version** before it is uploaded.
- **The changelog section is the release notes.**
- **The Roll back workflow** withdraws bad releases and releases the last good code as a new, higher patch version, so installed copies move forward to good code.
- **`+crt-static`**, so Scrimp runs on a PC without the Visual C++ runtime installed.
- **`cargo xtask licences --check`** keeps the list of open-source licences current.

### What changes for Scrimp

- **Windows only.** No macOS or Linux jobs and no "all platforms" switch. `scrimp-core`'s unit tests also run on `ubuntu-latest`, because they're quick and keep the core free of Windows assumptions.
- **Coverage runs on `windows-latest`**, because most of Scrimp only runs there. Gates: 90% overall, 95% for compress, decompress, verify and state detection. The thin Win32 UI code in `scrimpw` is left out, as Keevoro leaves out its desktop shell.
- **Integration tests** create VHDX fixture volumes on the Windows runner.
- **The installer job** runs `scripts/build-installer.sh`, which calls `makensis`, instead of `tauri build`. As in Keevoro, it runs on every push to main, not on pull requests, so there's always a current installer to download from the run's page.
- **Release assets:** `scrimp-windows-x64.exe` (the CLI on its own, for `install.ps1`), `Scrimp-windows-x64-setup.exe` and `SHA256SUMS`.
- **`cargo deny check`** for licences and security advisories, and a CycloneDX SBOM attached to each release. Neither is in Keevoro yet; both are cheap to add.
- **Signing** is added to the build job in 1.1 (below).

### Code signing (1.1)

The MVP ships unsigned, as Keevoro does today. Windows SmartScreen will warn when the installer is run until it's signed and has built up a reputation.

The Windows 11 modern menu needs a signed sparse package: the certificate's subject must match the manifest's `Publisher`, and Windows must trust it. Apply now, because approval takes time. Options:

- **SignPath Foundation:** free signing for open-source projects, integrates with GitHub Actions.
- **Azure Artifact Signing:** low monthly cost; check eligibility for your country and entity type.
- **A purchased certificate:** most expensive, most hassle.

For development, `cargo xtask dev-cert` creates a self-signed certificate and installs it in `TrustedPeople` on the developer's own machine.

## 14. Settings

`%LOCALAPPDATA%\Scrimp\settings.json`, read and checked by `scrimp-core::settings`. Every setting is optional; a missing file means the defaults.

```json
{
  "schemaVersion": 1,
  "defaultMethod": "auto",
  "minimumFileSizeBytes": 8192,
  "filesThatChangeOften": "skip",
  "workers": { "hdd": 1, "ssd": 4, "removable": 1, "unknown": 1 },
  "extensions": { "bak": "lzx", "dat": "skip" }
}
```

Settings a newer Scrimp added are ignored, so going back to an older version never stops Scrimp starting. A file with a newer `schemaVersion` is refused. A mistake is reported in plain words naming the setting. Writing the file (atomically, keeping settings this version doesn't know) comes with the settings dialog after the MVP.

There is no `checkForUpdates` setting. Scrimp never checks for updates in the background; `scrimp update` checks when the user runs it.

## 15. Installation and distribution

### Two ways to install

| Way | Gets you | Who it's for |
| --- | --- | --- |
| `irm https://raw.githubusercontent.com/<owner>/scrimp/main/install.ps1 \| iex` | The CLI only, on the user's PATH | 0.1 users and scripters. Same as Keevoro's one-liner. |
| `Scrimp-windows-x64-setup.exe` | The CLI, the worker, the Explorer menu, Start menu entries, an uninstaller | Everyone, from 0.3 |

Both install to the same folder, so someone who started with the one-liner can run the installer later and nothing is duplicated.

### The NSIS installer

Per user, no administrator prompt, the same choice Keevoro makes through Tauri (`installMode: currentUser`). Scrimp writes its own short `installer/scrimp.nsi` because there's no Tauri to generate one. `makensis` runs on GitHub's Windows runners.

```text
%LOCALAPPDATA%\Programs\Scrimp\
├── scrimp.exe                 CLI, on the user's PATH
├── scrimpw.exe                worker
├── uninstall.exe
└── shell\
    └── 0.3.0\
        └── scrimp_shell.dll   one folder per version, because Explorer keeps it loaded
```

The installer copies files, then runs `scrimpw.exe --register`. The uninstaller runs `scrimpw.exe --unregister`, then removes the folder. Registration lives in Rust, not in the NSIS script, so it is covered by tests and the developer loop (`cargo xtask dev-register`) runs the same code.

`--register` writes the `HKCU` menu entries pointing at the newest `shell\<version>` folder, adds the PATH entry, and creates the Start menu entries. `--unregister` reverses each step and leaves nothing behind. Uninstalling doesn't decompress anyone's files, and the uninstaller says so.

### Adding Scrimp to PATH properly

Only the user's PATH (`HKCU\Environment\Path`) is changed, never the system PATH. Both `--register` and `install.ps1` follow these rules:

1. **Add exactly one entry:** `%LOCALAPPDATA%\Programs\Scrimp`. It never changes between versions, because `scrimp.exe` sits at the top of the install folder rather than in a versioned subfolder.
2. **Don't add it twice.** Compare case-insensitively, ignoring a trailing backslash and expanding variables for the comparison only.
3. **Leave every other entry exactly as it was**, including its order and any unexpanded variables such as `%USERPROFILE%\bin`.
4. **Keep the value's registry type.** PATH is usually `REG_EXPAND_SZ`. Read it without expanding (`RegGetValueW` with `RRF_NOEXPAND`, or `DoNotExpandEnvironmentNames` in PowerShell) and write it back with the same type. PowerShell's `[Environment]::SetEnvironmentVariable` doesn't preserve this, so `install.ps1` uses the registry directly.
5. **Never pass PATH through NSIS string handling.** Standard NSIS builds cut strings at 1,024 characters, which is a well-known way installers wipe long PATHs. That's another reason the work happens in `scrimpw.exe`.
6. **Tell running programs.** Broadcast `WM_SETTINGCHANGE` with `"Environment"`, so new terminals see the change without signing out.
7. **On uninstall, remove only Scrimp's entry**, matched by the same rules. If the user's PATH had no other entries, the empty value is removed rather than left behind.

The add and remove logic is a pure function over the PATH string in `scrimp-win::user_path`, with unit tests for each rule: long PATHs, duplicates, trailing semicolons, unexpanded variables and an empty PATH.

### Updating: `scrimp update`

Scrimp can't replace itself the way `keevoro update` does, because Explorer keeps the menu DLL loaded. So `scrimp update` hands over to the installer:

1. Reads the latest release from the GitHub API and compares versions (`scrimp-update`, adapted from `keevoro-update`).
2. If there's a newer version, shows its changelog section and downloads `Scrimp-windows-x64-setup.exe` and `SHA256SUMS`.
3. Refuses to continue if the checksum doesn't match. From 1.1 it also checks the installer's signature.
4. Starts the installer in silent update mode, passing its own process ID, and exits.
5. The installer waits for that process to exit. If the worker is in the middle of a job, it asks the worker to finish its current file and stop, then waits.
6. It replaces `scrimp.exe` and `scrimpw.exe`, puts the new DLL in a new `shell\<version>` folder, and runs `scrimpw.exe --register`, which points the menu at the new DLL.
7. Explorer keeps using the old DLL until it unloads it. New menus load the new one. No reboot, and no Explorer restart.
8. The next time the worker starts, it deletes old `shell\<version>` folders that are no longer in use.

Someone who installed only the CLI with the one-liner gets the same behaviour from `scrimp update`, except that step 6 replaces `scrimp.exe` directly, as Keevoro does.

### Also published

The CLI on its own (`scrimp-windows-x64.exe`), as Keevoro publishes `keevoro-windows-x64.exe`. A winget manifest follows in 1.1, once releases are signed.

## 16. Decisions

| # | Decision | Status |
| --- | --- | --- |
| 1 | Menu label and wording | **Decided:** top-level "Scrimp Compress" and "Scrimp Decompress" with the Scrimp icon; algorithm choice behind Shift (section 4) |
| 2 | Static registry menu phase | **Decided:** skipped. Build the `IExplorerCommand` handler once and register it under `HKCU` |
| 3 | Licence | **Decided:** MIT |
| 4 | Windows 10 | **Decided:** first MVP target, together with the Windows 11 full menu |
| 5 | Installer | **Decided:** NSIS, per user, as Keevoro |
| 6 | Build and release | **Decided:** Keevoro's workflows and `xtask` commands, adapted (section 13) |
| 7 | PATH | **Decided:** one user PATH entry, added and removed by the rules in section 15 |
| 8 | Updates | **Decided:** `scrimp update`, run by the user, hands over to the installer |
| 9 | Signing | **Decided:** 1.1. Apply to SignPath Foundation now; approval takes time |
| 10 | Change-prone files in automatic mode | Recommended: skip |
| 11 | Name availability | Open: check the GitHub repository name, crates.io names, winget ID and trademarks for "Scrimp" before announcing |

## 17. MVP definition of done

- [x] Native Rust, no managed runtime, no runtime DLLs beyond Windows: `cargo xtask imports` in CI and in releases
- [x] XPRESS4K, XPRESS8K, XPRESS16K, LZX and LZNT1 compression and decompression: `crates/scrimp-win/tests/round_trip.rs`
- [x] Byte-for-byte integrity across every algorithm change in CI: `every_change_between_methods_keeps_the_content_and_lands_exactly`
- [x] Clear handling of unsupported volumes, including Dev Drives: `crates/scrimp-win/tests/volumes.rs` on CI's exFAT, ReFS, 64 KB-cluster and no-WOF drives; Dev Drives called by name
- [ ] Explorer menu on Windows 10 and in Windows 11's "Show more options" and Shift + right-click menus: built and tested through the shell's own interfaces (`crates/scrimp-shell/src/tests.rs`); waiting for a person to try it in Explorer (TASKS.md, Tried by a person, S6)
- [ ] Files, multiple selections, folders and drives: tested through the shell's interfaces and the engine; waiting for a person to try them in Explorer (S6)
- [x] Current state and real savings reported: `scrimp inspect`, `scrimp analyse` and Scrimp Savings…, from one analysis in `scrimp-core`
- [x] Native progress with cancel (pause supported by the scheduler, button after the MVP): task dialogs, tested for real in `crates/scrimp-win/src/ui.rs`
- [x] One worker per user with per-disk worker limits: `crates/scrimpw/tests/worker.rs` and the engine's scheduler tests
- [x] Automatic mode with change-prone and already-compressed skipping: `crates/scrimp-core/src/policy.rs`
- [x] CLI with JSON output and documented exit codes: [docs/command-line.md](command-line.md), `crates/scrimp-cli/tests/commands.rs`
- [x] Mounted VHDX support with honest reporting: the summary says the VHDX file only shrinks once compacted, detected on CI's real mounted VHDX drives
- [x] Explorer survives an injected panic in the shell extension: `a_panic_comes_back_as_an_error_and_never_reaches_explorer`
- [x] NSIS installer: per-user install, upgrade and clean uninstall, no administrator prompt: `scripts/test-installer.ps1` in CI
- [x] `install.ps1` one-line CLI install and `scrimp update`, both verifying `SHA256SUMS`: `crates/scrimp-update/src/tests.rs`
- [x] One user PATH entry, added and removed correctly; uninstall leaves PATH as it was: `crates/scrimp-win/src/user_path.rs` tests and the installer test
- [x] No background processes when idle; no network access except when the user runs `scrimp update`: the worker exits when its jobs are done (`every_waiting_job_is_run_then_the_worker_exits`); only `scrimp-update` has a network client
- [x] Keevoro-style Release button, rollback workflow, checksums and SBOM: `.github/workflows/release.yml` and `rollback.yml`
- [x] Coverage at or above 90% overall and 95% for compress, decompress, verify and state detection: CI's coverage job, with `cargo xtask coverage` for the 95%
- [x] Developer documentation: `docs/developing.md`, Windows API findings, command line, AI agent rules

## 18. After 1.0

First, 1.1: code signing, the Windows 11 modern menu, a winget manifest and sampling in automatic mode (section 11).

Then, unchanged from the original plan, in rough priority order:

1. Folder watching, to compress new files in chosen folders (fills the WOF inheritance gap)
2. Explorer property page showing compression details
3. Compression analysis across a drive
4. Scheduled compression during idle time
5. Adaptive algorithms based on measured results
6. VHDX compaction
7. Duplicate detection and hardlink management
8. Keevoro integration through the JSON CLI or a library crate

## 19. References

- [windows-rs](https://github.com/microsoft/windows-rs)
- [WofSetFileDataLocation](https://learn.microsoft.com/en-us/windows/win32/api/wofapi/nf-wofapi-wofsetfiledatalocation)
- [FSCTL_DELETE_EXTERNAL_BACKING](https://learn.microsoft.com/en-us/windows-hardware/drivers/ifs/fsctl-delete-external-backing)
- [Integrate a packaged app with File Explorer](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/integrate-packaged-app-with-file-explorer)
- [Grant package identity by packaging with external location](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/grant-identity-to-nonpackaged-apps)
- [Creating static cascading menus](https://learn.microsoft.com/en-us/windows/win32/shell/creating-static-cascading-menus)
- [Windows Compression API](https://learn.microsoft.com/en-us/windows/win32/cmpapi/using-the-compression-api)
- [Compactor](https://github.com/Freaky/Compactor), [CompactRS](https://github.com/IRedDragonICY/compactrs), [CompactGUI](https://github.com/IridiumIO/CompactGUI), [woftool](https://github.com/wbenny/woftool)
