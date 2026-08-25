//! Persistent WebView2 renderer for silent HTML thermal receipt printing.

#![cfg(windows)]

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use once_cell::sync::OnceCell;
use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    NavigationCompletedEventHandler, PrintCompletedHandler, CoreWebView2EnvironmentOptions,
    Microsoft::Web::WebView2::Win32::*,
};
use windows::core::{Interface, PCWSTR, HSTRING};
use windows::Win32::Foundation::{E_POINTER, E_UNEXPECTED, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, ShowWindow, CS_HREDRAW, CS_VREDRAW,
    SW_HIDE, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

struct HtmlPrintJob {
    printer: String,
    html: String,
    paper_mm: u32,
    reply: mpsc::Sender<Result<()>>,
}

struct EngineHandles {
    env: ICoreWebView2Environment,
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

static JOB_TX: OnceCell<mpsc::Sender<HtmlPrintJob>> = OnceCell::new();

const ENGINE_CLASS: &str = "FinvorooHtmlPrintEngine";
const NAV_TIMEOUT: Duration = Duration::from_secs(30);
const PRINT_TIMEOUT: Duration = Duration::from_secs(60);

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

pub fn print_html(printer: &str, html: &str, paper_mm: u32) -> Result<()> {
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
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    let hwnd = unsafe { create_hidden_window()? };
    let handles = unsafe { create_webview(hwnd)? };
    unsafe {
        handles.controller.SetIsVisible(false)?;
        ShowWindow(hwnd, SW_HIDE);
    }

    // Warm the renderer once so the first receipt is faster.
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
    wait_navigation(webview, || unsafe {
        webview
            .NavigateToString(&HSTRING::from(
                "<!DOCTYPE html><html><body></body></html>",
            ))
            .context("warm-up NavigateToString")
    })
}

fn render_and_print(handles: &EngineHandles, printer: &str, html: &str, paper_mm: u32) -> Result<()> {
    if html.is_empty() {
        return Ok(());
    }

    wait_navigation(&handles.webview, || unsafe {
        handles
            .webview
            .NavigateToString(&HSTRING::from(html))
            .context("NavigateToString")
    })?;

    // Allow layout/paint to settle before silent print.
    thread::sleep(Duration::from_millis(80));

    silent_print(&handles.env, &handles.webview, printer, paper_mm)
}

fn wait_navigation<F>(webview: &ICoreWebView2, navigate: F) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    let (tx, rx) = mpsc::channel();
    let mut token = EventRegistrationToken::default();
    unsafe {
        webview.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                let result = (|| {
                    let args = args.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
                    let mut success = false;
                    args.IsSuccess(&mut success)?;
                    if !success {
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

    navigate()?;

    match rx.recv_timeout(NAV_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => bail!("html navigation timed out"),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            bail!("html navigation handler disconnected")
        }
    }
}

fn silent_print(
    env: &ICoreWebView2Environment,
    webview: &ICoreWebView2,
    printer: &str,
    paper_mm: u32,
) -> Result<()> {
    let webview16: ICoreWebView2_16 = webview
        .cast()
        .context("WebView2 Print API (ICoreWebView2_16) is unavailable — update WebView2 Runtime")?;

    let env6: ICoreWebView2Environment6 = env
        .cast()
        .context("WebView2 Print API (ICoreWebView2Environment6) is unavailable — update WebView2 Runtime")?;

    let settings = unsafe { env6.CreatePrintSettings()? };
    let settings2: ICoreWebView2PrintSettings2 = settings
        .cast()
        .context("WebView2 printer settings (ICoreWebView2PrintSettings2) are unavailable")?;

    unsafe {
        settings2.SetPrinterName(&HSTRING::from(printer))?;
        if let Ok(base) = settings.cast::<ICoreWebView2PrintSettings>() {
            base.SetShouldPrintBackground(true)?;
            base.SetShouldPrintHeaderAndFooter(false)?;
            let margin_in = if paper_mm <= 58 { 0.08 } else { 0.12 };
            let _ = base.SetMarginTop(margin_in);
            let _ = base.SetMarginBottom(margin_in);
            let _ = base.SetMarginLeft(margin_in);
            let _ = base.SetMarginRight(margin_in);
        }
    }

    let (tx, rx) = mpsc::channel();
    unsafe {
        webview16.Print(
            &settings,
            &PrintCompletedHandler::create(Box::new(move |error_code, status| {
                let result = (|| {
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

    match rx.recv_timeout(PRINT_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => bail!("html print timed out"),
        Err(mpsc::RecvTimeoutError::Disconnected) => bail!("html print handler disconnected"),
    }
}

unsafe fn create_hidden_window() -> Result<HWND> {
    let class_name = wide(ENGINE_CLASS);
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        style: CS_HREDRAW | CS_VREDRAW,
        ..Default::default()
    };
    RegisterClassW(&wc);

    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        PCWSTR(class_name.as_ptr()),
        PCWSTR(wide("Finvoroo HTML Print").as_ptr()),
        WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0),
        -20000,
        -20000,
        400,
        800,
        None,
        None,
        None,
        None,
    )?;
    Ok(hwnd)
}

unsafe fn create_webview(hwnd: HWND) -> Result<EngineHandles> {
    let (env_tx, env_rx) = mpsc::channel();
    let options = CoreWebView2EnvironmentOptions::default();
    CreateCoreWebView2EnvironmentWithOptions(
        PCWSTR::null(),
        &HSTRING::from(""),
        &ICoreWebView2EnvironmentOptions::from(options),
        &CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |error_code, environment| {
                let result = (|| {
                    error_code?;
                    environment.ok_or_else(|| windows::core::Error::from(E_POINTER))
                })();
                env_tx
                    .send(result)
                    .map_err(|_| windows::core::Error::from(E_UNEXPECTED))
            },
        )),
    )?;
    let env = webview2_com::wait_with_pump(env_rx).context("CreateCoreWebView2Environment")?;

    let (ctrl_tx, ctrl_rx) = mpsc::channel();
    let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
        move |error_code, controller| {
            let result = (|| {
                error_code?;
                controller.ok_or_else(|| windows::core::Error::from(E_POINTER))
            })();
            ctrl_tx
                .send(result)
                .map_err(|_| windows::core::Error::from(E_UNEXPECTED))
        },
    ));
    env.CreateCoreWebView2Controller(hwnd, &handler)?;
    let controller = webview2_com::wait_with_pump(ctrl_rx).context("CreateCoreWebView2Controller")?;
    let webview = unsafe { controller.CoreWebView2()? };

    Ok(EngineHandles {
        env,
        controller,
        webview,
    })
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
