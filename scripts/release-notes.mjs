/**
 * Generate GitHub release notes from cinny/CHANGELOG.md.
 *
 * Writes the most recent dated section (top of the file) to
 * `release-notes.md` in the repo root. The build workflow consumes
 * that file via `gh release create --notes-file release-notes.md`
 * (or `--notes` after `cat`).
 *
 * Format mirrors the in-app changelog viewer — see
 * cinny/src/app/features/changelog/parser.ts for the parsing rules
 * the app itself uses.
 *
 *   ## DD.MM.YYYY
 *
 *   - `<7-8 char SHA>` Verb followed by description.
 *
 * Falls back to a "see commit log" stub if the changelog is missing,
 * empty, or unparseable so a release never blocks on a CI script bug.
 */
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const CHANGELOG = join(ROOT, 'cinny', 'CHANGELOG.md');
const OUT = join(ROOT, 'release-notes.md');

const FALLBACK = 'See commit log for changes — changelog not available.\n';
const DATE_HEADING_RE = /^##\s+\d{2}\.\d{2}\.\d{4}\s*$/m;

function emit(text) {
  writeFileSync(OUT, text);
  console.log('--- release-notes.md ---');
  console.log(text);
}

if (!existsSync(CHANGELOG)) {
  console.warn(`[release-notes] ${CHANGELOG} not found — emitting fallback.`);
  emit(FALLBACK);
  process.exit(0);
}

const md = readFileSync(CHANGELOG, 'utf8');

// Split on lines that start a dated section. The first capture before any
// heading (the top-level "# Changelog" intro) is discarded; sections come
// in newest-first order.
const sections = md
  .split(/(?=^##\s+\d{2}\.\d{2}\.\d{4}\s*$)/m)
  .filter((s) => DATE_HEADING_RE.test(s));

if (sections.length === 0) {
  console.warn('[release-notes] No dated sections found — emitting fallback.');
  emit(FALLBACK);
  process.exit(0);
}

// Most recent section sits at index 0 because the file convention is
// newest-at-top. Trim trailing footer line ("[End of changelog. ...]") if
// it landed inside the section (it shouldn't if the file is well-formed).
let latest = sections[0].trim();
latest = latest.replace(/\n\[End of changelog\.[^\]]*\]\s*$/, '').trim();

if (!latest) {
  emit(FALLBACK);
  process.exit(0);
}

emit(latest + '\n');
