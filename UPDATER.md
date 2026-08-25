# Updates for the installed Print Agent

Pharmacies **never** need Node.js, Rust, npm, or developer tools to update.

## Current production path (supported)

1. Developer builds a new `FinvorooPrintAgent-Setup.exe` (`npm run build` on a Windows machine or GitHub Actions).
2. Pharmacy downloads the new `.exe` and runs it.
3. The NSIS installer replaces the application files under the install directory.
4. Pairing, token, and default printer stay in `%APPDATA%\com.finvoroo.print-agent\` (not deleted on update or uninstall).
5. After install, the tray agent starts again.

Until a code-signing certificate and Tauri updater keys exist, **this installer-based update is the only supported client update path**.

Do **not** enable `bundle.createUpdaterArtifacts` until those keys are in CI secrets. Enabling it without keys makes `tauri build` fail.

## Future: signed in-app updater

Do **not** download an unsigned `.exe` from an arbitrary URL.

Use [tauri-plugin-updater](https://v2.tauri.app/plugin/updater/) once Finvoroo has:

1. A Windows Authenticode certificate
2. A static JSON endpoint with version, notes, and artifact URLs
3. Minisign / Tauri updater keys in CI **secrets** (never in git)

Suggested layout:

```text
https://downloads.finvoroo.com/print-agent/latest.json
https://downloads.finvoroo.com/print-agent/FinvorooPrintAgent-Setup.exe
https://downloads.finvoroo.com/print-agent/FinvorooPrintAgent-Setup.exe.sig
```

`latest.json` (Tauri v2 format) must include the signature. The agent verifies it **before** replacing the binary. The client still does not need Node.js or Rust.

## Rollout checklist (when signing is ready)

1. Generate updater keypair offline; store the private key in CI
2. Add `tauri-plugin-updater` + a “Check for updates” tray item
3. Set `createUpdaterArtifacts` to `true` and publish NSIS installer + signature from the same pipeline as `npm run build`
4. Keep pairing token and printer names across updates (config stays in AppData)
