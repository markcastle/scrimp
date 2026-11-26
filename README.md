# Scrimp

**Same files. Less space.**

Scrimp saves disk space on Windows using the compression already built into NTFS. Right-click a file, a folder or a whole drive in Explorer, choose **Scrimp Compress**, and your files stay exactly as they are, just smaller on disk. Apps open them as normal. **Scrimp Decompress** undoes it at any time.

It isn't an archive format and it doesn't add a compression engine of its own. Windows has had transparent file compression for years (it is what `compact.exe` uses) but no friendly way to reach it. Scrimp is that way in.

## Status

Scrimp is in early development. The command line works; the Explorer menu is next. Try it on files you have backed up first.

The plan, including what the first version will and won't do, is in [docs/plan.md](docs/plan.md). The order of work is in [TASKS.md](TASKS.md).

## What it will do

- **Scrimp Compress** and **Scrimp Decompress** in Explorer's right-click menu on Windows 10 and Windows 11.
- An automatic mode that picks a sensible method for each file, and leaves alone files that wouldn't shrink (photos, videos, archives) or that change often (databases, virtual disks).
- An honest report at the end: "Saved 35.2 GB. Games now takes up 51.2 GB instead of 86.4 GB."
- A command line, `scrimp`, that does everything the menu does, with JSON output for scripts.

## What it won't do

- No background service, no telemetry, no account.
- No network access, except when you run `scrimp update`.
- No administrator prompt for ordinary use.

## Install

One line in PowerShell installs the command line. Running it again updates it.

```powershell
irm https://raw.githubusercontent.com/markcastle/scrimp/main/install.ps1 | iex
```

## Use

```powershell
scrimp analyse "D:\Games"
```

```powershell
scrimp compress "D:\Games"
```

```powershell
scrimp decompress "D:\Games"
```

Everything else, including `--json` for scripts, is in [docs/command-line.md](docs/command-line.md).

## Building it yourself

See [docs/developing.md](docs/developing.md).

## Licence

MIT, see [LICENSE](LICENSE). The Scrimp name and icon are covered separately, see [TRADEMARKS.md](TRADEMARKS.md).
