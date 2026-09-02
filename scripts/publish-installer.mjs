#!/usr/bin/env node
/**
 * Copies the Tauri NSIS setup.exe to FinvorooPrintAgent-Setup.exe and writes
 * print-agent-latest.json when the build produced a minisign .sig file.
 */

import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const nsisDir = join(root, 'src-tauri', 'target', 'release', 'bundle', 'nsis');
const distDir = join(root, 'dist');
const publicDownloadsDir = join(root, '..', 'React-frontend', 'public', 'downloads');
const destName = 'FinvorooPrintAgent-Setup.exe';
const manifestName = 'print-agent-latest.json';

function readVersion() {
  try {
    const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
    if (pkg?.version) return String(pkg.version);
  } catch {
    /* ignore */
  }
  return '0.0.0';
}

function findSignatureFile(setupExeName) {
  const direct = join(nsisDir, `${setupExeName}.sig`);
  if (existsSync(direct)) return direct;

  const sigs = readdirSync(nsisDir).filter((name) => name.toLowerCase().endsWith('.sig'));
  if (sigs.length === 1) return join(nsisDir, sigs[0]);

  const stem = setupExeName.toLowerCase().replace(/\.exe$/i, '');
  const match = sigs.find((name) => name.toLowerCase().includes(stem));
  return match ? join(nsisDir, match) : null;
}

function writeManifest(signature, version) {
  const manifest = {
    version,
    notes: 'Finvoroo Print Agent update',
    pub_date: new Date().toISOString(),
    platforms: {
      'windows-x86_64': {
        signature: signature.trim(),
        url: `https://app.finvoroo.com/downloads/${destName}`,
      },
    },
  };
  const manifestJson = `${JSON.stringify(manifest, null, 2)}\n`;
  writeFileSync(join(distDir, manifestName), manifestJson);
  if (existsSync(join(root, '..', 'React-frontend'))) {
    mkdirSync(publicDownloadsDir, { recursive: true });
    writeFileSync(join(publicDownloadsDir, manifestName), manifestJson);
  }
  return join(distDir, manifestName);
}

if (!existsSync(nsisDir)) {
  console.error(`NSIS output not found: ${nsisDir}`);
  console.error('Build the Windows installer on a Windows machine: npm run build:windows-installer');
  process.exit(1);
}

const exes = readdirSync(nsisDir).filter((name) => name.toLowerCase().endsWith('.exe'));
const setup =
  exes.find((name) => name.toLowerCase().endsWith('-setup.exe') && name !== destName) ||
  exes.find((name) => name === destName);

if (!setup) {
  console.error(`No NSIS setup .exe in ${nsisDir}`);
  process.exit(1);
}

const source = join(nsisDir, setup);
mkdirSync(distDir, { recursive: true });
const nsisDest = join(nsisDir, destName);
const distDest = join(distDir, destName);
copyFileSync(source, nsisDest);
copyFileSync(source, distDest);

if (existsSync(join(root, '..', 'React-frontend'))) {
  mkdirSync(publicDownloadsDir, { recursive: true });
  copyFileSync(source, join(publicDownloadsDir, destName));
}

console.log('Installer ready for pharmacies (no Node/Rust required):');
console.log(`  ${nsisDest}`);
console.log(`  ${distDest}`);

const sigPath = findSignatureFile(setup);
if (sigPath) {
  const manifestPath = writeManifest(readFileSync(sigPath, 'utf8'), readVersion());
  console.log(`Updater manifest ready (upload alongside the installer):`);
  console.log(`  ${manifestPath}`);
} else {
  console.log(
    `No .sig found in ${nsisDir} — skipping ${manifestName} ` +
      '(set createUpdaterArtifacts: true and TAURI_SIGNING_PRIVATE_KEY to produce one).',
  );
  if (process.env.CI === 'true' && process.env.GITHUB_REF?.includes('/tags/print-agent-v')) {
    console.error('Tag release requires a signed updater manifest.');
    process.exit(1);
  }
}
