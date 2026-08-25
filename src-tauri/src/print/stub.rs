//! Non-Windows compile target: HTTP API and settings still run; printing is Windows-only.

use anyhow::bail;

use super::{JobKind, PrintRequest, PrinterInfo};

pub fn list_printers() -> anyhow::Result<Vec<PrinterInfo>> {
    Ok(Vec::new())
}

pub fn init_html_engine() -> anyhow::Result<()> {
    Ok(())
}

pub fn print_job(req: &PrintRequest) -> anyhow::Result<()> {
    let _ = JobKind::parse(&req.job_type)?;
    bail!(
        "Finvoroo Print Agent printing is implemented for Windows. This computer is running {}.",
        std::env::consts::OS
    )
}

pub fn test_print(_printer_id: &str) -> anyhow::Result<()> {
    bail!("Test print requires Windows and an installed printer driver.")
}
