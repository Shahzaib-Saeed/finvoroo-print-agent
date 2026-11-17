#!/usr/bin/env node
/**
 * Collects the macOS build (universal: Intel + Apple Silicon) for the website:
 *   FinvorooPrintAgent-Mac.dmg            what a pharmacy downloads
 *   FinvorooPrintAgent-Mac.app.tar.gz     what the in-app updater downloads (only with a signing key)
 * and adds the darwin entries to print-agent-latest.json WITHOUT touching the Windows entry,
 * so run it after (or before) publish-installer.mjs in either order.
 */

import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const bundleDirs = ['universal-apple-darwin', 'aarch64-apple-darwin', 'x86_64-apple-darwin', ''].map((t) =>
  join(root, 'src-tauri', 'target', ...(t ? [t] : []), 'release', 'bundle'),
);
const bundleRoot = bundleDirs.find((d) => existsSync(join(d, 'dmg')));
const distDir = join(root, 'dist');
const publicDownloadsDir = join(root, '..', 'React-frontend', 'public', 'downloads');
const dmgName = 'FinvorooPrintAgent-Mac.dmg';
const tarName = 'FinvorooPrintAgent-Mac.app.tar.gz';
const manifestName = 'print-agent-latest.json';

if (!bundleRoot) {
  console.error('No macOS bundle found. Run: npm run build:mac (on a Mac)');
  process.exit(1);
}

const version = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')).version;
const pick = (dir, suffix) => {
  const d = join(bundleRoot, dir);
  return existsSync(d) ? readdirSync(d).find((n) => n.endsWith(suffix)) && join(d, readdirSync(d).find((n) => n.endsWith(suffix))) : null;
};

const dmg = pick('dmg', '.dmg');
if (!dmg) {
  console.error(`No .dmg in ${join(bundleRoot, 'dmg')}`);
  process.exit(1);
}

const targets = [distDir];
if (existsSync(join(root, '..', 'React-frontend'))) targets.push(publicDownloadsDir);
for (const dir of targets) {
  mkdirSync(dir, { recursive: true });
  copyFileSync(dmg, join(dir, dmgName));
}
console.log(`Mac installer ready: ${join(distDir, dmgName)}`);

const tar = pick('macos', '.app.tar.gz');
const sig = tar && existsSync(`${tar}.sig`) ? `${tar}.sig` : null;
if (!tar || !sig) {
  console.log('No updater archive/.sig (TAURI_SIGNING_PRIVATE_KEY was not set) — Macs will need to download updates by hand.');
  process.exit(0);
}

for (const dir of targets) copyFileSync(tar, join(dir, tarName));
const entry = {
  signature: readFileSync(sig, 'utf8').trim(),
  url: `https://app.finvoroo.com/downloads/${tarName}`,
};

for (const dir of targets) {
  const file = join(dir, manifestName);
  const manifest = existsSync(file)
    ? JSON.parse(readFileSync(file, 'utf8'))
    : { version, notes: 'Finvoroo Print Agent update', pub_date: new Date().toISOString(), platforms: {} };
  if (manifest.version !== version) {
    console.warn(`Manifest version ${manifest.version} differs from this build (${version}); publish both platforms from the same version.`);
  }
  manifest.platforms = { ...manifest.platforms, 'darwin-aarch64': entry, 'darwin-x86_64': entry };
  writeFileSync(file, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`Updated ${file}`);
}
