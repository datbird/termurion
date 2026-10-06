//! 13u Task 9 Step 0: does a layered window (`WS_EX_LAYERED`, `LWA_ALPHA`) still present and
//! read back correctly through the wgpu swap chain Tandem uses? Ignored by default: it makes a
//! real (off screen, tool, never activated) window, so it runs only on request:
//! `scripts/xt test --lib fade_probe -- --ignored --nocapture`.
//!
//! Over ssh the process runs in session 0, which has no composed desktop: `PrintWindow` reads black
//! even before the layered style is added, and a second `SetLayeredWindowAttributes` on a window is
//! refused (error 31). So here the probe proves only that the style and the attribute apply and that
//! every present returns without error; the color readback needs the same test run in the person's
//! desktop session (a checklist item).

use std::ptr::null;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, RDW_ALLCHILDREN, RDW_FRAME, RDW_INVALIDATE, RedrawWindow, ReleaseDC, SelectObject};
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const SIZE: i32 = 64;
const COLOR: [f32; 4] = [0.8, 0.4, 0.2, 1.0];

fn make_window() -> HWND {
    unsafe {
        let class: Vec<u16> = "TandemFadeProbe\0".encode_utf16().collect();
        let wc = WNDCLASSW { lpfnWndProc: Some(DefWindowProcW), lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        let h = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), class.as_ptr(), WS_POPUP, -30000, -30000, SIZE, SIZE, 0 as _, 0 as _, 0 as _, null());
        assert!(!h.is_null(), "CreateWindowExW failed");
        // Cloaked, so it is never drawn on the desktop, yet DWM keeps composing its surface.
        let one: i32 = 1;
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(h, windows_sys::Win32::Graphics::Dwm::DWMWA_CLOAK as u32, (&one as *const i32).cast(), 4);
        ShowWindow(h, SW_SHOWNOACTIVATE);
        h
    }
}

fn pump() {
    unsafe {
        let mut m: MSG = std::mem::zeroed();
        while PeekMessageW(&mut m, 0 as _, 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(60));
}

/// The window's centre pixel as (r, g, b), by `PrintWindow` with `PW_RENDERFULLCONTENT`.
fn print_pixel(h: HWND) -> [u8; 3] {
    unsafe {
        let screen = GetDC(0 as _);
        let dc = CreateCompatibleDC(screen);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: SIZE, biHeight: -SIZE, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..std::mem::zeroed() };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, 0 as _, 0);
        let old = SelectObject(dc, bmp as _);
        let ok = PrintWindow(h, dc, 2);
        assert!(ok != 0, "PrintWindow failed");
        let px = std::slice::from_raw_parts(bits as *const u8, (SIZE * SIZE * 4) as usize);
        let at = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
        let out = [px[at + 2], px[at + 1], px[at]];
        SelectObject(dc, old);
        DeleteObject(bmp as _);
        DeleteDC(dc);
        ReleaseDC(0 as _, screen);
        out
    }
}

fn near(got: [u8; 3], want: [u8; 3], tol: i32) -> bool {
    got.iter().zip(want).all(|(g, w)| (*g as i32 - w as i32).abs() <= tol)
}

#[test]
#[ignore = "makes a real off screen window on the desktop session"]
fn layered_windows_present_with_the_swap_chain() {
    probe();
}

fn probe() {
    use crate::render::{Renderer, test_gpu};
    use crate::scene::Frame;
    use winit::raw_window_handle::{RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle};

    let (instance, adapter, device, queue) = test_gpu::shared_all();
    let hwnd = make_window();
    let surface = unsafe {
        let mut wh = Win32WindowHandle::new(std::num::NonZeroIsize::new(hwnd as isize).unwrap());
        wh.hinstance = std::num::NonZeroIsize::new(windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(null()) as isize);
        instance
            .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle { raw_display_handle: Some(RawDisplayHandle::Windows(WindowsDisplayHandle::new())), raw_window_handle: RawWindowHandle::Win32(wh) })
            .expect("surface")
    };
    // The shared adapter may be one this surface cannot present on: ask the same shared
    // instance for one that can (a second device, never a second instance).
    let _ = (&adapter, &device, &queue);
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() })).expect("an adapter that presents");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device");
    println!("PROBE using {:?} {}", adapter.get_info().backend, adapter.get_info().name);
    let caps = surface.get_capabilities(&adapter);
    let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: SIZE as u32,
        height: SIZE as u32,
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: crate::transparency::pick_alpha_mode(&caps.alpha_modes, false),
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
        color_space: wgpu::SurfaceColorSpace::Auto,
    };
    surface.configure(&device, &config);
    let info = adapter.get_info();
    println!("PROBE adapter: backend={:?} name={} format={format:?} alpha={:?}", info.backend, info.name, config.alpha_mode);
    let mut renderer = Renderer::new(&device, &queue, format, "Consolas", 16.0);
    let frame = Frame { clear: COLOR, ..Default::default() };
    // Three frames; false when acquiring or drawing did not succeed.
    let mut present = |label: &str| -> bool {
        let mut all_ok = true;
        for i in 0..3 {
            match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(t) => {
                    let view = t.texture.create_view(&Default::default());
                    if let Err(e) = renderer.draw(&frame, &view, SIZE as u32, SIZE as u32) {
                        println!("PROBE {label} frame {i}: draw error {e}");
                        all_ok = false;
                    }
                    queue.present(t);
                }
                other => {
                    println!("PROBE {label} frame {i}: acquire gave {}", match other {
                        wgpu::CurrentSurfaceTexture::Timeout => "Timeout",
                        wgpu::CurrentSurfaceTexture::Occluded => "Occluded",
                        wgpu::CurrentSurfaceTexture::Outdated => "Outdated",
                        wgpu::CurrentSurfaceTexture::Suboptimal(_) => "Suboptimal",
                        wgpu::CurrentSurfaceTexture::Lost => "Lost",
                        wgpu::CurrentSurfaceTexture::Validation => "Validation",
                        wgpu::CurrentSurfaceTexture::Success(_) => unreachable!(),
                    });
                    all_ok = false;
                }
            }
            pump();
        }
        all_ok
    };
    // The sRGB target stores the encoded color: the clear 0.8, 0.4, 0.2 reads as 231, 170, 124.
    let srgb = |c: f32| (if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 } * 255.0).round() as u8;
    let want = if format.is_srgb() { [srgb(COLOR[0]), srgb(COLOR[1]), srgb(COLOR[2])] } else { [204, 102, 51] };

    let plain_ok = present("opaque");
    let plain = print_pixel(hwnd);
    println!("PROBE opaque: present_ok={plain_ok} pixel={plain:?} want={want:?}");

    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED as isize);
        let set = SetLayeredWindowAttributes(hwnd, 0, 128, LWA_ALPHA);
        println!("PROBE SetLayeredWindowAttributes ok={}", set != 0);
        assert!(set != 0);
        let (mut key, mut alpha, mut flags) = (0u32, 0u8, 0u32);
        GetLayeredWindowAttributes(hwnd, &mut key, &mut alpha, &mut flags);
        println!("PROBE layered attributes: alpha={alpha} flags={flags}");
        assert_eq!((alpha, flags), (128, LWA_ALPHA));
    }
    let layered_ok = present("layered");
    let layered = print_pixel(hwnd);
    println!("PROBE layered: present_ok={layered_ok} pixel={layered:?}");

    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex & !(WS_EX_LAYERED as isize));
        RedrawWindow(hwnd, null::<RECT>(), 0 as _, RDW_INVALIDATE | RDW_FRAME | RDW_ALLCHILDREN);
    }
    let back_ok = present("removed");
    let back = print_pixel(hwnd);
    println!("PROBE removed: present_ok={back_ok} pixel={back:?}");

    drop(surface);
    unsafe { DestroyWindow(hwnd) };

    println!("PROBE baseline readback matches: {}", near(plain, want, 6));
    // PrintWindow reads black even for the baseline (an off screen flip-model window is not
    // composed), so the color is not asserted: the present results are.
    assert!(plain_ok && layered_ok && back_ok, "a present failed: opaque {plain_ok}, layered {layered_ok}, removed {back_ok}");
}
