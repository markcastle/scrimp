//! The worker's windows: Windows' own task dialogs, which follow the
//! system's look, scaling and accessibility with no drawing of our own.
//!
//! Only built with the `ui` feature, which only the worker turns on. The
//! dialogs need version 6 of Windows' common controls, which a program only
//! gets by asking for it in its manifest; linking them into a program
//! without that manifest could stop it starting.

use windows::Win32::Foundation::{HWND, LPARAM, S_FALSE, S_OK, WPARAM};
use windows::Win32::UI::Controls::{
    TASKDIALOG_NOTIFICATIONS, TASKDIALOGCONFIG, TDCBF_CANCEL_BUTTON, TDCBF_CLOSE_BUTTON, TDE_CONTENT,
    TDE_MAIN_INSTRUCTION, TDF_ALLOW_DIALOG_CANCELLATION, TDF_CALLBACK_TIMER, TDF_SHOW_MARQUEE_PROGRESS_BAR,
    TDM_CLICK_BUTTON, TDM_SET_ELEMENT_TEXT, TDM_SET_PROGRESS_BAR_MARQUEE, TDN_BUTTON_CLICKED, TDN_CREATED, TDN_TIMER,
    TaskDialogIndirect,
};
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows::core::{HRESULT, PCWSTR};

const IDCANCEL: usize = 2;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// What the progress dialog shows at a moment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProgressView {
    /// The large first line, such as "Compressing Games".
    pub headline: String,
    /// The text under it, such as files done and space saved.
    pub detail: String,
    /// When true, the dialog closes.
    pub finished: bool,
}

struct Progress<'a> {
    poll: &'a mut dyn FnMut() -> ProgressView,
    cancel: &'a mut dyn FnMut(),
    shown: ProgressView,
    closing: bool,
}

impl Progress<'_> {
    fn handle(&mut self, hwnd: HWND, msg: TASKDIALOG_NOTIFICATIONS) -> HRESULT {
        let send = |message: u32, wparam: usize, lparam: isize| {
            // SAFETY: `hwnd` is the live dialog this callback is for; the
            // message is handled synchronously, so any string passed in
            // `lparam` outlives it.
            unsafe { SendMessageW(hwnd, message, Some(WPARAM(wparam)), Some(LPARAM(lparam))) };
        };
        match msg {
            TDN_CREATED => send(TDM_SET_PROGRESS_BAR_MARQUEE.0 as u32, 1, 30),
            TDN_TIMER => {
                let view = (self.poll)();
                if view.finished {
                    self.closing = true;
                    send(TDM_CLICK_BUTTON.0 as u32, IDCANCEL, 0);
                } else {
                    if view.headline != self.shown.headline {
                        let text = wide(&view.headline);
                        send(
                            TDM_SET_ELEMENT_TEXT.0 as u32,
                            TDE_MAIN_INSTRUCTION.0 as usize,
                            text.as_ptr() as isize,
                        );
                    }
                    if view.detail != self.shown.detail {
                        let text = wide(&view.detail);
                        send(
                            TDM_SET_ELEMENT_TEXT.0 as u32,
                            TDE_CONTENT.0 as usize,
                            text.as_ptr() as isize,
                        );
                    }
                    self.shown = view;
                }
            }
            TDN_BUTTON_CLICKED => {
                if self.closing {
                    return S_OK;
                }
                // Cancel pressed: ask the job to stop, and keep the dialog
                // open until the file in progress has finished.
                (self.cancel)();
                return S_FALSE;
            }
            _ => {}
        }
        S_OK
    }
}

unsafe extern "system" fn progress_callback(
    hwnd: HWND,
    msg: TASKDIALOG_NOTIFICATIONS,
    _wparam: WPARAM,
    _lparam: LPARAM,
    data: isize,
) -> HRESULT {
    // A panic must never unwind into Windows' code.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `data` is the `Progress` that `progress` passed in and keeps
        // alive until TaskDialogIndirect returns; the dialog calls back on
        // the same thread, one call at a time.
        let state = unsafe { &mut *(data as *mut Progress<'_>) };
        state.handle(hwnd, msg)
    }))
    .unwrap_or(S_OK)
}

/// Shows a progress dialog until `poll` says the work is finished. Pressing
/// Cancel calls `cancel` once and the dialog stays open until then.
pub fn progress(title: &str, poll: &mut dyn FnMut() -> ProgressView, cancel: &mut dyn FnMut()) {
    let first = poll();
    let (title, headline, detail) = (wide(title), wide(&first.headline), wide(&first.detail));
    let mut state = Progress {
        poll,
        cancel,
        shown: first,
        closing: false,
    };
    let config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_SHOW_MARQUEE_PROGRESS_BAR | TDF_CALLBACK_TIMER | TDF_ALLOW_DIALOG_CANCELLATION,
        dwCommonButtons: TDCBF_CANCEL_BUTTON,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(headline.as_ptr()),
        pszContent: PCWSTR(detail.as_ptr()),
        pfCallback: Some(progress_callback),
        lpCallbackData: &mut state as *mut Progress<'_> as isize,
        ..Default::default()
    };
    // SAFETY: `config` and every string it points to live until this call
    // returns, as does `state`, which the callback reaches through
    // `lpCallbackData`.
    let _ = unsafe { TaskDialogIndirect(&config, None, None, None) };
}

/// Shows a message with a Close button, and optional details behind a
/// "Show details" control.
pub fn message(title: &str, headline: &str, body: &str, details: Option<&str>) {
    let (title, headline, body) = (wide(title), wide(headline), wide(body));
    let details = details.map(wide);
    let (show, hide) = (wide("Show details"), wide("Hide details"));
    let mut config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
        dwCommonButtons: TDCBF_CLOSE_BUTTON,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(headline.as_ptr()),
        pszContent: PCWSTR(body.as_ptr()),
        ..Default::default()
    };
    if let Some(d) = &details {
        config.pszExpandedInformation = PCWSTR(d.as_ptr());
        config.pszCollapsedControlText = PCWSTR(show.as_ptr());
        config.pszExpandedControlText = PCWSTR(hide.as_ptr());
    }
    // SAFETY: `config` and every string it points to live until this call returns.
    let _ = unsafe { TaskDialogIndirect(&config, None, None, None) };
}
