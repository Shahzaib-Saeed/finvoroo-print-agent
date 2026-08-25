pub mod escpos_raster;

#[cfg(windows)]
#[path = "windows.rs"]
mod backend;

#[cfg(not(windows))]
#[path = "stub.rs"]
mod backend;

pub use backend::{init_html_engine, list_printers, print_job, test_print};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PrintOptions {
    #[serde(default)]
    pub paper_mm: Option<u32>,
    #[serde(default)]
    pub open_drawer: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrinterInfo {
    pub id: String,
    pub name: String,
    #[serde(rename = "systemName")]
    pub system_name: String,
    pub default: bool,
    #[serde(rename = "type")]
    pub printer_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrintRequest {
    pub printer_id: String,
    #[serde(rename = "type")]
    pub job_type: String,
    pub data: String,
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub options: Option<PrintOptions>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Pdf,
    Zpl,
    Raw,
    EscPos,
    Html,
}

impl JobKind {
    pub fn parse(value: &str) -> Result<Self, anyhow::Error> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pdf" => Ok(Self::Pdf),
            "zpl" => Ok(Self::Zpl),
            "raw" => Ok(Self::Raw),
            "escpos" | "esc/pos" | "esc-pos" => Ok(Self::EscPos),
            "html" => Ok(Self::Html),
            other => anyhow::bail!(
                "unsupported print type '{other}' (use pdf, html, zpl, escpos, or raw)"
            ),
        }
    }

    pub fn is_raw_spooler(self) -> bool {
        matches!(self, Self::Zpl | Self::Raw | Self::EscPos)
    }
}

pub fn decode_payload(data: &str, encoding: Option<&str>, kind: JobKind) -> Result<Vec<u8>, anyhow::Error> {
    let encoding = encoding.unwrap_or("").trim().to_ascii_lowercase();
    let trimmed = data.trim();
    let is_pdf_header = trimmed.starts_with("%PDF");
    let use_b64 = encoding == "base64"
        || (kind == JobKind::Pdf && !is_pdf_header && !trimmed.is_empty());

    if use_b64 {
        let cleaned: String = data.chars().filter(|c| !c.is_whitespace()).collect();
        let payload = cleaned
            .rsplit_once("base64,")
            .map(|(_, b)| b)
            .unwrap_or(&cleaned);
        return Ok(base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            payload,
        )?);
    }

    Ok(data.as_bytes().to_vec())
}

pub fn classify_printer(name: &str, driver: Option<&str>) -> String {
    let hay = format!("{} {}", name, driver.unwrap_or("")).to_ascii_lowercase();
    if hay.contains("zebra")
        || hay.contains("zdesigner")
        || hay.contains("gk420")
        || hay.contains("zd4")
        || hay.contains("zt4")
        || hay.contains("gc420")
    {
        return "zebra".into();
    }
    if hay.contains("epson")
        || hay.contains("star ")
        || hay.contains("tm-t")
        || hay.contains("escpos")
        || hay.contains("thermal")
        || hay.contains("receipt")
        || hay.contains("pos-")
        || hay.contains("pos 80")
        || hay.contains("pos80")
        || hay.contains("pos 58")
        || hay.contains("pos58")
        || hay.contains("bixolon")
        || hay.contains("bc-95")
        || hay.contains("bc95")
        || hay.contains("xprinter")
        || hay.contains("citizen")
    {
        return "thermal".into();
    }
    "windows".into()
}

pub fn zebra_test_zpl() -> &'static str {
    "^XA\r\n^MMT\r\n^PW812\r\n^LL400\r\n^FO40,40^A0N,48,48^FDFinvoroo Print Agent^FS\r\n^FO40,110^A0N,28,28^FDSilent test print^FS\r\n^FO40,160^A0N,24,24^FDZebra / ZPL path OK^FS\r\n^XZ\r\n"
}

/// ESC/POS test for thermal receipt printers (Bixolon BC-95AC, Epson TM, etc.).
pub fn thermal_test_escpos() -> &'static [u8] {
    b"\x1b@\x1ba\x01\n\n*** Finvoroo ***\nPrint Agent test\n\nTest print OK\n\n\n\n\n\n\x1b\x64\x04\x1dVA\x00"
}

/// One-page A4 PDF used for test prints on Windows GDI printers.
pub fn build_test_pdf() -> Vec<u8> {
    MINIMAL_TEST_PDF.to_vec()
}

const MINIMAL_TEST_PDF: &[u8] = b"%PDF-1.4
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj
2 0 obj<</Type/Pages/Count 1/Kids[3 0 R]>>endobj
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>endobj
4 0 obj<</Length 128>>stream
BT /F1 24 Tf 72 720 Td (Finvoroo Print Agent) Tj ET
BT /F1 12 Tf 72 680 Td (Silent test print - no browser dialog.) Tj ET
endstream
endobj
5 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj
xref
0 6
0000000000 65535 f 
0000000009 00000 n 
0000000058 00000 n 
0000000115 00000 n 
0000000266 00000 n 
0000000446 00000 n 
trailer<</Size 6/Root 1 0 R>>
startxref
523
%%EOF
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_kind_aliases() {
        assert_eq!(JobKind::parse("pdf").unwrap(), JobKind::Pdf);
        assert_eq!(JobKind::parse("ZPL").unwrap(), JobKind::Zpl);
        assert_eq!(JobKind::parse("raw").unwrap(), JobKind::Raw);
        assert_eq!(JobKind::parse("escpos").unwrap(), JobKind::EscPos);
        assert_eq!(JobKind::parse("esc/pos").unwrap(), JobKind::EscPos);
        assert_eq!(JobKind::parse("html").unwrap(), JobKind::Html);
        assert!(JobKind::parse("shell").is_err());
        assert!(JobKind::EscPos.is_raw_spooler());
        assert!(!JobKind::Pdf.is_raw_spooler());
    }

    #[test]
    fn decode_plain_zpl() {
        let bytes = decode_payload("^XA^XZ", Some("plain"), JobKind::Zpl).unwrap();
        assert_eq!(bytes, b"^XA^XZ");
    }

    #[test]
    fn decode_base64_escpos() {
        let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"\x1b@");
        let bytes = decode_payload(&encoded, Some("base64"), JobKind::EscPos).unwrap();
        assert_eq!(bytes, b"\x1b@");
    }

    #[test]
    fn classify_does_not_hardcode_only_gk420d() {
        assert_eq!(classify_printer("HP LaserJet", None), "windows");
        assert_eq!(classify_printer("Zebra ZD421", Some("ZDesigner")), "zebra");
        assert_eq!(classify_printer("EPSON TM-T20", None), "thermal");
        assert_eq!(classify_printer("BC-95AC", Some("BIXOLON")), "thermal");
    }
}
