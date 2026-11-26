# Scrimp: Planning

Scrimp exposes the transparent file compression built into Windows through Explorer's right-click menu and a command line. Files keep their names and contents and open normally; they just take up less room on disk.

The full plan, with the reasoning behind each decision, is [docs/plan.md](docs/plan.md). This file is the engineering summary that the code follows.

## Who it is for

Someone whose drive is filling up with games, old projects or archives, who has never heard of WOF or LZX and shouldn't need to. Developers and power users get the details behind Shift + right-click and in the command line.

The promise, in their words:

- "I right-click a folder and it takes less space."
- "My files still open exactly as before."
- "I can undo it whenever I like."
- "It tells me honestly how much I saved."

## Design direction: very Apple

- **Two plain menu items:** Scrimp Compress and Scrimp Decompress. No algorithm names in the menu.
- **Show only what makes sense.** Items hide when they can't work, rather than appearing greyed out.
- **Honest numbers in plain words:** "Saved 35.2 GB. Games now takes up 51.2 GB instead of 86.4 GB."
- **Technical detail behind a door:** a Details disclosure, Shift + right-click, `settings.json` and the CLI.

## How it fits together

| Part | What it is | Notes |
| --- | --- | --- |
| `scrimp-core` | The rules: algorithms, states, outcomes, automatic policy, planning, scheduling | Pure Rust, no Windows calls, tested on any OS |
| `scrimp-win` | Safe wrappers over the Windows compression, file and volume APIs | Added in phase 1; one of two crates allowed `unsafe` |
| `scrimp-cli` | `scrimp.exe`, the command line | |
| `scrimpw` | The worker: runs jobs from the menu, shows progress and results | One per user; takes jobs from `%LOCALAPPDATA%\Scrimp\jobs` |
| `scrimp-shell` | The Explorer menu DLL (`IExplorerCommand`) | Added in phase 4; never compresses anything itself |
| `scrimp-update` | `scrimp update`: finds, checks and installs a newer release | Added in phase 5, adapted from Keevoro |
| `xtask` | Project tooling: the changelog and cutting a release | From Keevoro |

## Crates in use

Add a row when a crate is added, with why.

| Crate | Used by | Why |
| --- | --- | --- |
| `clap` | `scrimp-cli` | Argument parsing and `--version` |
| `ctrlc` | `scrimp-cli` | Ctrl+C cancels a job between files |
| `serde_json` | `scrimp-cli` | `--json` output |
| `thiserror` | `scrimp-core` | Typed errors |
| `serde`, `serde_json` | `scrimp-core` | Reading `settings.json` |
| `anyhow`, `regex` | `xtask` | Errors and changelog parsing in tooling |
| `tempfile` | `xtask` and `scrimp-win` tests | Throwaway git repositories and test files |
| `windows` | `scrimp-win` | Microsoft's bindings to the Windows API |

## Platforms

Windows 10 22H2 and Windows 11, x64, first. The MVP's menu is registered in the user's registry, which works on Windows 10 and in Windows 11's "Show more options" menu without code signing. The Windows 11 modern menu needs a signed package and comes in 1.1. ARM64 comes later.
