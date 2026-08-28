//! Persistent WebView2 renderer for silent HTML thermal receipt printing.
//!
//! Uses only types from `webview2-com` 0.38 / `windows` 0.61 (they must be the same
//! windows_core). Strings go through `CoTaskMemPWSTR` so we never mix HSTRING versions.

#![cfg(windows)]

use std::ffi::c_void;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use once_cell::sync::OnceCell;
use webview2_com::{
    CapturePreviewCompletedHandler, CoTaskMemPWSTR, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, ExecuteScriptCompletedHandler,
    Microsoft::Web::WebView2::Win32::*, NavigationCompletedEventHandler, PrintCompletedHandler,
};
use windows::core::{w, Interface, BOOL};
use windows::Win32::Foundation::{E_POINTER, HGLOBAL, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
use windows::Win32::System::Com::{
    CoInitializeEx, IStream, COINIT_APARTMENTTHREADED, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, SetWindowPos,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, PM_REMOVE, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_SHOWNOACTIVATE, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::print::escpos_raster::{self, MonoBitmap};

/// What the engine thread managed to do with a receipt.
pub enum HtmlOutcome {
    /// ESC/POS bit image, ready for the spooler's RAW channel. Includes the cut.
    Raster(Vec<u8>),
    /// Rasterising was not possible, so the WebView2 print API and the printer
    /// driver were used instead. The caller still owes the printer a cut.
    Printed,
}

struct HtmlPrintJob {
    printer: String,
    html: String,
    paper_mm: u32,
    reply: mpsc::Sender<Result<HtmlOutcome>>,
}

struct EngineHandles {
    hwnd: HWND,
    env: ICoreWebView2Environment,
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

static JOB_TX: OnceCell<mpsc::Sender<HtmlPrintJob>> = OnceCell::new();

const NAV_TIMEOUT: Duration = Duration::from_secs(30);
const PRINT_TIMEOUT: Duration = Duration::from_secs(60);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(30);

/// The render window lives off every monitor so nothing flashes on the till.
const OFFSCREEN_ORIGIN: i32 = -30_000;
/// Blank paper left after the last dot, so the cut does not clip a descender.
const TRAILING_DOT_ROWS: u32 = 24;

pub fn init() -> Result<()> {
    if JOB_TX.get().is_some() {
        return Ok(());
    }

    let (tx, rx) = mpsc::channel::<HtmlPrintJob>();
    thread::Builder::new()
        .name("finvoroo-html-print".into())
        .spawn(move || {
            if let Err(err) = engine_thread_main(rx) {
                tracing::error!("html print engine thread exited: {err:#}");
            }
        })
        .context("spawn html print engine thread")?;

    JOB_TX
        .set(tx)
        .map_err(|_| anyhow::anyhow!("html print engine already initialized"))?;
    tracing::info!("html print engine initialized");
    Ok(())
}

pub fn print_html(printer: &str, html: &str, paper_mm: u32) -> Result<HtmlOutcome> {
    let tx = JOB_TX
        .get()
        .ok_or_else(|| anyhow::anyhow!("html print engine is not initialized"))?;
    let (reply_tx, reply_rx) = mpsc::channel();
    tx.send(HtmlPrintJob {
        printer: printer.to_string(),
        html: html.to_string(),
        paper_mm,
        reply: reply_tx,
    })
    .map_err(|_| anyhow::anyhow!("html print engine is not running"))?;

    reply_rx
        .recv()
        .map_err(|_| anyhow::anyhow!("html print engine is not running"))?
}

fn engine_thread_main(rx: mpsc::Receiver<HtmlPrintJob>) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }

    let hwnd = unsafe { create_hidden_window()? };
    let handles = unsafe { create_webview(hwnd)? };
    unsafe {
        // Tall viewport so long receipts layout at full height before we measure them.
        handles.controller.SetBounds(RECT {
            left: 0,
            top: 0,
            right: 600,
            bottom: 8000,
        })?;
        // CapturePreview only returns pixels for a webview that is actually
        // compositing, so the window stays shown — parked off-screen.
        handles.controller.SetIsVisible(true)?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }

    if let Err(err) = warm_up(&handles.webview) {
        tracing::warn!("html print engine warm-up skipped: {err:#}");
    }

    for job in rx {
        let result = render_and_print(&handles, &job.printer, &job.html, job.paper_mm);
        let _ = job.reply.send(result);
    }

    Ok(())
}

fn warm_up(webview: &ICoreWebView2) -> Result<()> {
    wait_navigation(webview, "<!DOCTYPE html><html><body></body></html>")
}

fn render_and_print(
    handles: &EngineHandles,
    printer: &str,
    html: &str,
    paper_mm: u32,
) -> Result<HtmlOutcome> {
    if html.is_empty() {
        return Ok(HtmlOutcome::Printed);
    }

    wait_navigation(&handles.webview, html)?;
    wait_for_layout(&handles.webview);

    match render_raster(handles, paper_mm) {
        Ok(payload) => return Ok(HtmlOutcome::Raster(payload)),
        Err(err) => tracing::warn!(
            "receipt raster failed, falling back to driver print: {err:#}"
        ),
    }

    silent_print(&handles.env, &handles.webview, printer, paper_mm)?;
    Ok(HtmlOutcome::Printed)
}

/// Render the receipt at the printer's dot pitch and turn it into an ESC/POS bit
/// image. This is the path that avoids driver page sizes entirely.
fn render_raster(handles: &EngineHandles, paper_mm: u32) -> Result<Vec<u8>> {
    let (layout_mm, width_dots) = escpos_raster::paper_geometry(paper_mm);
    let scale = escpos_raster::rasterization_scale(layout_mm, width_dots);

    let controller3: ICoreWebView2Controller3 = handles
        .controller
        .cast()
        .context("ICoreWebView2Controller3 is unavailable — update WebView2 Runtime")?;
    unsafe {
        // Raw pixels plus a fixed rasterization scale means the till's display DPI
        // cannot change how many dots wide the receipt comes out.
        controller3.SetShouldDetectMonitorScaleChanges(false)?;
        controller3.SetBoundsMode(COREWEBVIEW2_BOUNDS_MODE_USE_RAW_PIXELS)?;
        controller3.SetRasterizationScale(scale)?;
    }

    let height_css = measure_content_height_px(&handles.webview, layout_mm)?;
    if height_css < 40.0 {
        bail!("measured receipt height {height_css}px is implausible");
    }
    // A few slack pixels keep a rounded-down last line inside the capture. The
    // extra white is trimmed off again before the cut.
    let height_dots = (((height_css + 4.0) * scale).ceil() as u32)
        .min(escpos_raster::MAX_RASTER_ROWS);

    unsafe { resize_surface(handles, width_dots, height_dots)? };
    tracing::info!(
        paper_mm,
        layout_mm,
        width_dots,
        height_css,
        height_dots,
        "rasterising receipt"
    );

    let png = capture_png(&handles.webview)?;
    let bitmap = decode_png_to_mono(&png, width_dots)?;
    if bitmap.is_blank() {
        bail!("captured receipt is blank");
    }

    let bitmap = escpos_raster::trim_leading_blank_rows(bitmap);
    let bitmap = escpos_raster::trim_trailing_blank_rows(bitmap, TRAILING_DOT_ROWS);
    Ok(escpos_raster::escpos_payload(&bitmap))
}

unsafe fn resize_surface(handles: &EngineHandles, width: u32, height: u32) -> Result<()> {
    // The webview cannot composite more than its host window, so grow both.
    SetWindowPos(
        handles.hwnd,
        None,
        OFFSCREEN_ORIGIN,
        OFFSCREEN_ORIGIN,
        width as i32,
        height as i32,
        SWP_NOACTIVATE | SWP_NOZORDER,
    )
    .context("SetWindowPos for capture surface")?;
    handles
        .controller
        .SetBounds(RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        })
        .context("SetBounds for capture surface")?;
    handles.controller.SetIsVisible(true)?;
    pump_for(Duration::from_millis(40));
    Ok(())
}

fn capture_png(webview: &ICoreWebView2) -> Result<Vec<u8>> {
    let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL(std::ptr::null_mut()), true) }
        .context("CreateStreamOnHGlobal for receipt capture")?;

    let (tx, rx) = mpsc::channel();
    unsafe {
        webview.CapturePreview(
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
            &stream,
            &CapturePreviewCompletedHandler::create(Box::new(move |error_code| {
                let result = (|| -> Result<()> {
                    error_code?;
                    Ok(())
                })();
                let _ = tx.send(result);
                Ok(())
            })),
        )?;
    }
    wait_with_pump_timeout(rx, CAPTURE_TIMEOUT)??;

    read_stream(&stream)
}

fn read_stream(stream: &IStream) -> Result<Vec<u8>> {
    let mut stat = STATSTG::default();
    unsafe { stream.Stat(&mut stat, STATFLAG_NONAME) }.context("Stat on capture stream")?;
    let len = stat.cbSize as usize;
    if len == 0 {
        bail!("receipt capture stream is empty");
    }

    unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }.context("Seek on capture stream")?;
    let mut buf = vec![0u8; len];
    let mut read: u32 = 0;
    unsafe {
        stream
            .Read(buf.as_mut_ptr() as *mut c_void, len as u32, Some(&mut read))
            .ok()
            .context("Read on capture stream")?;
    }
    buf.truncate(read as usize);
    if buf.is_empty() {
        bail!("receipt capture returned no bytes");
    }
    Ok(buf)
}

fn decode_png_to_mono(png: &[u8], width_dots: u32) -> Result<MonoBitmap> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .context("decode receipt capture png")?
        .into_luma8();
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        bail!("receipt capture is {width}x{height}");
    }
    Ok(escpos_raster::pack_luma(
        width,
        height,
        image.as_raw(),
        width_dots,
        escpos_raster::BLACK_THRESHOLD,
    ))
}

fn navigate_to_html(webview: &ICoreWebView2, html: &str) -> Result<()> {
    let html = CoTaskMemPWSTR::from(html);
    unsafe { webview.NavigateToString(*html.as_ref().as_pcwstr()) }.context("NavigateToString")
}

fn wait_navigation(webview: &ICoreWebView2, html: &str) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut token = 0i64;
    unsafe {
        webview.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                let result = (|| -> Result<()> {
                    let args = args.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
                    let mut success = BOOL::default();
                    args.IsSuccess(&mut success)?;
                    if !success.as_bool() {
                        bail!("html navigation failed");
                    }
                    Ok(())
                })();
                let _ = tx.send(result);
                Ok(())
            })),
            &mut token,
        )?;
    }

    let nav_result = navigate_to_html(webview, html);
    if let Err(err) = nav_result {
        let _ = unsafe { webview.remove_NavigationCompleted(token) };
        return Err(err);
    }

    let result = match wait_with_pump_timeout(rx, NAV_TIMEOUT) {
        Ok(result) => result,
        Err(err) => {
            let _ = unsafe { webview.remove_NavigationCompleted(token) };
            return Err(err);
        }
    };
    let _ = unsafe { webview.remove_NavigationCompleted(token) };
    result
}

fn silent_print(
    env: &ICoreWebView2Environment,
    webview: &ICoreWebView2,
    printer: &str,
    paper_mm: u32,
) -> Result<()> {
    let webview16: ICoreWebView2_16 = webview.cast().context(
        "WebView2 Print API (ICoreWebView2_16) is unavailable — update WebView2 Runtime",
    )?;

    let env6: ICoreWebView2Environment6 = env.cast().context(
        "WebView2 Print API (ICoreWebView2Environment6) is unavailable — update WebView2 Runtime",
    )?;

    let settings = unsafe { env6.CreatePrintSettings()? };
    let settings2: ICoreWebView2PrintSettings2 = settings
        .cast()
        .context("WebView2 printer settings (ICoreWebView2PrintSettings2) are unavailable")?;

    let width_in = (paper_mm as f64) / 25.4;
    // Chrome uses `@page { size: 80mm auto }`. WebView2 treats `auto` as ~0 and
    // then shrink-to-fits the receipt (looks like 0.001 font on 80mm). A 1" floor
    // does the same. Measure the real receipt; if that fails, use a tall roll.
    let (layout_mm, _) = escpos_raster::paper_geometry(paper_mm);
    let content_px = measure_content_height_px(webview, layout_mm).unwrap_or(0.0);
    let content_px = if content_px < 80.0 {
        tracing::warn!(
            "html print height {content_px}px is implausible; using 22in roll fallback"
        );
        22.0 * 96.0
    } else {
        content_px
    };
    // CSS @page already has 2–3mm. Extra WebView2 margins shrink the printable
    // area so the last rows spill onto page 2 and the cutter fires between pages.
    let cutter_in = 12.0 / 25.4;
    let page_height_in = ((content_px / 96.0) * 1.30 + cutter_in).min(80.0);
    let page_height_mm = page_height_in * 25.4;
    tracing::info!(
        paper_mm,
        content_px,
        page_height_in,
        "html print page size"
    );
    if let Err(err) = inject_page_size(webview, paper_mm, page_height_mm) {
        tracing::warn!("html print @page override failed: {err:#}");
    }

    unsafe {
        let printer_name = CoTaskMemPWSTR::from(printer);
        settings2.SetPrinterName(*printer_name.as_ref().as_pcwstr())?;
        let _ = settings2.SetMediaSize(COREWEBVIEW2_PRINT_MEDIA_SIZE_CUSTOM);
        let _ = settings2.SetCopies(1);
        if let Ok(base) = settings.cast::<ICoreWebView2PrintSettings>() {
            let _ = base.SetOrientation(COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT);
            let _ = base.SetScaleFactor(1.0);
            let _ = base.SetShouldPrintBackgrounds(true);
            let _ = base.SetShouldPrintHeaderAndFooter(false);
            let _ = base.SetMarginTop(0.0);
            let _ = base.SetMarginBottom(0.0);
            let _ = base.SetMarginLeft(0.0);
            let _ = base.SetMarginRight(0.0);
            let _ = base.SetPageWidth(width_in);
            let _ = base.SetPageHeight(page_height_in);
        }
    }

    let (tx, rx) = mpsc::channel();
    unsafe {
        webview16.Print(
            &settings,
            &PrintCompletedHandler::create(Box::new(move |error_code, status| {
                let result = (|| -> Result<()> {
                    error_code?;
                    match status {
                        COREWEBVIEW2_PRINT_STATUS_SUCCEEDED => Ok(()),
                        COREWEBVIEW2_PRINT_STATUS_PRINTER_UNAVAILABLE => {
                            bail!("Printer is unavailable.")
                        }
                        _ => bail!("HTML print failed"),
                    }
                })();
                let _ = tx.send(result);
                Ok(())
            })),
        )?;
    }

    wait_with_pump_timeout(rx, PRINT_TIMEOUT)?
}

fn wait_for_layout(webview: &ICoreWebView2) {
    let deadline = Instant::now() + Duration::from_millis(280);
    loop {
        let ready = execute_script(
            webview,
            r#"(function(){
  var imgs = document.images;
  if (!imgs.length) return 1;
  for (var i = 0; i < imgs.length; i++) {
    var src = imgs[i].currentSrc || imgs[i].src || "";
    if (src.indexOf("data:") === 0 && imgs[i].naturalWidth > 0) continue;
    if (!imgs[i].complete) return 0;
    if (imgs[i].naturalWidth === 0) return 0;
  }
  return 1;
})()"#,
        )
        .ok()
        .map(|json| json.trim().trim_matches('"') == "1")
        .unwrap_or(false);
        if ready || Instant::now() >= deadline {
            break;
        }
        pump_for(Duration::from_millis(16));
    }
    pump_for(Duration::from_millis(20));
}

fn inject_page_size(webview: &ICoreWebView2, paper_mm: u32, height_mm: f64) -> Result<()> {
    let js = format!(
        r#"(function(){{
  var old = document.getElementById('finvoroo-page-size');
  if (old) old.remove();
  var s = document.createElement('style');
  s.id = 'finvoroo-page-size';
  s.textContent = '@page {{ size: {paper_mm}mm {height_mm:.2}mm; margin: 0; }}'
    + 'html, body, #pos-receipt-print, .thermal-receipt-body {{'
    + 'page-break-inside: auto !important; break-inside: auto !important; }}'
    + 'html.print-thermal-receipt-only body, html.print-thermal-receipt-only body * {{'
    + 'visibility: visible !important; }}';
  document.head.appendChild(s);
  return 1;
}})()"#
    );
    execute_script(webview, &js)?;
    Ok(())
}

/// Force the receipt to lay out at the printable width and report how tall it is
/// in CSS pixels. `layout_mm` is the paper's printable width, not the roll width.
fn measure_content_height_px(webview: &ICoreWebView2, layout_mm: u32) -> Result<f64> {
    let js = format!(
        r#"(function(){{
  var mm = {layout_mm};
  var root = document.getElementById('pos-receipt-print') || document.body;
  var targets = [document.documentElement, document.body, root,
    document.querySelector('.thermal-print-source'),
    document.querySelector('.thermal-receipt-stage'),
    document.querySelector('.thermal-receipt-sheet'),
    document.querySelector('.thermal-receipt-body')];
  for (var t = 0; t < targets.length; t++) {{
    var el = targets[t];
    if (!el || !el.style) continue;
    el.style.setProperty('position','static','important');
    el.style.setProperty('left','auto','important');
    el.style.setProperty('top','auto','important');
    el.style.setProperty('margin','0','important');
    el.style.setProperty('transform','none','important');
    el.style.setProperty('visibility','visible','important');
    el.style.setProperty('display','block','important');
    // border-box keeps any padding inside the printable width. Under content-box
    // the padding is added to it, which drifts the receipt right and pushes the
    // overflow off the right-hand edge.
    el.style.setProperty('box-sizing','border-box','important');
    el.style.setProperty('width', mm + 'mm','important');
    el.style.setProperty('max-width', mm + 'mm','important');
    el.style.setProperty('overflow','visible','important');
    el.style.setProperty('background','#fff','important');
    if (el.classList && el.classList.contains('thermal-receipt-body')) {{
      el.style.setProperty('padding-top','0','important');
    }}
  }}
  // A scrollbar would narrow the layout and shift the capture, so suppress it.
  var noBars = document.getElementById('finvoroo-no-scrollbars');
  if (!noBars) {{
    noBars = document.createElement('style');
    noBars.id = 'finvoroo-no-scrollbars';
    noBars.textContent = 'html, body {{ overflow: hidden !important; }}'
      + '::-webkit-scrollbar {{ display: none !important; width: 0 !important; }}'
      // Nothing inside the receipt may be wider than the head, and no element may
      // be nudged sideways by a stray margin or transform.
      + '#pos-receipt-print, #pos-receipt-print * {{'
      + ' box-sizing: border-box !important;'
      + ' max-width: 100% !important;'
      + ' transform: none !important; }}';
    document.head.appendChild(noBars);
  }}
  void document.body.offsetHeight;
  var origin = root.getBoundingClientRect();
  var h = Math.max(root.scrollHeight || 0, root.offsetHeight || 0, origin.height || 0);
  var list = root.querySelectorAll('*');
  for (var i = 0; i < list.length; i++) {{
    var r = list[i].getBoundingClientRect();
    h = Math.max(h, r.bottom - origin.top, list[i].scrollHeight || 0, list[i].offsetHeight || 0);
  }}
  return Math.ceil(h);
}})()"#
    );

    let json = execute_script(webview, &js)?;
    let trimmed = json.trim().trim_matches('"');
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
        bail!("content height script returned {json}");
    }
    trimmed
        .parse::<f64>()
        .with_context(|| format!("content height json {json}"))
}

fn execute_script(webview: &ICoreWebView2, js: &str) -> Result<String> {
    let (tx, rx) = mpsc::channel();
    let js = CoTaskMemPWSTR::from(js);
    unsafe {
        webview.ExecuteScript(
            *js.as_ref().as_pcwstr(),
            &ExecuteScriptCompletedHandler::create(Box::new(move |error_code, result| {
                error_code?;
                let _ = tx.send(result);
                Ok(())
            })),
        )?;
    }
    wait_with_pump_timeout(rx, Duration::from_secs(10))
}

fn pump_for(dur: Duration) {
    let deadline = Instant::now() + dur;
    let mut msg = MSG::default();
    while Instant::now() < deadline {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}

unsafe fn create_hidden_window() -> Result<HWND> {
    let class_name = w!("FinvorooHtmlPrintEngine");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        lpszClassName: class_name,
        style: CS_HREDRAW | CS_VREDRAW,
        ..Default::default()
    };
    RegisterClassW(&wc);

    // Tool window, never activated, parked off every monitor: the cashier never
    // sees it, but it still composites so CapturePreview returns real pixels.
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        class_name,
        w!("Finvoroo HTML Print"),
        WS_POPUP,
        OFFSCREEN_ORIGIN,
        OFFSCREEN_ORIGIN,
        600,
        8000,
        None,
        None,
        None,
        None,
    )?;
    Ok(hwnd)
}

unsafe fn create_webview(hwnd: HWND) -> Result<EngineHandles> {
    let (env_tx, env_rx) = mpsc::channel();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(|handler| unsafe {
            CreateCoreWebView2Environment(&handler).map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, environment| {
            error_code?;
            env_tx
                .send(environment.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                .expect("html print env channel");
            Ok(())
        }),
    )
    .map_err(|e| anyhow::anyhow!("CreateCoreWebView2Environment: {e:?}"))?;
    let env = env_rx
        .recv()
        .context("CreateCoreWebView2Environment channel closed")??;

    let env_for_controller = env.clone();
    let (ctrl_tx, ctrl_rx) = mpsc::channel();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            env_for_controller
                .CreateCoreWebView2Controller(hwnd, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, controller| {
            error_code?;
            ctrl_tx
                .send(controller.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                .expect("html print controller channel");
            Ok(())
        }),
    )
    .map_err(|e| anyhow::anyhow!("CreateCoreWebView2Controller: {e:?}"))?;
    let controller = ctrl_rx
        .recv()
        .context("CreateCoreWebView2Controller channel closed")??;
    let webview = controller.CoreWebView2()?;

    Ok(EngineHandles {
        hwnd,
        env,
        controller,
        webview,
    })
}

/// Pump this thread's Win32 message loop until `rx` receives or `timeout` elapses.
/// WebView2 completions are posted as window messages; a blocking `recv` would deadlock.
fn wait_with_pump_timeout<T>(rx: mpsc::Receiver<T>, timeout: Duration) -> Result<T> {
    let deadline = Instant::now() + timeout;
    let mut msg = MSG::default();
    loop {
        if let Ok(value) = rx.try_recv() {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            bail!("html print operation timed out");
        }

        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                if let Ok(value) = rx.try_recv() {
                    return Ok(value);
                }
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
