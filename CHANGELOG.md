# Changelog

Every change to Scrimp that a person would notice, by version, newest first. Each release has what was **added**, what was **changed**, what was **fixed** and what was **removed**, written for the people using Scrimp rather than for the people building it. The Release workflow copies a version's section from here into its GitHub Release, which is what `scrimp update` shows as "What's new".

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) with one plain rule: a patch is for fixes and small adjustments, a minor is for anything you would notice as new. How a section gets here is in [docs/developing.md](docs/developing.md#cutting-a-release).

## [Unreleased]

### Added

- **Scrimp in Explorer's right-click menu.** Right-click a file, a folder or a drive and choose **Scrimp Compress** or **Scrimp Decompress**. A single file shows only the one that makes sense, and drives that can't save space this way (USB sticks, network locations, Dev Drives) show neither. Hold Shift to choose how to compress: Automatic, Faster to Open, Smallest, or For Files That Change. On Windows 11, Scrimp is under **Show more options**.
- **A progress window while Scrimp works**, showing the files done and the space saved so far, with Cancel, which stops after the file in progress. At the end, a summary of what was saved and why any files were left alone.
- **Try Again as Administrator.** When Windows doesn't allow changes to some files, the summary offers to try just those again as administrator. Windows asks first, and Scrimp then lists exactly what it will change before it starts.
- **A short note the first time you compress**, saying your files stay exactly as they are and how to undo it.
- **`scrimp update`** installs the newest release, checking the download first. `scrimp update --check` only says whether there is one. Scrimp never checks by itself.
- **An installer**, `Scrimp-windows-x64-setup.exe`. It needs no administrator, adds the menu and the `scrimp` command, and appears in Installed apps, where Scrimp can be removed again. Removing it leaves files Scrimp compressed working as normal.

## [0.1.0] - 2026-11-26

### Added

- **The `scrimp` command line.** `scrimp compress` saves space on files and folders using the compression built into Windows: your files stay exactly as they are, just smaller on disk, and apps open them as normal. `scrimp decompress` undoes it. `scrimp inspect` shows how a file is stored, and `scrimp analyse` shows how much space a folder takes and what compressing it would do, without changing anything.
- **Automatic choices.** Scrimp picks a method for each kind of file, and leaves alone photos, videos and archives (already compressed), files that change often (Windows would undo the saving), Windows' own files, links, cloud files and encrypted files.
- **Honest results.** At the end Scrimp says what it saved, such as "Saved 35.2 GB. Games now takes up 51.2 GB instead of 86.4 GB.", and why any files were left alone.
- **For scripts:** `--json` output, `--dry-run`, `--workers`, and documented exit codes. See [docs/command-line.md](docs/command-line.md).
- **Settings** in `%LOCALAPPDATA%\Scrimp\settings.json`: the default method, the smallest file worth compressing, what to do with files that change often, and per-extension choices.

[Unreleased]: https://github.com/markcastle/scrimp/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/markcastle/scrimp/releases/tag/v0.1.0
