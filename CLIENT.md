# Finvoroo Print Agent — pharmacy install

You only need a Windows PC and the installer. You do **not** need Node.js, npm, Rust, Git, or any developer tools.

```text
Download FinvorooPrintAgent-Setup.exe
        ↓
Install
        ↓
Agent appears in the Windows tray
        ↓
Open Finvoroo in Chrome
        ↓
Pair with the 6-digit code
        ↓
Select printers
        ↓
Test print
        ↓
Done
```

1. Download **FinvorooPrintAgent-Setup.exe**.
2. Double-click it and complete the installer.
3. Look for **Finvoroo Print Agent** in the system tray (bottom-right). Click it if you need the pairing window.
4. Open Finvoroo in Chrome on **this same computer**.
5. In the Print Agent window, show the 6-digit pairing code.
6. In Finvoroo settings, enter that code, click **Pair**, and turn **Use agent** on.
7. Choose Receipt, Invoice, and Label printers, then **Test print**.

Install your usual Windows printer drivers first (receipt printer, laser, Zebra, and so on). The Print Agent uses those printers.

To update, run the newer `FinvorooPrintAgent-Setup.exe`. Your pairing stays on this PC.

Pair **once**. After that the agent stays paired through reboots, internet outages, and switching between the Finvoroo website and Finvoroo Desktop. You only need a new code if you click **Regenerate token** in the agent window.

## Mac

Download **FinvorooPrintAgent-Mac.dmg**, open it, drag *Finvoroo Print Agent* to *Applications* and open it
(first time: right-click → **Open**). It lives in the menu bar and starts at login. Add your printer first in
System Settings → Printers & Scanners, then pair with the 6-digit code as on Windows. Use Chrome, Edge or Firefox (not Safari).
