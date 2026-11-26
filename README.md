# Scrimp

**Same files. Less space.**

Scrimp saves disk space on Windows using the compression already built into NTFS. Right-click a file, a folder or a whole drive in Explorer, choose **Scrimp Compress**, and your files stay exactly as they are, just smaller on disk. Apps open them as normal. **Scrimp Decompress** undoes it at any time.

It isn't an archive format and it doesn't add a compression engine of its own. Windows has had transparent file compression for years (it is what `compact.exe` uses) but no friendly way to reach it. Scrimp is that way in.

## Status

Scrimp is at the very start of development. Nothing is released yet, and nothing here is ready to run on your files.

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

Not yet. When 0.1 is released, one line in PowerShell will install the command line:

```powershell
irm https://raw.githubusercontent.com/markcastle/scrimp/main/install.ps1 | iex
```

## Building it yourself

See [docs/developing.md](docs/developing.md).

## Licence

MIT, see [LICENSE](LICENSE). The Scrimp name and icon are covered separately, see [TRADEMARKS.md](TRADEMARKS.md).
