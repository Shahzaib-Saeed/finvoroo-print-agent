# Updates for the installed Print Agent

Pharmacies **never** need Node.js, Rust, npm, or developer tools to install or update.

## Install once, update automatically (current)

1. Pharmacy installs `FinvorooPrintAgent-Setup.exe` once (see `CLIENT.md`).
2. From then on, the running tray agent checks `plugins.updater.endpoints` (see
   `src-tauri/tauri.conf.json`) on startup and every hour (`src-tauri/src/updater.rs`).
3. When a newer signed release exists, it is downloaded in the background and its
   **minisign signature is verified** before anything is installed — an unsigned or
   tampered installer is rejected automatically.
4. The agent waits until no `/print` request has started in the last 15 seconds
   (`AppState::last_print_at`, capped at a 10-minute wait) so an update never lands
   mid-receipt, then runs the installer silently (`windows.installMode: "quiet"`)
   and restarts the tray — invisible to the cashier.
5. Pairing, token, and default printer stay in `%APPDATA%\com.finvoroo.print-agent\`
   (NSIS `currentUser` installs never touch AppData on install/uninstall).

This uses [tauri-plugin-updater](https://v2.tauri.app/plugin/updater/) with an
Ed25519 (minisign) keypair generated once offline. This is independent of a
Windows Authenticode certificate — Authenticode is optional (it only affects
SmartScreen warnings on a manually double-clicked download) and is **not**
required for the signed-update verification above.

## Publishing a new release

1. Bump `version` in `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`.
2. CI (`.github/workflows/print-agent-windows.yml`) builds with
   `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets set,
   which makes `tauri build` (via `createUpdaterArtifacts: true`) emit a `.sig`
   file next to the NSIS installer.
3. `npm run build` (which runs `scripts/publish-installer.mjs`) assembles
   `dist/print-agent-latest.json` — the Tauri-updater manifest — from that
   signature, alongside the renamed `FinvorooPrintAgent-Setup.exe`.
4. Upload `FinvorooPrintAgent-Setup.exe` **and** `print-agent-latest.json` to
   `app.finvoroo.com/downloads/` (manual upload today, same as the installer
   always has been — see the CI artifact for both files).
5. Installed agents pick it up on their next hourly check (or next restart).

## Keys

The signing keypair (`tauri signer generate`) was generated once, offline. The
private key + its password live **only** in this repo's GitHub Actions secrets
(`TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) — never in
git. The public key is committed in `src-tauri/tauri.conf.json`
(`plugins.updater.pubkey`); losing the private key means a new keypair (and a
new pubkey shipped in the next manually-installed release) is required.
