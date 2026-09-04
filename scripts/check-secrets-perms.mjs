#!/usr/bin/env node
/**
 * Enforces owner-only permissions on the local release signing material.
 *
 * `.secrets/` holds the Tauri updater minisign private key, the passphrase that
 * decrypts it, and the Android release keystore. The directory is gitignored and
 * untracked, so git will never tell you if its modes drift - and they had
 * drifted: the updater key and its passphrase were mode 0644 inside a 0755
 * directory while the Android keystore beside them was correctly 0600. Anything
 * able to read those two files can sign an update that every installed client
 * accepts, because the matching public key is compiled into tauri.conf.json.
 *
 * Runs as part of the desktop build (see tauri.conf.json beforeBuildCommand), so
 * a regression is caught on the machine that actually holds the keys. In CI the
 * directory does not exist - signing material comes from repository secrets -
 * and this is a no-op.
 *
 *   node scripts/check-secrets-perms.mjs         report and fail on drift
 *   node scripts/check-secrets-perms.mjs --fix   correct the modes in place
 */
import { chmodSync, existsSync, readdirSync, statSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const SECRETS = join(ROOT, '.secrets');
const FIX = process.argv.includes('--fix');

const DIR_MODE = 0o700;
const FILE_MODE = 0o600;

if (!existsSync(SECRETS)) {
  // Normal in CI and for any clone that does not hold release keys.
  console.log('[check-secrets-perms] no .secrets/ - nothing to check');
  process.exit(0);
}

const wrong = [];
const check = (path, want, label) => {
  const mode = statSync(path).mode & 0o777;
  if (mode === want) return;
  if (FIX) {
    chmodSync(path, want);
    console.log(`[check-secrets-perms] fixed ${label}: ${mode.toString(8)} -> ${want.toString(8)}`);
    return;
  }
  wrong.push(`${label} is ${mode.toString(8)}, expected ${want.toString(8)}`);
};

check(SECRETS, DIR_MODE, '.secrets/');
for (const name of readdirSync(SECRETS)) {
  check(join(SECRETS, name), FILE_MODE, `.secrets/${name}`);
}

if (wrong.length > 0) {
  console.error('[check-secrets-perms] release signing material is not owner-only:\n');
  for (const w of wrong) console.error(`  - ${w}`);
  console.error('\nRun: node scripts/check-secrets-perms.mjs --fix');
  process.exit(1);
}

console.log('[check-secrets-perms] OK - .secrets/ is owner-only');
