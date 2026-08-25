//! Windows spooler: RAW/ZPL via WritePrinter, PDF via the printer's driver (printto).
//! No browser is involved.

#![cfg(windows)]

use std::ffi::c_void;
use std::fs;
use std::path::PathBuf;
use std::ptr;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Printing::{
    ClosePrinter, EndDocPrinter, EndPagePrinter, EnumPrintersW, GetDefaultPrinterW, OpenPrinterW,
    StartDocPrinterW, StartPagePrinter, WritePrinter, DOC_INFO_1W, PRINTER_ENUM_CONNECTIONS,
    PRINTER_ENUM_LOCAL, PRINTER_INFO_2W,
};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

use super::{
    build_test_pdf, classify_printer, decode_payload, zebra_test_zpl, JobKind, PrintRequest,
    PrinterInfo,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn list_printers() -> Result<Vec<PrinterInfo>> {
    unsafe { list_printers_inner() }
}

unsafe fn list_printers_inner() -> Result<Vec<PrinterInfo>> {
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let mut needed: u32 = 0;
    let mut returned: u32 = 0;
    let _ = EnumPrintersW(flags, PCWSTR::null(), 2, None, &mut needed, &mut returned);
    if needed == 0 {
        return Ok(Vec::new());
    }

    let mut buffer = vec![0u8; needed as usize];
    EnumPrintersW(
        flags,
        PCWSTR::null(),
        2,
        Some(&mut buffer),
        &mut needed,
        &mut returned,
    )
    .context("EnumPrintersW")?;

    let default_name = default_printer_name().unwrap_or_default();
    let info_size = std::mem::size_of::<PRINTER_INFO_2W>();
    let count = returned as usize;
    let mut out = Vec::with_capacity(count);

    for i in 0..count {
        let info = &*(buffer.as_ptr().add(i * info_size) as *const PRINTER_INFO_2W);
        let name = pwstr_to_string(info.pPrinterName);
        if name.is_empty() {
            continue;
        }
        let driver = pwstr_to_string(info.pDriverName);
        let driver_opt = if driver.is_empty() { None } else { Some(driver) };
        let printer_type = classify_printer(&name, driver_opt.as_deref());
        out.push(PrinterInfo {
            id: name.clone(),
            name: name.clone(),
            system_name: name.clone(),
            default: !default_name.is_empty() && name.eq_ignore_ascii_case(&default_name),
            printer_type,
            driver: driver_opt,
        });
    }

    Ok(out)
}

fn pwstr_to_string(ptr: PWSTR) -> String {
    if ptr.0.is_null() {
        return String::new();
    }
    unsafe { ptr.to_string().unwrap_or_default() }
}

fn default_printer_name() -> Result<String> {
    unsafe {
        let mut size: u32 = 0;
        let _ = GetDefaultPrinterW(PWSTR(ptr::null_mut()), &mut size);
        if size == 0 {
            return Ok(String::new());
        }
        let mut buf = vec![0u16; size as usize];
        if !GetDefaultPrinterW(PWSTR(buf.as_mut_ptr()), &mut size).as_bool() {
            anyhow::bail!("GetDefaultPrinterW failed");
        }
        Ok(String::from_utf16_lossy(&buf[..buf.len().saturating_sub(1)]).trim_end_matches('\0').to_string())
    }
}

pub fn print_job(req: &PrintRequest) -> Result<()> {
    if req.printer_id.trim().is_empty() {
        bail!("printer_id is required");
    }
    if req.data.trim().is_empty() {
        bail!("print data is empty");
    }
    let kind = JobKind::parse(&req.job_type)?;
    let bytes = decode_payload(&req.data, req.encoding.as_deref(), kind)?;
    match kind {
        JobKind::Zpl | JobKind::Raw | JobKind::EscPos => print_raw(&req.printer_id, &bytes),
        JobKind::Pdf => print_pdf(&req.printer_id, &bytes),
    }
}

pub fn test_print(printer_id: &str) -> Result<()> {
    let id = printer_id.trim();
    if id.is_empty() {
        bail!("No printer selected");
    }
    let printers = list_printers()?;
    let kind = printers
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(id))
        .map(|p| p.printer_type.as_str())
        .unwrap_or("windows");

    if kind == "zebra" {
        return print_raw(id, zebra_test_zpl().as_bytes());
    }
    print_pdf(id, &build_test_pdf())
}

pub fn print_raw(printer: &str, payload: &[u8]) -> Result<()> {
    if payload.is_empty() {
        bail!("empty raw payload");
    }
    unsafe { print_raw_inner(printer, payload) }
}

unsafe fn print_raw_inner(printer: &str, payload: &[u8]) -> Result<()> {
    let mut name = wide(printer);
    let mut handle = HANDLE::default();
    OpenPrinterW(PCWSTR(name.as_mut_ptr()), &mut handle, None).map_err(|_| {
        anyhow::anyhow!("Printer \"{}\" is unavailable.", printer)
    })?;

    let result = (|| {
        let mut doc_name = wide("Finvoroo Print Agent");
        let mut datatype = wide("RAW");
        let doc = DOC_INFO_1W {
            pDocName: PWSTR(doc_name.as_mut_ptr()),
            pOutputFile: PWSTR(ptr::null_mut()),
            pDatatype: PWSTR(datatype.as_mut_ptr()),
        };
        let job_id = StartDocPrinterW(handle, 1, &doc as *const DOC_INFO_1W as *const _);
        if job_id == 0 {
            bail!("StartDocPrinterW failed");
        }
        if !StartPagePrinter(handle).as_bool() {
            let _ = EndDocPrinter(handle);
            bail!("StartPagePrinter failed");
        }

        let mut written: u32 = 0;
        let ok = WritePrinter(
            handle,
            payload.as_ptr() as *const c_void,
            payload.len() as u32,
            &mut written,
        );
        let _ = EndPagePrinter(handle);
        let _ = EndDocPrinter(handle);
        if !ok.as_bool() || written == 0 {
            bail!("WritePrinter failed (wrote {written} of {} bytes)", payload.len());
        }
        Ok(())
    })();

    let _ = ClosePrinter(handle);
    result
}

pub fn print_pdf(printer: &str, pdf: &[u8]) -> Result<()> {
    if pdf.len() < 8 || !pdf.starts_with(b"%PDF") {
        bail!("data is not a PDF document");
    }
    let path = write_temp_pdf(pdf)?;
    let result = print_pdf_via_shell(printer, &path);
    let _ = fs::remove_file(&path);
    result
}

fn write_temp_pdf(pdf: &[u8]) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("finvoroo-print-agent");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("job-{}.pdf", std::process::id()));
    fs::write(&path, pdf).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn print_pdf_via_shell(printer: &str, path: &PathBuf) -> Result<()> {
    unsafe { print_pdf_via_shell_inner(printer, path) }
}

unsafe fn print_pdf_via_shell_inner(printer: &str, path: &PathBuf) -> Result<()> {
    let mut file = wide(&path.to_string_lossy());
    let mut verb = wide("printto");
    let mut params = wide(&format!("\"{}\"", printer));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
        hwnd: Default::default(),
        lpVerb: PCWSTR(verb.as_mut_ptr()),
        lpFile: PCWSTR(file.as_mut_ptr()),
        lpParameters: PCWSTR(params.as_mut_ptr()),
        lpDirectory: PCWSTR::null(),
        nShow: SW_HIDE.0 as i32,
        hInstApp: Default::default(),
        lpIDList: ptr::null_mut(),
        lpClass: PCWSTR::null(),
        hkeyClass: Default::default(),
        dwHotKey: 0,
        Anonymous: Default::default(),
        hProcess: HANDLE::default(),
    };

    ShellExecuteExW(&mut info).map_err(|_| {
        anyhow::anyhow!("Printer \"{}\" is unavailable.", printer)
    })?;
    if info.hProcess.is_invalid() {
        return Ok(());
    }
    let wait = WaitForSingleObject(info.hProcess, 60_000);
    let _ = CloseHandle(info.hProcess);
    if wait != WAIT_OBJECT_0 {
        tracing::warn!("printto did not finish within 60s; job may still be in the spooler");
    }
    let _ = Duration::from_millis(0);
    Ok(())
}
