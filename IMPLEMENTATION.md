# Finvoroo Print Agent — implementation notes

Do **not** create a second print agent. This folder is the QZ Tray replacement.

```text
Finvoroo React Frontend
        ↓  HTTP 127.0.0.1:17392
Finvoroo Print Agent (Tauri tray, installed on the till PC)
        ↓
Windows printer (USB, network, or local)
        ↓
Receipt / Invoice / Label
```

Laravel checkout, FEFO, and journals are not involved. The agent prints PDF, HTML thermal receipts, ESC/POS, and ZPL. Do **not** install the agent on the Laravel server.

## Client vs developer

**Pharmacy / till PC:** run `FinvorooPrintAgent-Setup.exe` only. See [CLIENT.md](CLIENT.md) and the client section of [README.md](README.md).

**Developer / build machine:** `npm install` then `npm run build` on Windows to produce that `.exe`. Those commands are never for clients.

## Bind and security

- Listen **only** on `127.0.0.1:17392`
- CORS / Origin allowlist: `localhost`, `127.0.0.1`, `::1` (http), `https://finvoroo.com`, `https://*.finvoroo.com`
- Long-lived token is created on first launch and stored in `%APPDATA%\com.finvoroo.print-agent\config.json`
- Token is **never** returned from `GET /status`
- `POST /pair` returns the token only after a 60-second PIN shown in the agent window
- Print jobs require `X-Finvoroo-Print-Token` (or `Authorization: Bearer`)
- Body cap 32 MB; print types are `pdf | html | zpl | escpos | raw`
- The agent never executes shell commands from request payloads
- Print errors append to the app log dir (`print-agent.log`)

## HTTP API

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| GET | `/status` | public | Detect if the agent is running |
| GET | `/printers` | token | OS printer list (`name`, `systemName`, `type`) |
| POST | `/pair` | PIN + allowed origin | One-time pairing; returns install token |
| POST | `/print` | token | Silent print |
| GET | `/settings` | token | Port, default printer, paired origin (no token) |

### POST /pair

```json
{ "code": "123456", "origin": "http://localhost:5173", "workstation_id": "till-1" }
```

### POST /print

```json
{
  "printer_id": "POS 80",
  "type": "pdf|html|escpos|zpl|raw",
  "data": "...",
  "encoding": "base64|plain",
  "options": { "paper_mm": 58, "open_drawer": true }
}
```

- **`html`** — full HTML document from `buildThermalHtmlDocument()` (ThermalReceiptBody CSS). Rendered silently by a persistent WebView2 engine at tray startup; uses `ICoreWebView2::Print()` to the named printer (no Chrome dialog).
- **`options.paper_mm`** — `58` or `80` for thermal receipts (optional, default `80`). Margins match Chrome `@page` (2 mm / 3 mm).
- **`options.open_drawer`** — after a successful `html` print, sends a tiny ESC/POS drawer pulse on the same printer (optional, default `false`).
- **`pdf` / `zpl` / `escpos` / `raw`** — unchanged from prior releases.

Example HTML receipt curl (after pairing):

```bat
curl -s -X POST http://127.0.0.1:17392/print ^
  -H "Content-Type: application/json" ^
  -H "X-Finvoroo-Print-Token: YOUR_TOKEN" ^
  -d "{\"printer_id\":\"POS 80\",\"type\":\"html\",\"encoding\":\"plain\",\"data\":\"<!DOCTYPE html><html>...</html>\",\"options\":{\"paper_mm\":80,\"open_drawer\":true}}"
```

## Windows installer

NSIS current-user install (`installMode: currentUser`):

- Start Menu folder: **Finvoroo**
- Desktop shortcut from the installer finish page (created automatically on silent install)
- Post-install launches the tray agent (`src-tauri/windows/installer-hooks.nsh`)
- WebView2 bootstrapper is embedded (`embedBootstrapper`, silent)
- AppData pairing is **not** deleted on update or uninstall

Output name after `npm run build`:

```text
src-tauri/target/release/bundle/nsis/FinvorooPrintAgent-Setup.exe
```

## React driver flag

`localStorage.finvoroo.print_driver`:

- `finvoroo-agent` — silent agent (no Chrome dialog)
- `qz` — legacy QZ Tray (kept until production soak)
- `browser` — default for tills that never enabled the agent (`window.print`)

Legacy `finvoroo.print_agent.enabled=true` + saved token maps to `finvoroo-agent`.

Workstation printers (localStorage, never hard-coded names):

- Receipt → `finvoroo.print_agent.receipt_printer` (falls back to `printer_id`)
- Receipt paper → `finvoroo.receipt_paper` (`thermal_58` | `thermal_80`)
- Invoice → `finvoroo.print_agent.invoice_printer`
- Label → `finvoroo.print_agent.label_printer`

## Pharmacy setup

```text
Download FinvorooPrintAgent-Setup.exe
        ↓
Install (NSIS, current user)
        ↓
Agent starts, tray icon, autostart on login
        ↓
Show pairing code in the tray window
        ↓
Finvoroo → Pair → pick Receipt / Invoice / Label printers
        ↓
Done
```

## What was not changed

- Laravel `PosController` checkout
- QZ Tray cert/signing routes and `qz-print-service.js`
- Chrome kiosk shortcut (`silent-print-shortcut.js`) for `browser` driver tills

## Tests (developers only)

```bat
cd finvoroo-print-agent
npm test

cd ../React-frontend
npm run test:print-agent
```
