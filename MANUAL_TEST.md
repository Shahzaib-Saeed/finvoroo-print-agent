# Manual test checklist

Pharmacies must be able to complete printing after installing **only** `FinvorooPrintAgent-Setup.exe`. They must not be asked to install Node.js, Rust, npm, Git, or Visual Studio.

## Clean Windows PC (acceptance)

Use a Windows 10/11 machine with **no** developer tools installed.

- [ ] Copy `FinvorooPrintAgent-Setup.exe` onto the PC (USB, email, or download). Node.js/Rust/npm are **not** installed.
- [ ] Double-click the installer. It completes without Command Prompt or extra admin configuration beyond a normal Windows install.
- [ ] Start Menu contains **Finvoroo → Finvoroo Print Agent**.
- [ ] After install, the Print Agent launches and a tray icon appears.
- [ ] Closing the window hides to the tray; **Quit** from the tray exits.
- [ ] Reboot: agent autostarts; tray icon returns.
- [ ] Open Finvoroo in Chrome on this PC (`https://*.finvoroo.com` or localhost).
- [ ] Show pairing code in the agent → enter 6-digit code in Finvoroo → **Pair** → **Use agent** = ON.
- [ ] Receipt / Invoice / Label printer lists show Windows printers.
- [ ] Test print succeeds with **no** Chrome print dialog.
- [ ] POS complete-and-print is silent (no Chrome dialog)
- [ ] Receipt layout matches Chrome preview (logo, table borders, barcode stripes, grand total box)
- [ ] Short and long receipts (20+ lines) cut correctly on thermal paper
- [ ] 58 mm paper setting works when configured in Print Agent settings.
- [ ] Run a newer Setup.exe: pairing and printer selection still work (AppData preserved).

## Agent API (developer or IT)

- [ ] Quit from tray stops the API (`GET http://127.0.0.1:17392/status` fails)
- [ ] Launch again: `/status` returns `running: true` and does **not** include the token
- [ ] Unauthenticated `GET /printers` and `POST /print` return 401
- [ ] Wrong PIN is rejected; expired PIN is rejected
- [ ] Origin `https://evil.example` cannot pair
- [ ] Printer list includes USB, network, and Windows “Print to PDF”
- [ ] Invalid printer name returns `Printer "…" is unavailable.`
- [ ] PDF test print on an A4 printer (no Chrome dialog)
- [ ] ESC/POS / RAW test on a USB thermal receipt printer
- [ ] HTML thermal receipt via `POST /print` type `html` (silent, no browser)
- [ ] ZPL test on Zebra GK420d (or any ZDesigner printer) — no browser dialog
- [ ] Large PDF (~10–20 MB) prints without crashing
- [ ] Two printers: receipt vs invoice selection in Finvoroo both work

## Finvoroo React

- [ ] Agent not installed: settings show “Finvoroo Print Agent is not installed” + Install button
- [ ] Agent offline after pairing: POS print toasts “Finvoroo Print Agent is offline…”
- [ ] `printDriver=finvoroo-agent`: POS complete & print does **not** open Chrome
- [ ] Agent paired + offline: POS print toasts error only (no Chrome/QZ fallback)
- [ ] Receipt paper width setting (58 mm / 80 mm) persists and affects print width
- [ ] `printDriver=browser` (default on unpaired tills): Chrome print still works
- [ ] `printDriver=qz`: QZ Tray path still available
- [ ] Universal POS auto-print uses `printPosReceipt` (same agent path)
- [ ] Label ZPL goes to the Label printer when set

Do not require Finvoroo staff to log into the customer PC. The customer’s Windows printer list is the source of truth. Do not move printing onto the Laravel server.

## HTML thermal receipt comparison (POS 80)

Run on a Windows till with a real thermal printer after deploying **both** a new Print Agent installer and React build.

1. Print one receipt via Chrome (`printDriver=browser`, `window.print`) — reference.
2. Pair agent, set receipt printer to **POS 80**, paper **80 mm**.
3. Complete a sale with `printDriver=finvoroo-agent` — compare side-by-side.

Checklist:

| Item | Chrome | Agent HTML |
|------|--------|------------|
| Store logo | | |
| Barcode CSS stripes | | |
| Table borders | | |
| Grand total box | | |
| Long product names wrap | | |
| 20+ line receipt / cut position | | |
| Discount / VAT lines | | |
| Footer branding | | |

Target latency: checkout → print start under ~500 ms after agent warm-up.

Document any residual gaps (WebView2/Edge vs Chrome, driver horizontal scaling). CSS barcode is DOM stripes, not hardware barcode commands — same as Chrome.
