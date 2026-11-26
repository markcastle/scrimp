//! Scrimp Compress and Scrimp Decompress as Explorer commands.
//!
//! What Explorer sees (docs/plan.md section 4):
//!
//! - **Scrimp Compress** for anything that can be compressed, and
//!   **Scrimp Decompress** for anything that can be decompressed. For a
//!   single file the state decides; for folders, drives and several items
//!   both show.
//! - Nothing at all on a drive that can't compress (FAT32, exFAT, ReFS,
//!   network).
//! - With Shift held, Scrimp Compress opens a submenu: Automatic, Faster
//!   to Open, Smallest, For Files That Change.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use scrimp_core::{Algorithm, CompressionState, Target, jobs_folder};
use windows::Win32::Foundation::{E_FAIL, E_NOTIMPL, E_UNEXPECTED, S_FALSE, S_OK};
use windows::Win32::System::Com::{CoTaskMemAlloc, CoTaskMemFree, IBindCtx};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_SHIFT};
use windows::Win32::UI::Shell::{
    ECF_DEFAULT, ECF_HASSUBCOMMANDS, ECS_ENABLED, ECS_HIDDEN, IEnumExplorerCommand, IEnumExplorerCommand_Impl,
    IExplorerCommand, IExplorerCommand_Impl, IShellItemArray, SIGDN_FILESYSPATH,
};
use windows_core::{BOOL, GUID, HRESULT, PWSTR, Ref, Result, implement};

use crate::launch::{submit, this_dll, worker_beside};

/// Live objects, so Explorer knows when the DLL can be unloaded.
pub(crate) static LIVE: AtomicUsize = AtomicUsize::new(0);

/// Counts one live object for as long as it exists.
pub(crate) struct Alive;

impl Alive {
    pub(crate) fn new() -> Self {
        LIVE.fetch_add(1, Ordering::SeqCst);
        Alive
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Runs a COM method, turning a panic into an error so it never unwinds
/// into Explorer.
pub(crate) fn guard<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| Err(E_UNEXPECTED.into()))
}

/// Which of the two menu entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Compress,
    Decompress,
}

impl Kind {
    pub(crate) fn id(self) -> u128 {
        match self {
            Kind::Compress => scrimp_core::shell_ids::COMPRESS_COMMAND,
            Kind::Decompress => scrimp_core::shell_ids::DECOMPRESS_COMMAND,
        }
    }

    pub(crate) fn from_id(id: u128) -> Option<Kind> {
        [Kind::Compress, Kind::Decompress].into_iter().find(|k| k.id() == id)
    }
}

/// The Shift submenu's items: (title, target).
const CHOICES: [(&str, Target); 4] = [
    ("Automatic", Target::Automatic),
    ("Faster to Open", Target::Algorithm(Algorithm::Xpress4k)),
    ("Smallest", Target::Algorithm(Algorithm::Lzx)),
    ("For Files That Change", Target::Algorithm(Algorithm::Lznt1)),
];

#[implement(IExplorerCommand)]
pub(crate) struct Command {
    kind: Kind,
    /// Set for an item in the Shift submenu: its index in [`CHOICES`].
    choice: Option<usize>,
    /// Makes every method panic, so tests can prove panics never escape.
    #[cfg(test)]
    pub(crate) panic: bool,
    _alive: Alive,
}

impl Command {
    pub(crate) fn new(kind: Kind) -> Self {
        Command {
            kind,
            choice: None,
            #[cfg(test)]
            panic: false,
            _alive: Alive::new(),
        }
    }

    fn check_panic(&self) {
        #[cfg(test)]
        assert!(!self.panic, "a panic for the tests");
    }

    fn title(&self) -> &'static str {
        match (self.kind, self.choice) {
            (_, Some(i)) => CHOICES[i].0,
            (Kind::Compress, None) => "Scrimp Compress",
            (Kind::Decompress, None) => "Scrimp Decompress",
        }
    }

    fn target(&self) -> Target {
        match (self.kind, self.choice) {
            (Kind::Decompress, _) => Target::Uncompressed,
            (Kind::Compress, Some(i)) => CHOICES[i].1,
            (Kind::Compress, None) => Target::Automatic,
        }
    }

    /// Whether the menu should show for these paths. `slow` says whether
    /// the disk may be touched; when it can't be, only the paths' text is
    /// looked at.
    pub(crate) fn shows_for(&self, paths: &[PathBuf], slow: bool) -> Result<bool> {
        if paths.is_empty() {
            return Ok(false);
        }
        // Network locations are left alone, decided from the path alone.
        if paths
            .iter()
            .any(|p| p.to_string_lossy().starts_with(r"\\") && !p.to_string_lossy().starts_with(r"\\?\"))
        {
            return Ok(false);
        }
        if self.choice.is_some() {
            return Ok(true);
        }
        // When Explorer can't wait, show the entry rather than ask to be
        // asked again: if Explorer never asks again, the worst case is an
        // entry that then explains why it can't help, never a missing one.
        if !slow {
            return Ok(true);
        }
        let Ok(volume) = scrimp_win::volume(&paths[0]) else {
            return Ok(false);
        };
        if volume.unsupported(Algorithm::Lzx).is_some() && volume.unsupported(Algorithm::Lznt1).is_some() {
            return Ok(false);
        }
        if let [one] = paths
            && one.is_file()
        {
            let Ok(info) = scrimp_win::inspect(one) else {
                return Ok(false);
            };
            return Ok(match (self.kind, &info.state) {
                (_, CompressionState::Unsupported(_) | CompressionState::WimBacked | CompressionState::Unknown) => {
                    false
                }
                // Compressed already: Compress only shows with Shift, to change method.
                (Kind::Compress, CompressionState::Compressed(_)) => shift_held(),
                (Kind::Compress, CompressionState::Uncompressed) => true,
                (Kind::Decompress, state) => state.is_compressed(),
            });
        }
        Ok(true)
    }
}

fn shift_held() -> bool {
    // SAFETY: reads the keyboard state; no pointers involved.
    unsafe { GetKeyState(i32::from(VK_SHIFT.0)) < 0 }
}

/// A string Explorer will free with `CoTaskMemFree`.
fn co_string(text: &str) -> Result<PWSTR> {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    // SAFETY: allocates exactly enough for `wide`; ownership passes to the caller.
    let p = unsafe { CoTaskMemAlloc(wide.len() * 2) }.cast::<u16>();
    if p.is_null() {
        return Err(E_FAIL.into());
    }
    // SAFETY: `p` has room for `wide.len()` units and doesn't overlap it.
    unsafe { std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len()) };
    Ok(PWSTR(p))
}

/// The file system paths of the selected items. A selection with anything
/// that isn't a file or folder on disk (a library, a phone) gives an error,
/// so the menu doesn't show.
pub(crate) fn paths_of(items: Ref<IShellItemArray>) -> Result<Vec<PathBuf>> {
    let Some(items) = items.as_ref() else {
        return Ok(Vec::new());
    };
    // SAFETY: `items` is a live COM interface Explorer passed in.
    let count = unsafe { items.GetCount() }?;
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        // SAFETY: `i` is in range; the returned name is freed once, below.
        let name = unsafe { items.GetItemAt(i)?.GetDisplayName(SIGDN_FILESYSPATH) }?;
        // SAFETY: `name` is a NUL-terminated string from the shell.
        let text = unsafe { name.to_string() };
        // SAFETY: frees the string the shell allocated, once.
        unsafe { CoTaskMemFree(Some(name.0.cast())) };
        out.push(PathBuf::from(text.map_err(|_| E_FAIL)?));
    }
    Ok(out)
}

impl IExplorerCommand_Impl for Command_Impl {
    fn GetTitle(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        guard(|| {
            self.check_panic();
            co_string(self.title())
        })
    }

    fn GetIcon(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }

    fn GetToolTip(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }

    fn GetCanonicalName(&self) -> Result<GUID> {
        guard(|| {
            self.check_panic();
            Ok(GUID::from_u128(
                self.kind.id().wrapping_add(self.choice.map_or(0, |i| i as u128 + 1)),
            ))
        })
    }

    fn GetState(&self, items: Ref<IShellItemArray>, ok_to_be_slow: BOOL) -> Result<u32> {
        guard(|| {
            self.check_panic();
            let Ok(paths) = paths_of(items) else {
                return Ok(ECS_HIDDEN.0 as u32);
            };
            let shows = self.shows_for(&paths, ok_to_be_slow.as_bool())?;
            Ok(if shows {
                ECS_ENABLED.0 as u32
            } else {
                ECS_HIDDEN.0 as u32
            })
        })
    }

    fn Invoke(&self, items: Ref<IShellItemArray>, _context: Ref<IBindCtx>) -> Result<()> {
        guard(|| {
            self.check_panic();
            let paths = paths_of(items)?;
            let jobs = jobs_folder().ok_or(E_FAIL)?;
            let worker = this_dll().as_deref().and_then(worker_beside).ok_or(E_FAIL)?;
            submit(paths, self.target(), &jobs, &worker).map_err(|_| E_FAIL)?;
            Ok(())
        })
    }

    fn GetFlags(&self) -> Result<u32> {
        guard(|| {
            self.check_panic();
            let submenu = self.kind == Kind::Compress && self.choice.is_none() && shift_held();
            Ok(if submenu {
                ECF_HASSUBCOMMANDS.0 as u32
            } else {
                ECF_DEFAULT.0 as u32
            })
        })
    }

    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        guard(|| {
            self.check_panic();
            if self.kind != Kind::Compress || self.choice.is_some() {
                return Err(E_NOTIMPL.into());
            }
            let items = (0..CHOICES.len())
                .map(|i| {
                    let mut c = Command::new(Kind::Compress);
                    c.choice = Some(i);
                    c.into()
                })
                .collect();
            Ok(Choices::new(items).into())
        })
    }
}

/// The Shift submenu's items, handed out one by one.
#[implement(IEnumExplorerCommand)]
pub(crate) struct Choices {
    items: Vec<IExplorerCommand>,
    next: AtomicUsize,
    _alive: Alive,
}

impl Choices {
    fn new(items: Vec<IExplorerCommand>) -> Self {
        Choices {
            items,
            next: AtomicUsize::new(0),
            _alive: Alive::new(),
        }
    }
}

impl IEnumExplorerCommand_Impl for Choices_Impl {
    fn Next(&self, count: u32, out: *mut Option<IExplorerCommand>, fetched: *mut u32) -> HRESULT {
        guard(|| {
            let mut given = 0u32;
            while given < count {
                let i = self.next.fetch_add(1, Ordering::SeqCst);
                let Some(item) = self.items.get(i) else {
                    self.next.store(self.items.len(), Ordering::SeqCst);
                    break;
                };
                // SAFETY: Explorer passes room for `count` items at `out`.
                unsafe { out.add(given as usize).write(Some(item.clone())) };
                given += 1;
            }
            if !fetched.is_null() {
                // SAFETY: Explorer passed a place for the count, checked not null.
                unsafe { fetched.write(given) };
            }
            Ok(if given == count { S_OK } else { S_FALSE })
        })
        .unwrap_or(E_UNEXPECTED)
    }

    fn Skip(&self, count: u32) -> Result<()> {
        self.next.fetch_add(count as usize, Ordering::SeqCst);
        Ok(())
    }

    fn Reset(&self) -> Result<()> {
        self.next.store(0, Ordering::SeqCst);
        Ok(())
    }

    fn Clone(&self) -> Result<IEnumExplorerCommand> {
        Err(E_NOTIMPL.into())
    }
}
