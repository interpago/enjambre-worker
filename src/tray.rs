use std::sync::Arc;
use winapi::shared::minwindef::*;
use winapi::shared::windef::*;
use winapi::um::libloaderapi::GetModuleHandleW;
use winapi::um::shellapi::*;
use winapi::um::winuser::*;
use winapi::um::wingdi::*;

use crate::AppState;

const WM_TRAY: UINT = WM_APP + 1;
const ID_EXIT: UINT = 1001;
const TRAY_CLASS: [u16; 12] = [
    'W' as u16, 'k' as u16, 'T' as u16, 'r' as u16, 'a' as u16, 'y' as u16,
    'C' as u16, 'l' as u16, 'a' as u16, 's' as u16, 's' as u16, 0,
];

static mut APP_STATE: *const AppState = std::ptr::null();
static mut SHOULD_EXIT: bool = false;

pub fn run(state: &Arc<AppState>) {
    unsafe {
        APP_STATE = Arc::as_ptr(state);

        let hi = GetModuleHandleW(std::ptr::null()) as HINSTANCE;

        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(tray_wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hi,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: TRAY_CLASS.as_ptr(),
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            0,
            TRAY_CLASS.as_ptr(),
            std::ptr::null(),
            0,
            CW_USEDEFAULT, CW_USEDEFAULT, 0, 0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hi,
            std::ptr::null_mut(),
        );

        if hwnd.is_null() {
            return;
        }

        let hicon = create_green_icon();
        let tip = to_wide("Worker Node - Enjambre");

        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as DWORD;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = hicon;

        let tip_len = tip.len().min(128);
        let dst = std::slice::from_raw_parts_mut(nid.szTip.as_mut_ptr(), 128);
        dst[..tip_len].copy_from_slice(&tip[..tip_len]);

        Shell_NotifyIconW(NIM_ADD, &mut nid);

        let mut msg = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if SHOULD_EXIT {
                break;
            }
        }

        Shell_NotifyIconW(NIM_DELETE, &mut nid);
        DeleteObject(hicon as HGDIOBJ);
        APP_STATE = std::ptr::null();
    }
}

unsafe extern "system" fn tray_wndproc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAY => match lparam as UINT {
            WM_LBUTTONDOWN => {
                if !APP_STATE.is_null() {
                    crate::panel::show_panel(&*APP_STATE);
                }
                0
            }
            WM_RBUTTONDOWN => {
                let hmenu = CreatePopupMenu();
                let exit_text = to_wide("Salir");
                AppendMenuW(hmenu, MF_STRING, ID_EXIT as usize, exit_text.as_ptr());
                SetForegroundWindow(hwnd);

                let mut pt = std::mem::zeroed();
                GetCursorPos(&mut pt);
                TrackPopupMenu(
                    hmenu,
                    TPM_RIGHTBUTTON,
                    pt.x,
                    pt.y,
                    0,
                    hwnd,
                    std::ptr::null_mut(),
                );
                DestroyMenu(hmenu);
                0
            }
            _ => 0,
        },
        WM_COMMAND => {
            let id = (wparam as DWORD & 0xFFFF) as UINT;
            if id == ID_EXIT {
                SHOULD_EXIT = true;
                PostQuitMessage(0);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn create_green_icon() -> HICON {
    let size = 32i32;
    let n = (size * size * 4) as usize;
    let mut rgba = vec![0u8; n];
    for p in rgba.chunks_mut(4) {
        p[0] = 0;
        p[1] = 204;
        p[2] = 0;
        p[3] = 255;
    }

    unsafe {
        let hbmp = CreateBitmap(size, size, 1, 32, rgba.as_ptr() as *const _);
        if hbmp.is_null() {
            return std::ptr::null_mut();
        }

        let mut ii: ICONINFO = std::mem::zeroed();
        ii.fIcon = TRUE as BOOL;
        ii.hbmMask = hbmp;
        ii.hbmColor = hbmp;

        let hicon = CreateIconIndirect(&mut ii);
        DeleteObject(hbmp as HGDIOBJ);
        hicon
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
