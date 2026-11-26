# Developing Scrimp

This is the guide for working on Scrimp: how to build and test it, and how a change becomes a release. The rules every change follows are in [AI_AGENT_PROMPT.md](../AI_AGENT_PROMPT.md); they apply to people as much as to agents.

## Building and testing

You need Rust stable (the version is pinned by `rust-toolchain.toml`, so `rustup` installs it for you) and, for anything beyond `scrimp-core`, a Windows PC.

```sh
cargo build --workspace
```

```sh
cargo test --workspace
```

```sh
cargo clippy --workspace --all-targets -- -D warnings
```

```sh
cargo fmt --all
```

All four must pass before a change is finished. CI runs them on every pull request.

### What runs where

Most of Scrimp only works on Windows, because it calls Windows' compression APIs. `scrimp-core` is deliberately pure Rust with no Windows calls, so its tests run anywhere, including on a Mac or Linux machine. CI runs those tests on Linux as well as Windows, which keeps Windows assumptions from creeping into the core.

Everything else is built and tested on GitHub's `windows-latest` runners. They run Windows Server as an administrator, so tests can create small VHDX volumes and compress real files on them. They can't show the Explorer menu the way people see it, so anything to do with the menu is also tried by hand on Windows 10 and Windows 11 and recorded under "Tried by a person" in [TASKS.md](../TASKS.md).

### Testing against other kinds of drive

`scrimp-win` behaves differently on NTFS with large clusters, exFAT, ReFS (as a Dev Drive is) and a drive Windows' compression driver isn't attached to, and its tests in `crates/scrimp-win/tests/volumes.rs` check each. They need small test drives, which `scripts/test-volumes.ps1` makes as VHDX files mounted as N:, L:, X:, R: and W:. CI makes them on every run.

To run those tests on your own PC, open PowerShell as an administrator (mounting a VHDX needs it) and run:

```powershell
.\scripts\test-volumes.ps1
```

It prints the `SCRIMP_TEST_*` variables to set in the window you run `cargo test` from. Without them, those tests skip. When you're done:

```powershell
.\scripts\test-volumes.ps1 -Remove
```

### Trying the Explorer menu

The menu can't be tested on GitHub's runners, because they have no desktop. To try it on your own PC, build everything, then add the menu for your user:

```powershell
cargo build --workspace
```

```powershell
.\target\debug\scrimpw.exe --register
```

Right-click a file or folder. On Windows 10 you'll see **Scrimp Compress** and **Scrimp Decompress**; on Windows 11 they're under **Show more options** (or Shift + right-click). Hold Shift to see the choices under Scrimp Compress, and **Scrimp Savings…**.

Explorer keeps the DLL loaded once it has used it, so to try a new build, take the menu out, restart Explorer, rebuild, and add it again:

```powershell
.\target\debug\scrimpw.exe --unregister
```

```powershell
Stop-Process -Name explorer
```

Explorer starts again by itself. A crash in Scrimp's menu code is caught and turned into an error, so it can't take Explorer down; if Explorer ever does go down with Scrimp's menu registered, that's a bug to report with the steps that caused it.

### Coverage

CI measures test coverage on Windows and fails under 90% of lines. Code that compresses, decompresses, verifies or detects a file's state is held to 95% in review. To see coverage yourself on Windows:

```sh
cargo install cargo-llvm-cov
```

```sh
cargo llvm-cov --workspace --summary-only
```

## Cutting a release

The work of a release is done before it, in [CHANGELOG.md](../CHANGELOG.md). Every change a person would notice goes under **Unreleased** as it lands, in one of **Added**, **Changed**, **Fixed** or **Removed**, written for the person using Scrimp: what they will see, not which function moved. Those notes become the GitHub Release, and a release refuses to run while the section is empty.

The headings also decide the version number:

| Unreleased lists | The release is | From 0.2.1 |
| --- | --- | --- |
| Something **Added** or **Removed** | A minor release: something a person would notice as new, or gone | 0.3.0 |
| Only **Changed** and **Fixed** | A patch | 0.2.2 |

That is the whole rule. It lives in `next_version` in `crates/xtask/src/changelog.rs` and is covered by `cargo test`. You can always overrule it by giving a number yourself, which is how 1.0.0 will happen. Whatever the number, it must be higher than the last, because installed copies only ever move to a higher version.

### The button

On GitHub, open **Actions**, pick **Release** on the left, press **Run workflow**, and press the green button. Leave the version blank and the changelog decides it; type one to choose yourself. The workflow then:

1. Works out the version if none was given, and refuses if it is not higher than the last release, if CI has not passed on the latest commit to main, or if the changelog has nothing under Unreleased.
2. Dates the Unreleased section as the version and opens a fresh one, sets the version in `Cargo.toml`, updates `Cargo.lock`, commits and tags.
3. Creates the GitHub Release as a draft, with the changelog section as the notes.
4. Builds `scrimp.exe` for Windows x64, checks it reports the right version, and uploads it to the draft as `scrimp-windows-x64.exe`.
5. Adds a `SHA256SUMS` file and publishes the release, so "latest" never points at a release with nothing to download.

Step 4 also builds the installer, `Scrimp-windows-x64-setup.exe`, with `scripts/build-installer.sh`.

Tick **Rehearse** to see steps 1 and 2 in the log without anything being pushed, built or published. It is also the way to ask what number the next release would be.

### The terminal

```sh
cargo xtask release
```

```sh
git push --follow-tags
```

The first command fetches the tags, works out the number the same way and says which it chose; `cargo xtask release 0.3.0` chooses for it. It is exactly what the button runs, so the two routes cannot drift. The tag push starts the Release workflow at the build step. The terminal route does not check CI for you.

If the release commit reaches main but the tag does not, finish it from the button: run **Release** and type that version.

`cargo xtask` on its own lists everything it can do.

## Rolling back a release

If a release turns out to be broken, open **Actions**, pick **Roll back**, press **Run workflow**, and give the last good version and one sentence on why. The workflow then:

1. Marks every release newer than the good one as a pre-release, which withdraws it. "Latest" points at the good version again at once, so `install.ps1` hands out the good version and `scrimp update` no longer offers the bad one.
2. Puts the good version's code back on main as one commit, **Roll back to vX**, with a line in the changelog saying so and why. The release tooling (`.github`, `.cargo`, `crates/xtask`, `install.ps1` and `CHANGELOG.md`) keeps its current state.
3. Releases that code as the next patch number, higher than the withdrawn release, so copies that already took the bad version move to the good code the next time they update.

Work on main that landed after the good version is not lost: bring it back with `git revert` of the Roll back commit, fix what was wrong, and release again.

## How installing works

Each release carries:

| File | For |
| --- | --- |
| `Scrimp-windows-x64-setup.exe` | The installer: the menu, the worker and the command line, per user |
| `scrimp-windows-x64.exe` | The command line on its own, for `install.ps1` |
| `SHA256SUMS` | Checking any of the above arrived intact |

### The installer

`installer/scrimp.nsi`, built by `scripts/build-installer.sh` in CI and in releases. It installs per user to `%LOCALAPPDATA%\Programs\Scrimp`, with the menu DLL in `shell\<version>\`, because Explorer keeps an old one loaded while a new one installs. It then runs `scrimpw.exe --register --add-to-path`, so the menu and PATH rules are Rust code with tests, not NSIS script. Uninstalling runs `--unregister` and leaves the person's settings in `%LOCALAPPDATA%\Scrimp`.

CI's `installer` job runs `scripts/test-installer.ps1`: a silent install into a folder with spaces in its name, a check of every file, registry entry and the PATH, a second install over the top, and an uninstall that must leave PATH exactly as it was.

`install.ps1` asks GitHub for the latest release, downloads `scrimp-windows-x64.exe`, checks it against `SHA256SUMS`, and puts it in `%LOCALAPPDATA%\Programs\Scrimp` as `scrimp.exe`. Running it again updates. It needs no administrator.

### How Scrimp is added to PATH

`install.ps1` (and, from 0.3, the installer) adds one entry to the user's own PATH and never touches the system PATH. The rules, and why each matters:

1. **One entry, the install folder**, which never changes between versions.
2. **Never twice.** Entries are compared ignoring case and a trailing backslash, with variables expanded only for the comparison.
3. **Every other entry is left exactly as it was**, in the same order, including unexpanded ones such as `%USERPROFILE%\bin`.
4. **The registry value keeps its type.** PATH is usually `REG_EXPAND_SZ`. PowerShell's `[Environment]::SetEnvironmentVariable` reads it with variables expanded and writes it back as a plain string, which hard-codes those variables. `install.ps1` uses the registry directly instead.
5. **Running programs are told.** `WM_SETTINGCHANGE` is broadcast, so new terminal windows see the change without signing out.
6. **Uninstalling removes only Scrimp's entry**, matched by the same rules.

## Scrimp is not signed yet

The checksum proves a download arrived intact. It doesn't prove who made it, because the checksum file comes from the same place as the program. Until releases are signed (planned for 1.1), Windows SmartScreen may warn the first time Scrimp runs, and `install.ps1` clears the "downloaded from the internet" mark because it is run by someone who has chosen to trust this repository.
