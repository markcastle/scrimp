# Scrimp: Instructions for AI Coding Agents

Read this before changing anything. Then read [PLANNING.md](PLANNING.md) for the why and [TASKS.md](TASKS.md) for what to do next. The full plan is [docs/plan.md](docs/plan.md).

## What you are building

Scrimp exposes the file compression built into Windows (WOF and NTFS compression) through Explorer's right-click menu and a command line. It modifies people's real files in place. A bug that loses or alters a file's contents is the worst possible outcome, far worse than a crash or a missed feature.

## Non-negotiable rules

1. **Never lose or alter a file's contents.** Only Windows' own compression APIs touch file data. Every operation is covered by tests that hash files before and after. A failed operation is reported with the file's actual state, never as a success.
2. **Never crash Explorer.** The menu DLL does no compression, no recursive enumeration and no network access. Every exported function and COM method catches panics. Never set `panic = "abort"` in any Cargo profile.
3. **Never treat "unknown" as "uncompressed".** If Scrimp can't tell a file's state, it says so and leaves the file alone.
4. **Leave alone what isn't ours.** WIM-backed and CompactOS system files, protected system folders, cloud placeholders and unsupported volumes are skipped and reported, never changed.
5. **`unsafe` only where Windows requires it.** It is denied for the workspace. Only `scrimp-win` and `scrimp-shell` may opt out, and every `unsafe` block carries a `// SAFETY:` comment saying why it is sound.
6. **No `unwrap` or `expect` outside tests.** Return errors. A problem with one file never stops a job; it goes in the report.
7. **Nothing leaves the machine.** No telemetry and no background network access. The only network call is `scrimp update`, and only when the person runs it.
8. **Examples are always made up.** This repository is public. Never put real paths, drive letters, user names, file names, counts or sizes from anyone's computer into code, comments, tests, fixtures, docs, commit messages or the changelog. Use the same made-up set everywhere:
   - Home folder: `C:\Users\You`.
   - Folders: `D:\Games`, `D:\Archive`, `C:\Users\You\Documents`.
   - Files: `Example.dat`, `Report-2019.docx`, `Holiday.mp4`.
   - Figures: 6,000 files taking 86.4 GB, compressed to 51.2 GB, saving 35.2 GB, with 862 files already as small as they can be. Where numbers are added or taken away, make them add up.
   - People: none. Email addresses only at `example.com`.

## How to work

- **TDD.** Write the failing test first. Coverage must stay at or above 90% overall and 95% for compress, decompress, verify and state detection.
- **One task at a time** from TASKS.md, top to bottom within the current phase. Tick it in the same commit.
- **Say what changed, for the user.** Anything a person using Scrimp would notice goes under **Unreleased** in [CHANGELOG.md](CHANGELOG.md) in the same commit, under Added, Changed, Fixed or Removed. The heading decides the next version number (Added or Removed makes a minor release), and the words become the release notes. See [docs/developing.md](docs/developing.md#cutting-a-release).
- **Nothing a person set may be lost by an update.** Every change to `settings.json` comes with a conversion and a test that an older file still opens with everything in place.
- **Keep the core pure.** Policy, planning, scheduling and outcomes belong in `scrimp-core` with no Windows calls, so they can be tested anywhere. Windows calls belong in `scrimp-win`.
- **Do not add a crate before a task needs it.** When you add one, add it to the dependency table in [docs/plan.md](docs/plan.md) with why.

## Commands

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three must succeed before you finish. Windows-only code is only fully checked on Windows; CI runs everything on `windows-latest`.

## Style

- Plain names over clever ones. Small functions. Comments explain why, not what.
- Every module starts with a short doc comment saying what it is for, in human terms.
- User-facing text is plain English for non-technical people: "3 files are in use by another app", never "ERROR_SHARING_VIOLATION". No em dashes.
- No algorithm names where everyday users look. XPRESS, LZX and LZNT1 appear only in the command line, in settings and behind a "Details" disclosure. In the menu they are "Faster to Open", "Smallest" and "For Files That Change".
- Use British spelling in prose and user-facing text (analyse, organise), and Rust conventions in code.

## The user experience bar

Calm, plain and "very Apple". When a choice affects what the user sees or reads, pick the option a non-technical person would understand without help. Hide anything technical behind a "Details" or Shift + right-click door.

## Common mistakes to avoid

- Forgetting that writing to a WOF-compressed file decompresses all of it. Files that change often are skipped by automatic mode for this reason.
- Assuming WOF compression applies to new files in a folder. It doesn't; only NTFS (LZNT1) folder compression is inherited.
- Following symbolic links or junctions. They cause loops and double counting.
- Counting a hardlinked file twice. Its names share one copy of the data.
- Running several workers against one spinning disk. It slows to a crawl.
- Trusting "size on disk" figures without checking them against a real compressed file.
