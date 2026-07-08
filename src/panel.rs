use winapi::shared::minwindef::{HINSTANCE, LPARAM, LRESULT, UINT, WPARAM};
use winapi::shared::windef::{HWND, RECT};
use winapi::um::libloaderapi::GetModuleHandleW;
use winapi::um::winuser::*;
use winapi::um::wingdi::*;

use crate::AppState;

const WIDTH: i32 = 320;
const HEIGHT: i32 = 220;
const CLASS_NAME: [u16; 8] = [
    'W' as u16, 'N' as u16, 'P' as u16, 'a' as u16, 'n' as u16, 'l' as u16, 0, 0,
];

static mut PANEL_STATE: *const AppState = std::ptr::null();

pub fn show_panel(state: &AppState) {
    unsafe {
        if !PANEL_STATE.is_null() {
            return;
        }
        PANEL_STATE = state as *const AppState;

        let hinstance = GetModuleHandleW(std::ptr::null()) as HINSTANCE;

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(panel_wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            lpszMenuName: std::ptr::null(),
            lpszClassName: CLASS_NAME.as_ptr(),
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS_NAME.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            CW_USEDEFAULT, CW_USEDEFAULT, WIDTH, HEIGHT,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null_mut(),
        );

        if hwnd.is_null() {
            PANEL_STATE = std::ptr::null();
            return;
        }

        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            sw - WIDTH - 10,
            sh - HEIGHT - 60,
            WIDTH,
            HEIGHT,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        ShowWindow(hwnd, SW_SHOWNA);
        UpdateWindow(hwnd);

        let mut msg = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        PANEL_STATE = std::ptr::null();
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn panel_wndproc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);

            let ptr = PANEL_STATE;
            if !ptr.is_null() {
                let state = &*ptr;
                let stats = &state.stats;
                let bytes = stats.total_bytes();
                let tokens = stats.estimated_tokens();
                let credits = stats.credits();
                let elapsed = state.session_start.elapsed();
                let h = elapsed.as_secs() / 3600;
                let m = (elapsed.as_secs() % 3600) / 60;
                let s = elapsed.as_secs() % 60;
                let modo = if state.hardware.has_nvidia_gpu {
                    "GPU"
                } else {
                    "CPU"
                };

                SetBkMode(hdc, TRANSPARENT as i32);

                let lines = [
                    format!("  Worker Node - Panel"),
                    format!(""),
                    format!("  Modo:                {modo}"),
                    format!("  Creditos:             {credits}"),
                    format!("  Total:                {} KB", bytes / 1024),
                    format!("  Tokens estimados:     {tokens}"),
                    format!("  Sesion:               {h:02}h {m:02}m {s:02}s"),
                    format!(""),
                    format!("  Click aqui para cerrar"),
                ];

                let mut y = 10i32;
                for line in &lines {
                    let wide = to_wide(line);
                    let mut r = RECT {
                        left: 0,
                        top: y,
                        right: WIDTH,
                        bottom: y + 24,
                    };
                    DrawTextW(hdc, wide.as_ptr(), -1, &mut r, DT_LEFT | DT_TOP);
                    y += 22;
                }
            }

            EndPaint(hwnd, &ps);
            0
        }
        WM_LBUTTONDOWN => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
