# The command line

`scrimp` does everything Scrimp does from Explorer, from a terminal or a script. Your files stay exactly as they are, just smaller on disk, and apps open them as normal.

## Commands

### Compress

```powershell
scrimp compress "D:\Games"
```

Compresses every file in the folder and the folders inside it, choosing a method for each kind of file. Links are never followed. Photos, videos and archives are left alone, because they're already compressed, and so are files that change often, such as virtual disks and databases, because Windows would undo the saving the next time they change.

A large file of a kind Scrimp doesn't recognise, 1 MB or more, is sampled first: Scrimp compresses three small pieces of it in memory, and leaves the file alone if they don't shrink. Many such files (game archives, disk images) are compressed already, and this finds out in milliseconds rather than after compressing the whole file. Sampling never changes the file, and it's skipped when you choose a method yourself. In `--json`, a file left alone this way has the reason `wontShrink`.

To choose the method yourself:

```powershell
scrimp compress "D:\Archive" --algorithm lzx
```

| Method | What it's for |
| --- | --- |
| `auto` | Scrimp chooses for each file (the default) |
| `xpress4k` | Fastest to open |
| `xpress8k` | A balance of speed and size |
| `xpress16k` | Smaller |
| `lzx` | Smallest |
| `ntfs` | Files that change often: it stays in place when a file is written to |

A method you choose applies to every file, except the ones Scrimp never touches: Windows' own files, links, cloud files, encrypted files and files under 8 KB.

You can give several files and folders at once. A file or folder inside another one you gave is only handled once.

### Decompress

```powershell
scrimp decompress "D:\Games"
```

Removes compression, so files take their full size again. Before each file, Scrimp checks there's room, and it always leaves the larger of 1 GB or 2% of the drive free.

### Inspect

```powershell
scrimp inspect "D:\Archive\Example.dat"
```

```text
D:\Archive\Example.dat: compressed with LZX. 4.8 GB, taking 2.2 GB on disk (55% saved).
```

### Analyse

```powershell
scrimp analyse "D:\Games"
```

```text
Games: 6,000 files, 86.4 GB, taking 86.4 GB on disk.
  Not compressed: 6,000 files, 86.4 GB.
Scrimp would compress 5,138 files, 62.0 GB in all. How much that saves depends on what's in them.
862 files are already as small as they can be.
```

Changes nothing. `analyze` works too.

### Update

```powershell
scrimp update
```

Checks GitHub for a newer release and installs it. If Scrimp was installed with the installer, the new installer is downloaded, checked and started quietly, and it carries on once `scrimp` has finished. If only the command line was installed, the new `scrimp.exe` takes the old one's place.

`scrimp update --check` only says whether there's a newer version. Scrimp never checks by itself: this is the only time it contacts the internet, and only when you run it. Every download is checked against the release's `SHA256SUMS` file, and one that doesn't match is thrown away.

## Options

| Option | Works with | What it does |
| --- | --- | --- |
| `-a`, `--algorithm METHOD` | `compress` | The method to use, from the table above |
| `--dry-run` | `compress`, `decompress` | Shows what would happen without changing anything |
| `--json` | all | One JSON object per line, for scripts (see below) |
| `--workers N` | `compress`, `decompress` | Works on N files at once on every drive, from 1 to 16 |
| `--settings FILE` | all | Uses this settings file instead of the usual one |

Without `--workers`, Scrimp chooses by the kind of drive: one file at a time on a spinning disk, a USB drive or a drive it can't identify, and up to four on an SSD.

Press **Ctrl+C** to stop. The file being worked on finishes (Windows can't stop part-way through a file), files already done keep their new size, and nothing else starts.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Every file reached its target, or was left alone for a normal reason |
| 1 | Finished, but some files couldn't be changed, or a folder couldn't be opened |
| 2 | The command line was wrong |
| 3 | Nothing could be done: every file is on a drive that can't compress |
| 4 | Stopped with Ctrl+C |
| 5 | Something went wrong before any work began, such as a broken settings file |

## JSON output

With `--json`, every line is one JSON object with a `schemaVersion` (now `1`) and a `type`. Nothing else is written to the output, so each line can be parsed on its own.

`compress` and `decompress` write a `file` line per file, then a `summary`:

```json
{"schemaVersion":1,"type":"file","path":"D:\\Games\\notes.txt","outcome":"compressed","method":"lzx","logicalBytes":2097152,"beforeBytes":2097152,"afterBytes":102400}
{"schemaVersion":1,"type":"file","path":"D:\\Games\\Holiday.mp4","outcome":"skipped","reason":"alreadyCompressedFormat","onDiskBytes":1048576}
{"schemaVersion":1,"type":"summary","dryRun":false,"beforeBytes":5242880,"afterBytes":3248128,"savedBytes":1994752,"failures":0,"lines":["Saved 1.9 MB. Games now takes up 3.1 MB instead of 5.0 MB.","1 file is already as small as it can be."]}
```

| `outcome` | Also has |
| --- | --- |
| `compressed` | `method`, `logicalBytes`, `beforeBytes`, `afterBytes` |
| `decompressed` | `logicalBytes`, `beforeBytes`, `afterBytes` |
| `alreadyDone`, `notBeneficial` | `onDiskBytes` |
| `skipped` | `reason`, `onDiskBytes` |
| `failed` | `reason`, `leftAs` (the file's state now), `onDiskBytes`, and `code` and `message` for a Windows error |
| `cancelled` | |

In a dry run, `compressed` and `decompressed` become `wouldCompress` and `wouldDecompress` with no `afterBytes`, and the summary has `wouldChangeFiles` and `wouldChangeBytes` instead of `afterBytes` and `savedBytes`.

Other lines: `folderUnreadable` (`path`, `reason`); `wofUnavailable` (`drive`), sent once when Windows' compression driver isn't running on a drive; and `virtualDisk` (`drive`), sent once when a drive is a mounted VHD or VHDX, whose file only gets smaller on its own drive once it's compacted.

`inspect` writes a `file` line per path with `state`, `logicalBytes` and `onDiskBytes` (and `reason` when the state is `unsupported`), or an `error` line. `analyse` writes one `analysis` line with totals, `byState`, `wouldCompress` and `lines`.

States: `uncompressed`, `xpress4k`, `xpress8k`, `xpress16k`, `lzx`, `ntfs`, `wimBacked`, `unsupported`, `unknown`.

## Settings

`%LOCALAPPDATA%\Scrimp\settings.json`, if it exists. Every setting is optional.

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

| Setting | What it does |
| --- | --- |
| `defaultMethod` | The method `compress` uses when you don't choose one |
| `minimumFileSizeBytes` | Smaller files are never compressed |
| `filesThatChangeOften` | `skip`, or `ntfs` to use NTFS compression for them |
| `workers` | Files worked on at once, by kind of drive, from 1 to 16 |
| `extensions` | A method, or `skip`, for files with these extensions in automatic mode |

A mistake in the file stops Scrimp with exit code 5 and a message naming the setting.
