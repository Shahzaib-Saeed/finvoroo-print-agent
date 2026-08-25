#!/usr/bin/env node
/**
 * Copies the Tauri NSIS setup.exe to the client-facing name:
 *   FinvorooPrintAgent-Setup.exe
 *
 * Developer machines produce this file. Pharmacies only run the .exe.
 */

import { copyFileSync, existsSync, mkdirSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const nsisDir = join(root, 'src-tauri', 'target', 'release', 'bundle', 'nsis');
const distDir = join(root, 'dist');
const destName = 'FinvorooPrintAgent-Setup.exe';

if (!existsSync(nsisDir)) {
  console.error(`NSIS output not found: ${nsisDir}`);
  console.error('Build the Windows installer on a Windows machine:');
  console.error('  npm run build:windows-installer');
  process.exit(1);
}

const exes = readdirSync(nsisDir).filter((name) =>
  name.toLowerCase().endsWith('.exe'),
);
const setup =
  exes.find((name) => name.toLowerCase().endsWith('-setup.exe') && name !== destName) ||
  exes.find((name) => name === destName);

if (!setup) {
  console.error(`No NSIS setup .exe in ${nsisDir}`);
  console.error(
    'The NSIS installer is produced on Windows only (`npm run build` on a Windows build machine).',
  );
  process.exit(1);
}

const source = join(nsisDir, setup);
mkdirSync(distDir, { recursive: true });
const nsisDest = join(nsisDir, destName);
const distDest = join(distDir, destName);
copyFileSync(source, nsisDest);
copyFileSync(source, distDest);

console.log(`Installer ready for pharmacies (no Node/Rust required):`);
console.log(`  ${nsisDest}`);
console.log(`  ${distDest}`);
