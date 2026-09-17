//! macOS printing through CUPS.
//!
//! Mirrors the Windows backend's public surface exactly — `list_printers`,
//! `print_job`, `test_print`, `init_html_engine`, `prewarm_html_engine` — so
//! `print::mod` re-exports the same API and the HTTP contract the React app
//! talks to is identical on every platform.
//!
//! Raw jobs (ESC/POS, ZPL, Raw) go out through `lp -o raw`, which hands the
//! bytes to the device backend untouched. That matters: the normal macOS path
//! would rasterise the payload as an A4 document and a thermal head would
//! print the escape codes as text, or nothing at all.
//!
//! Discovery reads configured CUPS queues, so any printer the user has added
//! in System Settings shows up — nothing here is specific to one model.

#![cfg(target_os = "macos")]

use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use super::{
    build_test_pdf, classify_printer, decode_payload, thermal_test_escpos, zebra_test_zpl, JobKind,
    PrintRequest, PrinterInfo,
};

/// ESC p 0 25 250 — same pulse the Windows backend sends.
const ESCPOS_OPEN_DRAWER: &[u8] = &[0x1b, 0x70, 0x00, 0x19, 0xfa];

/// HTML rendering is WebView2-based and Windows-only; there is no engine to
/// start here. Returning Ok keeps startup identical across platforms.
pub fn init_html_engine() -> Result<()> {
    Ok(())
}

pub fn prewarm_html_engine(_paper_mm: u32) -> Result<()> {
    Ok(())
}

pub fn list_printers() -> Result<Vec<PrinterInfo>> {
    let devices = cups_devices()?;
    if devices.is_empty() {
        return Ok(Vec::new());
    }

    let default_name = cups_default_printer();

    let mut out = Vec::with_capacity(devices.len());
    for (queue, uri) in devices {
        let (description, model) = cups_printer_details(&queue);

        // classify_printer() is shared with Windows and unchanged. Feeding it
        // the make-and-model when CUPS knows it, and the device URI otherwise,
        // is what lets `usb://Xprinter/...` resolve to "thermal".
        let driver = match (model.as_deref(), uri.as_str()) {
            (Some(m), _) if !m.trim().is_empty() => Some(m.to_string()),
            (_, u) if !u.is_empty() => Some(u.to_string()),
            _ => None,
        };
        let classify_hint = format!("{queue} {uri}");
        let printer_type = classify_printer(&classify_hint, driver.as_deref());

        out.push(PrinterInfo {
            id: queue.clone(),
            name: description.unwrap_or_else(|| queue.clone()),
            system_name: queue.clone(),
            default: !default_name.is_empty() && queue.eq_ignore_ascii_case(&default_name),
            printer_type,
            driver,
        });
    }

    Ok(out)
}

pub fn print_job(req: &PrintRequest) -> Result<()> {
    if req.printer_id.trim().is_empty() {
        bail!("printer_id is required");
    }
    if req.data.trim().is_empty() {
        bail!("print data is empty");
    }

    let kind = JobKind::parse(&req.job_type)?;
    if kind == JobKind::Html {
        bail!(
            "HTML printing needs the WebView2 renderer, which exists only on Windows. \
             Send the receipt as 'escpos' (or 'pdf' for a document printer) on macOS."
        );
    }

    let bytes = decode_payload(&req.data, req.encoding.as_deref(), kind)?;
    match kind {
        JobKind::Zpl | JobKind::Raw | JobKind::EscPos => {
            print_raw(&req.printer_id, &bytes)?;
            // A drawer only exists on a receipt printer, so the pulse rides the
            // raw path — the same place the Windows backend fires it from.
            if req
                .options
                .as_ref()
                .and_then(|o| o.open_drawer)
                .unwrap_or(false)
            {
                open_cash_drawer(&req.printer_id)?;
            }
            Ok(())
        }
        JobKind::Pdf => print_pdf(&req.printer_id, &bytes),
        JobKind::Html => unreachable!("handled above"),
    }
}

pub fn test_print(printer_id: &str) -> Result<()> {
    let id = printer_id.trim();
    if id.is_empty() {
        bail!("No printer selected");
    }

    let printers = list_printers()?;
    let known = printers.iter().find(|p| p.id.eq_ignore_ascii_case(id));
    if known.is_none() {
        bail!(
            "'{id}' is not a CUPS printer on this Mac. Add it in System Settings → \
             Printers & Scanners first — a USB device that only shows in `lpinfo -v` \
             has no queue to print to yet."
        );
    }

    // Same branching as Windows so a test print looks identical on both.
    let kind = known.map(|p| p.printer_type.as_str()).unwrap_or("windows");
    if kind == "zebra" {
        return print_raw(id, zebra_test_zpl().as_bytes());
    }
    if kind == "thermal" {
        return print_raw(id, thermal_test_escpos());
    }
    print_pdf(id, &build_test_pdf())
}

/// Bytes straight to the device backend — no filters, no page setup.
pub fn print_raw(printer: &str, payload: &[u8]) -> Result<()> {
    lp_submit(printer, payload, true)
}

/// A PDF is a document, so CUPS may filter it for the destination as usual.
pub fn print_pdf(printer: &str, pdf: &[u8]) -> Result<()> {
    lp_submit(printer, pdf, false)
}

fn open_cash_drawer(printer: &str) -> Result<()> {
    print_raw(printer, ESCPOS_OPEN_DRAWER)
}

/// Hand a payload to `lp` on stdin.
///
/// `raw` adds `-o raw`, which tells CUPS to skip every filter and pass the
/// bytes through untouched. Without it a thermal printer receives a rendered
/// page instead of ESC/POS.
fn lp_submit(printer: &str, payload: &[u8], raw: bool) -> Result<()> {
    if payload.is_empty() {
        bail!("nothing to print");
    }

    let mut cmd = Command::new("lp");
    cmd.arg("-d").arg(printer).arg("-t").arg("Finvoroo");
    if raw {
        cmd.arg("-o").arg("raw");
    }

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not run `lp` — is CUPS available on this Mac?")?;

    child
        .stdin
        .as_mut()
        .context("failed to open stdin for `lp`")?
        .write_all(payload)
        .context("failed to send the job to `lp`")?;

    let output = child.wait_with_output().context("`lp` did not finish")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if err.is_empty() {
            format!("lp exited with {}", output.status)
        } else {
            err
        };
        bail!("Printing to '{printer}' failed: {detail}");
    }

    Ok(())
}

/// Configured CUPS queues as `(queue name, device uri)`.
///
/// `lpstat -v` prints one line per queue:
///   `device for BC-96AC: usb://Xprinter/USB%20Printer%20P?location=14110000`
///
/// Note this lists queues, not raw USB devices. A printer that only appears in
/// `lpinfo -v` has been detected but not added, and cannot be printed to yet.
fn cups_devices() -> Result<Vec<(String, String)>> {
    let output = Command::new("lpstat")
        .arg("-v")
        .output()
        .context("could not run `lpstat` — is CUPS available on this Mac?")?;

    // A Mac with no printers added exits non-zero with "No destinations added".
    // That is an empty list, not a failure.
    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().filter_map(parse_lpstat_device_line).collect())
}

/// `device for NAME: URI` → `(NAME, URI)`.
fn parse_lpstat_device_line(line: &str) -> Option<(String, String)> {
    let rest = line.trim().strip_prefix("device for ")?;
    let (name, uri) = rest.split_once(':')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some((name.to_string(), uri.trim().to_string()))
}

fn cups_default_printer() -> String {
    let Ok(output) = Command::new("lpstat").arg("-d").output() else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(parse_lpstat_default_line)
        .unwrap_or_default()
}

/// `system default destination: NAME` → `NAME`.
/// "no system default destination" yields nothing.
fn parse_lpstat_default_line(line: &str) -> Option<String> {
    let (_, name) = line.trim().split_once("default destination:")?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}

/// `(description, make-and-model)` for a queue, both best-effort.
fn cups_printer_details(queue: &str) -> (Option<String>, Option<String>) {
    let Ok(output) = Command::new("lpoptions").arg("-p").arg(queue).output() else {
        return (None, None);
    };
    if !output.status.success() {
        return (None, None);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    (
        lpoptions_value(&stdout, "printer-info"),
        lpoptions_value(&stdout, "printer-make-and-model"),
    )
}

/// Pull `key=value` out of an `lpoptions` line, honouring single quotes around
/// values that contain spaces (`printer-make-and-model='Xprinter USB'`).
fn lpoptions_value(haystack: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let start = haystack.find(&needle)? + needle.len();
    let rest = &haystack[start..];

    let value = if let Some(stripped) = rest.strip_prefix('\'') {
        stripped.split('\'').next().unwrap_or("")
    } else {
        rest.split_whitespace().next().unwrap_or("")
    };

    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_usb_thermal_queue() {
        let (name, uri) = parse_lpstat_device_line(
            "device for BC-96AC: usb://Xprinter/USB%20Printer%20P?location=14110000",
        )
        .unwrap();
        assert_eq!(name, "BC-96AC");
        assert_eq!(uri, "usb://Xprinter/USB%20Printer%20P?location=14110000");
    }

    #[test]
    fn parses_other_backends_too() {
        // Discovery must not be specific to one printer or one connection type.
        let (name, uri) =
            parse_lpstat_device_line("device for Office_HP: ipp://192.168.1.50/ipp/print").unwrap();
        assert_eq!(name, "Office_HP");
        assert_eq!(uri, "ipp://192.168.1.50/ipp/print");

        let (name, _) = parse_lpstat_device_line("device for Zebra_ZD421: usb://Zebra/ZD421")
            .unwrap();
        assert_eq!(name, "Zebra_ZD421");
    }

    #[test]
    fn ignores_lines_that_are_not_devices() {
        assert!(parse_lpstat_device_line("printer BC-96AC is idle.").is_none());
        assert!(parse_lpstat_device_line("").is_none());
        assert!(parse_lpstat_device_line("device for : usb://x").is_none());
    }

    #[test]
    fn reads_the_default_destination() {
        assert_eq!(
            parse_lpstat_default_line("system default destination: BC-96AC").unwrap(),
            "BC-96AC"
        );
        assert!(parse_lpstat_default_line("no system default destination").is_none());
    }

    #[test]
    fn reads_quoted_and_bare_lpoptions_values() {
        let line = "copies=1 device-uri=usb://Xprinter/USB%20Printer%20P \
                    printer-info='Black Copper BC-96AC' printer-is-shared=false \
                    printer-make-and-model='Xprinter USB Printer P'";
        assert_eq!(
            lpoptions_value(line, "printer-info").unwrap(),
            "Black Copper BC-96AC"
        );
        assert_eq!(
            lpoptions_value(line, "printer-make-and-model").unwrap(),
            "Xprinter USB Printer P"
        );
        assert_eq!(lpoptions_value(line, "copies").unwrap(), "1");
        assert!(lpoptions_value(line, "not-present").is_none());
    }

    #[test]
    fn the_usb_thermal_printer_classifies_as_thermal() {
        // Shared classify_printer(), unchanged — the URI carries "Xprinter".
        let hint = "BC-96AC usb://Xprinter/USB%20Printer%20P?location=14110000";
        assert_eq!(classify_printer(hint, None), "thermal");
        assert_eq!(
            classify_printer("BC-96AC", Some("Xprinter USB Printer P")),
            "thermal"
        );
    }

    #[test]
    fn raw_job_kinds_still_take_the_raw_path() {
        assert!(JobKind::EscPos.is_raw_spooler());
        assert!(JobKind::Raw.is_raw_spooler());
        assert!(JobKind::Zpl.is_raw_spooler());
        assert!(!JobKind::Pdf.is_raw_spooler());
    }
}
