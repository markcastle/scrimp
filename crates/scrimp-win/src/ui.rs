//! The worker's windows: Windows' own task dialogs, which follow the
//! system's look, scaling and accessibility with no drawing of our own.
//!
//! Only built with the `ui` feature, which only the worker turns on. The
//! dialogs need version 6 of Windows' common controls, which a program only
//! gets by asking for it in its manifest; linking them into a program
//! without that manifest could stop it starting.

use windows::Win32::Foundation::{HWND, LPARAM, S_FALSE, S_OK, WPARAM};
use windows::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOG_NOTIFICATIONS, TASKDIALOGCONFIG, TDCBF_CANCEL_BUTTON, TDCBF_CLOSE_BUTTON,
    TDE_CONTENT, TDE_MAIN_INSTRUCTION, TDF_ALLOW_DIALOG_CANCELLATION, TDF_CALLBACK_TIMER,
    TDF_SHOW_MARQUEE_PROGRESS_BAR, TDM_CLICK_BUTTON, TDM_SET_BUTTON_ELEVATION_REQUIRED_STATE, TDM_SET_ELEMENT_TEXT,
    TDM_SET_PROGRESS_BAR_MARQUEE, TDN_BUTTON_CLICKED, TDN_CREATED, TDN_TIMER, TaskDialogIndirect,
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
/// Cancel calls `cancel` once and the dialog stays open until then. Returns
/// whether the dialog could be shown at all.
pub fn progress(title: &str, poll: &mut dyn FnMut() -> ProgressView, cancel: &mut dyn FnMut()) -> bool {
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
    unsafe { TaskDialogIndirect(&config, None, None, None) }.is_ok()
}

/// A button of a dialog's own, beside its Close or Cancel button.
#[derive(Debug, Clone, Copy)]
pub struct Button<'a> {
    pub text: &'a str,
    /// Shows Windows' shield on the button: pressing it asks for
    /// administrator rights.
    pub needs_admin: bool,
}

/// Which of Windows' own buttons a dialog ends with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    /// Close, which is also the default: for a dialog reporting what happened.
    Close,
    /// Cancel, with the dialog's first own button as the default: for a
    /// dialog asking whether to go ahead.
    Cancel,
}

/// The ID of the dialog's first own button. Windows' own use 1 to 8.
const FIRST_BUTTON: i32 = 100;
const IDCLOSE: i32 = 8;

unsafe extern "system" fn ask_callback(
    hwnd: HWND,
    msg: TASKDIALOG_NOTIFICATIONS,
    _wparam: WPARAM,
    _lparam: LPARAM,
    data: isize,
) -> HRESULT {
    std::panic::catch_unwind(|| {
        if msg == TDN_CREATED {
            // SAFETY: `data` is the slice of button IDs `ask` passed in and
            // keeps alive until TaskDialogIndirect returns.
            let shielded = unsafe { &*(data as *const Vec<i32>) };
            for id in shielded {
                // SAFETY: `hwnd` is the live dialog this callback is for.
                unsafe {
                    SendMessageW(
                        hwnd,
                        TDM_SET_BUTTON_ELEVATION_REQUIRED_STATE.0 as u32,
                        Some(WPARAM(*id as usize)),
                        Some(LPARAM(1)),
                    )
                };
            }
        }
        S_OK
    })
    .unwrap_or(S_OK)
}

/// Shows a dialog with optional details behind "Show details", the given
/// buttons of its own, and then Close or Cancel. Returns which of its own
/// buttons was pressed, or `None` for Close, Cancel or Escape.
pub fn ask(
    title: &str,
    headline: &str,
    body: &str,
    details: Option<&str>,
    buttons: &[Button<'_>],
    finish: Finish,
) -> Option<usize> {
    let (title, headline, body) = (wide(title), wide(headline), wide(body));
    let details = details.map(wide);
    let (show, hide) = (wide("Show details"), wide("Hide details"));
    let texts: Vec<Vec<u16>> = buttons.iter().map(|b| wide(b.text)).collect();
    let own: Vec<TASKDIALOG_BUTTON> = texts
        .iter()
        .zip(FIRST_BUTTON..)
        .map(|(text, id)| TASKDIALOG_BUTTON {
            nButtonID: id,
            pszButtonText: PCWSTR(text.as_ptr()),
        })
        .collect();
    let shielded: Vec<i32> = buttons
        .iter()
        .zip(FIRST_BUTTON..)
        .filter(|(b, _)| b.needs_admin)
        .map(|(_, id)| id)
        .collect();
    let (common, default) = match finish {
        Finish::Close => (TDCBF_CLOSE_BUTTON, IDCLOSE),
        Finish::Cancel if own.is_empty() => (TDCBF_CANCEL_BUTTON, IDCANCEL as i32),
        Finish::Cancel => (TDCBF_CANCEL_BUTTON, FIRST_BUTTON),
    };
    let mut config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
        dwCommonButtons: common,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(headline.as_ptr()),
        pszContent: PCWSTR(body.as_ptr()),
        cButtons: own.len() as u32,
        pButtons: own.as_ptr(),
        nDefaultButton: default,
        pfCallback: Some(ask_callback),
        lpCallbackData: &shielded as *const Vec<i32> as isize,
        ..Default::default()
    };
    if let Some(d) = &details {
        config.pszExpandedInformation = PCWSTR(d.as_ptr());
        config.pszCollapsedControlText = PCWSTR(show.as_ptr());
        config.pszExpandedControlText = PCWSTR(hide.as_ptr());
    }
    let mut pressed = 0i32;
    // SAFETY: `config` and everything it points to (strings, buttons and
    // the callback's `shielded`) live until this call returns.
    unsafe { TaskDialogIndirect(&config, Some(&mut pressed), None, None) }.ok()?;
    let index = usize::try_from(pressed.checked_sub(FIRST_BUTTON)?).ok()?;
    (index < buttons.len()).then_some(index)
}

/// Shows a message with a Close button, and optional details behind a
/// "Show details" control.
pub fn message(title: &str, headline: &str, body: &str, details: Option<&str>) {
    ask(title, headline, body, details, &[], Finish::Close);
}

#[cfg(test)]
mod tests {
    //! The dialogs shown for real and pressed through Windows' own
    //! messages, the way a person's click arrives.

    use super::*;
    use std::time::{Duration, Instant};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};

    /// Waits for the dialog titled `title`, then sends it `press`.
    /// Dialogs are shown one at a time, as a person would see them: opened
    /// from several test threads at once, one sometimes fails to open.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
        ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Returns whether the dialog was found.
    fn press_when_shown(
        title: &'static str,
        press: impl FnOnce(HWND) + Send + 'static,
    ) -> std::thread::JoinHandle<bool> {
        std::thread::spawn(move || {
            let wide_title = wide(title);
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(20) {
                // SAFETY: `wide_title` is NUL-terminated and outlives the call.
                if let Ok(hwnd) = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide_title.as_ptr())) } {
                    if !hwnd.is_invalid() {
                        std::thread::sleep(Duration::from_millis(200));
                        press(hwnd);
                        return true;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        })
    }

    fn click(hwnd: HWND, id: i32) {
        // SAFETY: `hwnd` is the dialog found by its title; posting a message
        // passes no pointers.
        let _ = unsafe { PostMessageW(Some(hwnd), TDM_CLICK_BUTTON.0 as u32, WPARAM(id as usize), LPARAM(0)) };
    }

    #[test]
    fn pressing_a_dialogs_own_button_says_which() {
        let _one = one_at_a_time();
        let presser = press_when_shown("Scrimp ui test: own button", |hwnd| click(hwnd, FIRST_BUTTON + 1));
        let pressed = ask(
            "Scrimp ui test: own button",
            "Headline",
            "Body",
            Some("Details"),
            &[
                Button {
                    text: "First",
                    needs_admin: false,
                },
                Button {
                    text: "Second",
                    needs_admin: true,
                },
            ],
            Finish::Cancel,
        );
        assert!(presser.join().unwrap(), "the dialog was never shown");
        assert_eq!(pressed, Some(1));
    }

    #[test]
    fn closing_a_message_is_not_a_choice() {
        let _one = one_at_a_time();
        let presser = press_when_shown("Scrimp ui test: close", |hwnd| {
            // SAFETY: as in `click`.
            let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        });
        message("Scrimp ui test: close", "Headline", "Body", None);
        presser.join().unwrap();

        let presser = press_when_shown("Scrimp ui test: cancel", |hwnd| click(hwnd, IDCANCEL as i32));
        let pressed = ask("Scrimp ui test: cancel", "Headline", "", None, &[], Finish::Cancel);
        presser.join().unwrap();
        assert_eq!(pressed, None);
    }

    #[test]
    fn progress_follows_the_work_and_closes_itself_when_done() {
        let _one = one_at_a_time();
        let mut polls = 0;
        let mut poll = || {
            polls += 1;
            ProgressView {
                headline: format!("Compressing Games ({polls})"),
                detail: format!("{polls} files done."),
                finished: polls > 4,
            }
        };
        let mut cancelled = false;
        assert!(progress("Scrimp ui test: progress", &mut poll, &mut || cancelled = true));
        assert!(polls > 4);
        assert!(!cancelled, "finishing isn't cancelling");
    }

    #[test]
    fn cancel_asks_the_work_to_stop_and_waits_for_it() {
        let _one = one_at_a_time();
        let presser = press_when_shown("Scrimp ui test: cancel progress", |hwnd| click(hwnd, IDCANCEL as i32));
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let started = Instant::now();
        let mut poll = || ProgressView {
            headline: "Compressing Games".into(),
            detail: String::new(),
            // Finishes a little after Cancel, as the file in progress would.
            finished: cancelled.load(std::sync::atomic::Ordering::SeqCst)
                || started.elapsed() > Duration::from_secs(30),
        };
        let mut cancel = || cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
        let shown = progress("Scrimp ui test: cancel progress", &mut poll, &mut cancel);
        let found = presser.join().unwrap();
        assert!(shown, "TaskDialogIndirect failed");
        assert!(found, "the dialog was shown but never found by its title");
        assert!(cancelled.load(std::sync::atomic::Ordering::SeqCst));
    }
}
