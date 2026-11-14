import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  STABLE_SETUP_NAME,
  stableSetupSource,
  bumpVersion,
  cargoWorkspaceVersion,
  changelogSection,
  decideBump,
  fillReleaseNotes,
  humanize,
  jsonVersion,
  needsShell,
  parseCommit,
  prependChangelog,
  releaseSection,
  setCargoWorkspaceVersion,
  setJsonVersion,
} from "./lib.mjs";

const c = (msg) => parseCommit(msg);

test("parses conventional subjects, scopes, ! and BREAKING CHANGE footers", () => {
  assert.deepEqual(c("fix(huawei): token error is a session loss"), {
    type: "fix",
    scope: "huawei",
    subject: "token error is a session loss",
    breaking: false,
    changelog: [],
  });
  assert.equal(c("feat!: new settings format").breaking, true);
  assert.equal(c("refactor: drop v1 journal\n\nBREAKING CHANGE: old journals are ignored").breaking, true);
  assert.equal(c("Add M1 implementation plan"), null);
  assert.deepEqual(c("fix: x\n\nChangelog: The app no longer logs out after five minutes.").changelog, [
    "The app no longer logs out after five minutes.",
  ]);
});

test("bump: breaking > feat > fix/perf; noise alone is not releasable", () => {
  const parse = (...m) => m.map(c);
  assert.equal(decideBump(parse("fix: a", "feat: b", "refactor!: c")), "major");
  assert.equal(decideBump(parse("fix: a", "feat: b")), "minor");
  assert.equal(decideBump(parse("perf: a", "docs: b")), "patch");
  assert.equal(decideBump(parse("docs: a", "chore: b", "test: c")), null);
  assert.equal(bumpVersion("0.1.0", "major"), "1.0.0");
  assert.equal(bumpVersion("0.1.3", "minor"), "0.2.0");
  assert.equal(bumpVersion("0.1.3", "patch"), "0.1.4");
});

test("changelog groups features, fixes and improvements in plain sentences, without noise", () => {
  const commits = [
    "feat(bands): star a primary band",
    "fix(ui): keep band edits across tabs",
    "perf: poll less often in the tray",
    "docs: update README",
    "chore(release): v0.2.0",
    "fix: x\n\nChangelog: Keep and Revert respond immediately, even when the router is slow.",
    "refactor!: settings file moved",
  ].map(c);
  const s = changelogSection("0.2.0", "2026-10-30", commits);
  assert.equal(
    s,
    [
      "## [0.2.0] - 2026-10-30",
      "",
      "### Added",
      "",
      "- Star a primary band.",
      "",
      "### Fixed",
      "",
      "- Keep band edits across tabs.",
      "- Keep and Revert respond immediately, even when the router is slow.",
      "",
      "### Improved",
      "",
      "- Poll less often in the tray.",
      "",
      "### Changed",
      "",
      "- **Breaking:** Settings file moved.",
      "",
    ].join("\n"),
  );
  assert.equal(humanize("already ends?"), "Already ends?");
});

test("generated text always uses the app's current name", () => {
  assert.equal(humanize("rename the app to Router Band Tuner"), "Rename the app to 5G Router Band Tuner.");
  assert.equal(humanize("display name 5G Router Band Tuner"), "Display name 5G Router Band Tuner.");
  const old = c("refactor!: rename\n\nChangelog: The app is now called Router Band Tuner. Settings carry over.");
  const text = changelogSection("1.0.0", "2026-11-01", [old]);
  assert.match(text, /\*\*Breaking:\*\* The app is now called 5G Router Band Tuner\. Settings carry over\./);
  assert.doesNotMatch(text, /called Router Band Tuner/);
});

test("fillReleaseNotes fills the date and normalises line endings", () => {
  assert.equal(fillReleaseNotes("## [1.0.0] - {{date}}\r\n\r\n- New.\r\n\r\n", "2026-11-01"), "## [1.0.0] - 2026-11-01\n\n- New.\n");
});

test("releaseSection finds one entry and stops at the next", () => {
  const text = "# Changelog\r\n\r\n## [0.2.0] - d\r\n\r\n- New.\r\n\r\n## [0.1.0] - d\r\n\r\n- Old.\r\n";
  assert.equal(releaseSection(text, "0.2.0"), "## [0.2.0] - d\n\n- New.\n");
  assert.equal(releaseSection(text, "0.1.0"), "## [0.1.0] - d\n\n- Old.\n");
  assert.equal(releaseSection(text, "9.9.9"), null);
});

test("only pnpm on Windows runs through a shell, so messages and paths stay one argument", () => {
  assert.equal(needsShell("pnpm", "win32"), true);
  for (const cmd of ["git", "gh", "cargo", "node"]) assert.equal(needsShell(cmd, "win32"), false);
  assert.equal(needsShell("pnpm", "linux"), false);
});

test("prepend keeps the header and puts the new release first", () => {
  const first = prependChangelog("", "## [0.1.0] - d\n\n- a\n");
  assert.match(first, /^# Changelog\n/);
  const second = prependChangelog(first, "## [0.2.0] - e\n\n- b\n");
  assert.ok(second.indexOf("[0.2.0]") < second.indexOf("[0.1.0]"));
  assert.equal(second.match(/# Changelog/g).length, 1);
});

test("version files are edited in place", () => {
  const cargo = '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "0.1.0"\nedition = "2021"\n\n[x]\nversion = "9"\n';
  assert.equal(cargoWorkspaceVersion(cargo), "0.1.0");
  const bumped = setCargoWorkspaceVersion(cargo, "0.2.0");
  assert.equal(cargoWorkspaceVersion(bumped), "0.2.0");
  assert.match(bumped, /\[x\]\nversion = "9"/);
  const json = '{\n  "name": "ui",\n  "version": "0.1.0",\n  "deps": { "version": "1" }\n}\n';
  assert.equal(jsonVersion(json), "0.1.0");
  assert.equal(setJsonVersion(json, "1.0.0"), json.replace('"0.1.0"', '"1.0.0"'));
});

test("the fixed-name installer is made from the NSIS setup, not the MSI", () => {
  const msi = "C:/out/msi/5G Router Band Tuner_1.0.0_x64_en-US.msi";
  const setup = "C:/out/nsis/5G Router Band Tuner_1.0.0_x64-setup.exe";
  assert.deepEqual(stableSetupSource([msi, setup]), { source: setup, problem: null });
  assert.equal(stableSetupSource([msi]).source, null);
  assert.match(stableSetupSource([msi]).problem, /no NSIS setup/);
  assert.equal(stableSetupSource([]).source, null);
});

test("two setups for one version are refused, so the release never guesses what the button serves", () => {
  const a = "C:/out/nsis/5G Router Band Tuner_1.0.0_x64-setup.exe";
  const b = "C:/out/nsis/5G Router Band Tuner_1.0.0-rc1_x64-setup.exe";
  const found = stableSetupSource([a, b]);
  assert.equal(found.source, null);
  assert.match(found.problem, /2 setup/);
});

test("the fixed name is safe to put in a link as it is", () => {
  assert.match(STABLE_SETUP_NAME, /^[A-Za-z0-9.-]+\.exe$/);
});

test("both READMEs link their Download button to the file every release uploads", () => {
  for (const name of ["README.md", "README.ar.md"]) {
    const readme = readFileSync(new URL(`../../${name}`, import.meta.url), "utf8");
    assert.ok(
      readme.includes(`https://github.com/Hjbiki/5g-router-band-tuner/releases/latest/download/${STABLE_SETUP_NAME}`),
      `${name} has no link to the newest ${STABLE_SETUP_NAME}`,
    );
  }
});
