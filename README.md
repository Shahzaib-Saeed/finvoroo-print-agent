# Finvoroo Print Agent

Local silent printing for Finvoroo ERP. The agent is installed **on each Windows till PC**, not on the Laravel server.

```text
Finvoroo React Frontend
        ↓  HTTP 127.0.0.1:17392
localhost Print Agent (Windows tray)
        ↓
Windows printer
        ↓
Receipt / Invoice / Label
```

---

## For pharmacies (clients)

You do **not** need Node.js, npm, Rust, Cargo, Git, Visual Studio, source code, or Command Prompt.

```text
Download FinvorooPrintAgent-Setup.exe
        ↓
Run the installer
        ↓
Agent appears in the Windows system tray
        ↓
Open Finvoroo in Chrome
        ↓
Enter the 6-digit pairing code
        ↓
Select printers
        ↓
Test print
        ↓
Done
```

1. Download **FinvorooPrintAgent-Setup.exe** (current release **v1.1.6**).
2. Run it and click through the installer (current-user install; no extra admin setup).
3. The Print Agent starts automatically and sits in the system tray.
4. It also starts with Windows after reboot.
5. Open Finvoroo (`https://*.finvoroo.com` or localhost) on **this same PC**.
6. In Print Agent settings, click **Show pairing code**.
7. In Finvoroo (Pharmacy Settings or Accounting → Print preferences), enter the 6-digit code and click **Pair**. Turn **Use agent** on.
8. Select Receipt, Invoice, and Label printers (these are the printers already installed in Windows).
9. Click **Test print**. After that, POS receipts, invoices, and labels print silently — Chrome’s print dialog is not used.

Install Windows printer drivers as you normally would (including ZDesigner for Zebra GK420d). The agent uses printers Windows already knows about.

To update later, download the new `FinvorooPrintAgent-Setup.exe` and run it. Pairing and printer choices stay on this PC.

See [CLIENT.md](CLIENT.md) for the pharmacy-only one-pager.

---

## For developers (build machine only)

These commands produce the installer. **Never** give them to a pharmacy.

```text
Build Print Agent
        ↓
Generate Windows installer
        ↓
Upload / distribute FinvorooPrintAgent-Setup.exe
```

### Windows build machine

1. Install [Rust](https://rustup.rs/), Node.js 20+, and Visual Studio C++ Build Tools.
2. WebView2 is bundled into the installer (bootstrapper).

```bat
cd finvoroo-print-agent
npm install
npm run build
```

Client-facing installer:

```text
src-tauri\target\release\bundle\nsis\FinvorooPrintAgent-Setup.exe
```

Also copied to `dist\FinvorooPrintAgent-Setup.exe` for easy upload.

Host that file at `https://finvoroo.com/downloads/finvoroo-print-agent` (or attach the GitHub Actions artifact). Pharmacies only download and run it.

NSIS can only be produced on **Windows** (or CI `windows-latest`). macOS/Linux `npm run build` will not produce the `.exe`.

### GitHub Actions

Workflow: `.github/workflows/print-agent-windows.yml` (`workflow_dispatch`). Download the `FinvorooPrintAgent-Setup` artifact.

### Local development (developers only)

```bat
cd finvoroo-print-agent
npm install
npm run dev
```

```bat
npm test
```

---

## Architecture and security (do not weaken)

- Binds **only** to `127.0.0.1:17392`.
- Pairing: tray shows a 6-digit PIN (60s) → Finvoroo Settings → Pair → Use agent = ON.
- CORS / Origin allowlist: `http://localhost`, `http://127.0.0.1`, `http://[::1]`, `https://finvoroo.com`, `https://*.finvoroo.com`.
- Print jobs require `X-Finvoroo-Print-Token` after pairing.
- Config / pairing live in `%APPDATA%\com.finvoroo.print-agent\` and survive app updates.
- Laravel checkout, FEFO, and journals are unchanged. The API never talks to the client printer.

See [IMPLEMENTATION.md](IMPLEMENTATION.md), [UPDATER.md](UPDATER.md), and [MANUAL_TEST.md](MANUAL_TEST.md).
