# How Windows compression really behaves

These are the Phase 0 findings: what Windows actually does when Scrimp asks it to compress, decompress and inspect files. They come from running `spikes/wof-probe` on real files, not from documentation, and every decision in `scrimp-win` should be traceable to a line here.

Where a finding surprised us, it says so. Where it changes the plan, it says what changes.

## Where these results come from

The probe ran on GitHub's `windows-latest` runner, which is **Windows Server 2025 (build 26100)**, against:

- the runner's own C: drive (NTFS);
- small VHDX volumes mounted by the workflow: NTFS with 4 KB clusters, NTFS with 64 KB clusters, exFAT and ReFS;
- an SMB share of an NTFS folder, reached as `\\localhost\scrimpprobe`;
- a file encrypted with EFS.

The full reports are on [markcastle/scrimp#1](https://github.com/markcastle/scrimp/pull/1). Some answers may differ on Windows 10 and Windows 11 desktops; those are listed at the end as things to try on a real PC.

## The short version

| Question | Answer | What Scrimp does |
| --- | --- | --- |
| Which API compresses? | `WofSetFileDataLocation` and `FSCTL_SET_EXTERNAL_BACKING` behave identically | Use `WofSetFileDataLocation` |
| What access does WOF need? | Read access is enough, for compressing and decompressing | Open with `GENERIC_READ` only |
| Is opening for write safe? | **No. Opening a WOF file with write access decompresses it** | Never open a file for writing during WOF work |
| What does NTFS compression need? | `FILE_READ_DATA` + `FILE_WRITE_DATA` | Open for read and write only for NTFS compression |
| Do timestamps change? | No, for every method | Nothing to restore |
| Which size is the real size? | `GetCompressedFileSizeW` and `AllocationSize` both match the space actually used | Use `GetCompressedFileSizeW` |
| What if a file won't shrink? | WOF returns error 344 and leaves the file untouched; NTFS compression marks it compressed anyway | Treat 344 as `NotBeneficial`; undo NTFS compression that saved nothing |
| Can compressing one large file be cancelled? | No | Cancel between files only, and say so |
| Can one method change straight to another? | Yes, except NTFS to WOF leaves both applied | Remove NTFS compression before applying WOF |
| Does WOF work on any mounted volume? | Only if Windows' WOF driver is attached to it, and it isn't for a VHDX mounted after boot | Detect it and explain, rather than failing every file |

## Compressing

### Both routes work the same

`WofSetFileDataLocation` (in `wofutil.dll`) and `DeviceIoControl` with `FSCTL_SET_EXTERNAL_BACKING` (what `compact.exe` uses) gave identical results for all four WOF algorithms: same sizes, same states, same errors.

Scrimp uses **`WofSetFileDataLocation`**. It takes a smaller structure, and `windows-rs` binds it directly.

### The structure trap

**This cost us a run.** The two routes take different structures for the same information:

| Route | Structure | Fields |
| --- | --- | --- |
| `WofSetFileDataLocation`, `WofIsExternalFile` | `WOF_FILE_COMPRESSION_INFO_V1` | `Algorithm`, `Flags` |
| `FSCTL_SET_EXTERNAL_BACKING` | `WOF_EXTERNAL_INFO` then `FILE_PROVIDER_EXTERNAL_INFO_V1` | `Version`, `Provider`, then `Version`, `Algorithm`, `Flags` |

Passing the FSCTL's `FILE_PROVIDER_EXTERNAL_INFO_V1` to `WofSetFileDataLocation` doesn't fail cleanly. Windows reads the version number (1) as the algorithm (1 is LZX) and the algorithm as flags. So asking for XPRESS4K silently gives you LZX, and asking for LZX fails with "The parameter is incorrect". `scrimp-win` needs a test that compresses with each algorithm and reads the algorithm back.

The algorithm numbers are: XPRESS4K 0, LZX 1, XPRESS8K 2, XPRESS16K 3.

### Access rights

| Open with | WOF compress | WOF remove | NTFS on | NTFS off |
| --- | --- | --- | --- | --- |
| `GENERIC_READ` | ok | ok | denied | denied |
| `FILE_READ_DATA` | ok | ok | denied | denied |
| `FILE_READ_ATTRIBUTES` + `FILE_WRITE_ATTRIBUTES` | ok | ok | denied | denied |
| `FILE_READ_DATA` + `FILE_WRITE_DATA` | ok | **fails, see below** | ok | ok |
| `GENERIC_READ` + `GENERIC_WRITE` | ok | **fails, see below** | ok | ok |

**Surprise: opening a WOF-compressed file with write access decompresses it.** The "remove" cell fails with error 342, "This object is not externally backed by any provider", because by the time we ask, the act of opening the file for writing has already undone the compression. Nothing needs to be written.

What this means for Scrimp:

- Every WOF operation (inspect, compress, decompress) opens the file with **`GENERIC_READ` only**.
- Opening for write is reserved for NTFS compression, and only after checking the file has no WOF compression to lose.
- Anything else that opens a file for writing (some backup, sync and antivirus tools do) will quietly undo WOF compression. That belongs in the help text, alongside "files that change often".

### Read-only files

Read-only files can't be opened for writing, so NTFS compression is refused with "Access is denied". WOF compression works, because it only needs read access, and the read-only attribute is left alone.

Scrimp compresses read-only files with WOF without touching their attributes. For NTFS compression, it clears the attribute, compresses and restores it, with a guard that restores it on every path out.

### Files another program has open

| The other program opened it for | and allows others to | Scrimp's result |
| --- | --- | --- |
| reading | read only | Error 32, "being used by another process" |
| reading | read and write | Compressed |
| writing | read and write | Compressed |
| reading | nothing | Error 32 |

Error 32 maps to `FailReason::InUse`: "3 files are in use by another app."

### Timestamps

Created, modified and last-accessed times were unchanged after every compression, decompression and change of method. Backup and sync tools that watch the modified time won't see Scrimp's work as a change.

### Hardlinks, alternate data streams and sparse files

- **Hardlinks:** compressing one name compressed the file under every name, as expected. Scrimp counts the data once.
- **Alternate data streams:** survived compression intact. WOF compresses the main stream only.
- **Sparse files:** both WOF and NTFS compression worked.

## Writing to a compressed file

| Compressed with | After one line is appended |
| --- | --- |
| Any WOF algorithm | Uncompressed; content correct |
| NTFS (LZNT1) | Still compressed; content correct |

This confirms the plan: automatic mode skips files that change often, and "For Files That Change" uses NTFS compression.

**`COMPRESS_ON_WRITE` isn't available here.** Windows' headers define `FILE_PROVIDER_FLAG_COMPRESS_ON_WRITE`, which suggests WOF files might one day stay compressed when written to. On this build it was rejected with "The parameter is incorrect" through both routes. It needs trying on a current Windows 11 desktop before Scrimp relies on it; until then, Scrimp doesn't use it.

## Changing from one method to another

Every pair of methods was tried directly, without removing the first one. Content was identical after all 30 changes.

| From | To | Result |
| --- | --- | --- |
| Uncompressed | anything | Works |
| A WOF algorithm | another WOF algorithm | Works, replaces it |
| A WOF algorithm | NTFS | Works, replaces it |
| A WOF algorithm | uncompressed | Works |
| NTFS | uncompressed | Works |
| **NTFS** | **a WOF algorithm** | **Leaves both applied** |

**Surprise: NTFS to WOF leaves the file with both kinds of compression.** The NTFS flag stays set underneath the WOF compression. So `scrimp-win`:

- removes NTFS compression before applying WOF;
- reports a file with both as `Unknown` (and leaves it alone) unless Scrimp is decompressing it, in which case it removes both.

## Sizes

| Method on 64 MiB of text | Size on disk | Saved |
| --- | --- | --- |
| None | 64.0 MiB | 0% |
| NTFS (LZNT1) | 12.0 MiB | 81% |
| XPRESS4K | 11.5 MiB | 82% |
| XPRESS8K | 9.0 MiB | 86% |
| XPRESS16K | 7.7 MiB | 88% |
| LZX | 3.2 MiB | 95% |

The test text is very repetitive, so the savings are far higher than real files will see. The order of the methods is what matters.

`GetCompressedFileSizeW` and `FILE_STANDARD_INFO.AllocationSize` returned the same number, and on the dedicated test volume both matched the drop in free space to the byte. WOF keeps the compressed data in a hidden stream that `FindFirstStreamW` doesn't list, and the main stream still reports its full logical size, so stream sizes are no use for this.

Scrimp uses **`GetCompressedFileSizeW`**: it works from a path and needs no handle.

## Files that won't shrink

| File | WOF (any algorithm) | NTFS |
| --- | --- | --- |
| 8 MiB of random data | Error 344, untouched | "Succeeds", marked compressed, nothing saved |
| Empty | Error 344, untouched | "Succeeds" |
| 100 bytes | Error 344, untouched | "Succeeds" |
| 4,096 bytes | Error 344, untouched | "Succeeds" |
| Half text, half random | Compressed | Compressed |

Error 344 is `ERROR_COMPRESSION_NOT_BENEFICIAL`: "Compressing this object would not save space." It's a normal outcome, `Outcome::NotBeneficial`, and the file is left exactly as it was.

NTFS compression never reports this. It sets the compressed flag even when nothing is saved, and the file then pays NTFS compression's costs for no benefit. So after NTFS compression, Scrimp compares the size on disk before and after, and if nothing was saved it removes the compression again and reports `NotBeneficial`.

## Cancelling

LZX compressed at about **10 MiB per second** on the runner, so a 1 GiB file took 100 to 120 seconds in a single call.

| Attempt | Result |
| --- | --- |
| `FSCTL_SET_EXTERNAL_BACKING` on a handle opened for overlapped I/O | The call blocked for the whole compression; there was nothing pending to cancel |
| `WofSetFileDataLocation` on a thread, then `CancelSynchronousIo` | `CancelSynchronousIo` reported success, and the compression carried on to the end |

In both cases the file finished compressed with its content intact.

So a compression in progress can't be cancelled. Scrimp:

- cancels between files, as the plan already says;
- shows "Finishing Example.dat…" after Cancel is pressed while a large file is in progress, rather than appearing to hang;
- runs compression off the UI thread, so the window stays responsive throughout.

## Volumes and filesystems

| Volume | WOF | NTFS (LZNT1) |
| --- | --- | --- |
| NTFS, 4 KB clusters | Works | Works |
| NTFS, 64 KB clusters | Works | Error 1, "Incorrect function" |
| exFAT | Error 1 | Error 1 |
| ReFS (including Dev Drives) | **Error 344, "would not save space"** | Error 50, "not supported" |
| SMB share of an NTFS folder | Error 50, "not supported" | **Works** |
| EFS-encrypted file on NTFS | Accepted | Error 1 |

Three surprises:

1. **ReFS answers WOF with "would not save space"**, not "not supported". If Scrimp trusted the error, a Dev Drive would look like a drive full of incompressible files. Scrimp checks the filesystem name (`GetVolumeInformationW`) before trying anything, and only offers WOF on NTFS.
2. **NTFS compression works over SMB.** The plan keeps network locations out of the MVP anyway, but this is a possible later feature.
3. **WOF accepted an EFS-encrypted file.** Whether the file is still readable and still encrypted afterwards wasn't checked. Until it is, Scrimp skips EFS files as the plan says.

`GetVolumeInformationW`'s `FILE_FILE_COMPRESSION` flag was set only on NTFS with 4 KB clusters. It describes NTFS compression only, so it can't be used to decide whether WOF is available.

## The WOF driver must be attached to the volume

**Surprise: Windows didn't attach its WOF driver to a VHDX mounted after boot.** On such a volume, every WOF call, including `compact.exe`'s, failed with error 1, "Incorrect function". NTFS compression still worked. After `fltmc attach wof N:` (which needs an administrator), WOF worked normally.

This affects the plan's promise that a mounted VHDX "just works". On this Windows Server build it doesn't. Scrimp:

- treats error 1 from a WOF call on an NTFS volume as "WOF isn't running on this drive", not as a per-file failure;
- stops trying WOF on that volume for the rest of the job;
- tells the person in plain words, and offers NTFS compression instead.

Whether Windows 10 and 11 desktops attach WOF to a newly mounted VHDX by themselves is the most important thing to try on a real PC.

## Windows' own compressed files

On the runner, system files such as `notepad.exe` and `ntdll.dll` are already compressed with WOF LZX through the ordinary file provider, not the WIM provider. They look exactly like files Scrimp compressed. So "WIM-backed" detection alone doesn't protect Windows' own files; the protected folders list in the plan is what does.

## Try on a real PC

These need a Windows 10 or Windows 11 desktop, not a server runner. Record the results in TASKS.md under "Tried by a person".

- Does Windows attach WOF by itself to a VHDX mounted after boot?
- Is `COMPRESS_ON_WRITE` accepted on Windows 11 24H2 or 25H2, and does a written file stay compressed?
- After WOF compresses an EFS-encrypted file, is it still encrypted and still readable?
- How fast is LZX on a typical desktop CPU, compared with the runner's 10 MiB per second?

## Running the probe yourself

The probe only creates and changes its own files, in the folder you give it, and writes about 1.5 GB of test data. Use a folder on a drive with a few GB free.

```powershell
cd spikes\wof-probe
```

```powershell
cargo run --release -- report.md --main D:\scrimp-probe --volume "D drive=D:\"
```

`report.md` then holds the same tables as above, for your PC.
