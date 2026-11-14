// Fails (exit 1) if the repo tracks captures, third-party bookmarklets or secrets.
// Prints offending paths only, never file content.
import { execFileSync } from 'node:child_process';
import { readFileSync, existsSync } from 'node:fs';
import { basename } from 'node:path';

// Files that legitimately name the forbidden patterns (this script).
const EXEMPT = new Set(['tools/check-repo-hygiene.mjs']);
const KEY = /-----BEGIN [A-Z ]*PRIVATE KEY-----/;
// PCIs can help locate someone, so tracked files only use made-up ones.
const DEMO_PCIS = new Set(['1', '100', '101', '103', '108', '120', '128', '140', '202']);
const PCI = /pci[\s:=">'`]{1,4}(\d+)/gi;
const PASSWORD_VAR = 'ROUTER_PASSWORD';

const files = execFileSync('git', ['ls-files', '-z'], { encoding: 'utf8', maxBuffer: 1 << 28 })
  .split('\0')
  .filter(Boolean);

const bad = new Set();
for (const f of files) {
  const name = basename(f);
  if (name === 'HAR' || name.endsWith('.har') || name === 'huawei-manager.js') bad.add(f);
}
for (const f of files) {
  if (EXEMPT.has(f) || bad.has(f) || !existsSync(f)) continue;
  const buf = readFileSync(f);
  if (buf.includes(0)) continue; // binary
  const text = buf.toString('utf8');
  if (KEY.test(text) || text.includes(PASSWORD_VAR)) bad.add(f);
  for (const m of text.matchAll(PCI)) if (!DEMO_PCIS.has(m[1])) bad.add(f);
}

if (bad.size) {
  console.error('Repo hygiene check failed for:');
  for (const f of [...bad].sort()) console.error('  ' + f);
  process.exit(1);
}
console.log('Repo hygiene OK (' + files.length + ' tracked files).');
