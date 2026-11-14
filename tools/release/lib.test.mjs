import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  STABLE_SETUP_NAME,
  installersFor,
  publishedNotes,
  pushCommand,
  stableSetupSource,
  tagMatchesVersion,
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
  // README.md is the Arabic one, the page GitHub shows first; README.en.md is the English one.
  for (const name of ["README.md", "README.en.md"]) {
    const readme = readFileSync(new URL(`../../${name}`, import.meta.url), "utf8");
    assert.ok(
      readme.includes(`https://github.com/Hjbiki/5g-router-band-tuner/releases/latest/download/${STABLE_SETUP_NAME}`),
      `${name} has no link to the newest ${STABLE_SETUP_NAME}`,
    );
  }
});

test("a release tag must name the version in the manifests", () => {
  assert.equal(tagMatchesVersion("v1.0.0", "1.0.0"), true);
  assert.equal(tagMatchesVersion("v1.0.1", "1.0.0"), false);
  assert.equal(tagMatchesVersion("1.0.0", "1.0.0"), false);
  assert.equal(tagMatchesVersion("v1.0.0-rc.1", "1.0.0"), false);
  assert.equal(tagMatchesVersion("", "1.0.0"), false);
});

test("the installers of a version are the files that carry exactly its number", () => {
  const names = [
    "5G Router Band Tuner_1.0.0_x64-setup.exe",
    "5G Router Band Tuner_1.0.0_x64_en-US.msi",
    "5G Router Band Tuner_11.0.0_x64-setup.exe",
    "5G Router Band Tuner_1.0.0-rc1_x64-setup.exe",
    "5G-Router-Band-Tuner-Setup.exe",
    "notes_1.0.0_.txt",
  ];
  assert.deepEqual(installersFor(names, "1.0.0"), [names[0], names[1]]);
  assert.deepEqual(installersFor([], "1.0.0"), []);
});

test("the published notes are the changelog entry followed by each file's SHA-256", () => {
  const ab = "ab".repeat(32);
  const cd = "cd".repeat(32);
  const notes = publishedNotes("## [1.0.0] - 2026-11-20\n\n### Added\n\n- A thing.\n", [
    { name: "5G-Router-Band-Tuner-Setup.exe", sha256: ab },
    { name: "5G Router Band Tuner_1.0.0_x64_en-US.msi", sha256: cd },
  ]);
  assert.ok(notes.startsWith("## [1.0.0] - 2026-11-20\n\n### Added\n\n- A thing.\n\n### SHA-256\n\n"));
  assert.ok(notes.includes(`- \`5G-Router-Band-Tuner-Setup.exe\`: \`${ab}\`\n`));
  assert.ok(notes.endsWith(`- \`5G Router Band Tuner_1.0.0_x64_en-US.msi\`: \`${cd}\`\n`));
});

const workflow = (name) => readFileSync(new URL(`../../.github/workflows/${name}`, import.meta.url), "utf8");

test("every action a workflow uses is pinned to one exact commit", () => {
  for (const name of ["ci.yml", "release.yml"]) {
    const used = [...workflow(name).matchAll(/^\s*(?:- )?uses:\s*(\S+)/gm)].map((m) => m[1]);
    assert.ok(used.length > 0, `${name} uses no action`);
    for (const ref of used) {
      // A workflow in this repo is called by its path; anything else needs a full commit hash.
      if (ref.startsWith("./")) continue;
      assert.match(ref, /^[\w.-]+\/[\w.-]+@[0-9a-f]{40}$/, `${name}: ${ref} isn't pinned to a commit`);
    }
  }
});

test("only the publishing job may write to the repo, and only on a version tag", () => {
  assert.ok(!workflow("ci.yml").includes("contents: write"), "the checks must stay read-only");
  const release = workflow("release.yml");
  assert.equal(release.match(/contents: write/g)?.length, 1);
  const publish = release.slice(release.indexOf("\n  publish:"));
  assert.ok(publish.includes("contents: write"), "the write permission belongs to the publish job");
  assert.ok(publish.includes("if: github.ref_type == 'tag'"), "publishing is for tag pushes only");
  assert.ok(publish.includes("needs: [checks, build]"), "publishing waits for the checks and the build");
  // Version tags only: a branch push or any other tag never starts a release.
  assert.match(release, /tags:\s*\['v\[0-9\]\+\.\[0-9\]\+\.\[0-9\]\+'\]/);
});

test("the upload folder holds only this version's files, whatever an earlier build left there", () => {
  // The build folder comes back from the workflow's cache, so it can hold an earlier version's
  // installers and upload folder. None of that may reach the release.
  const root = mkdtempSync(join(tmpdir(), "rbt-collect-"));
  try {
    const put = (path, text = "x") => {
      mkdirSync(join(root, path, ".."), { recursive: true });
      writeFileSync(join(root, path), text);
    };
    for (const name of ["ci-release.mjs", "lib.mjs"]) {
      mkdirSync(join(root, "tools/release"), { recursive: true });
      copyFileSync(fileURLToPath(new URL(name, import.meta.url)), join(root, "tools/release", name));
    }
    put("Cargo.toml", '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "1.0.0"\n');
    put("CHANGELOG.md", "# Changelog\n\n## [1.0.0] - d\n\n- New.\n\n## [0.1.0] - d\n\n- Old.\n");
    put("target/release/bundle/nsis/5G Router Band Tuner_1.0.0_x64-setup.exe", "new setup");
    put("target/release/bundle/msi/5G Router Band Tuner_1.0.0_x64_en-US.msi", "new msi");
    put("target/release/bundle/nsis/5G Router Band Tuner_0.1.0_x64-setup.exe", "old setup");
    put("target/release-upload/5G Router Band Tuner_0.1.0_x64-setup.exe", "old setup");
    put("target/release-upload/5G Router Band Tuner_0.1.0_x64_en-US.msi", "old msi");
    put("target/release-upload/5G-Router-Band-Tuner-Setup.exe", "old setup");
    put("target/release-upload/notes.md", "old notes");

    const run = spawnSync(process.execPath, [join(root, "tools/release/ci-release.mjs"), "collect"], {
      encoding: "utf8",
    });
    assert.equal(run.status, 0, run.stderr);

    const out = join(root, "target/release-upload");
    assert.deepEqual(readdirSync(out).sort(), [
      "5G Router Band Tuner_1.0.0_x64-setup.exe",
      "5G Router Band Tuner_1.0.0_x64_en-US.msi",
      STABLE_SETUP_NAME,
      "notes.md",
    ]);
    assert.equal(readFileSync(join(out, STABLE_SETUP_NAME), "utf8"), "new setup");
    const notes = readFileSync(join(out, "notes.md"), "utf8");
    assert.ok(notes.startsWith("## [1.0.0] - d\n\n- New.\n\n### SHA-256\n\n"), notes);
    assert.ok(!notes.includes("0.1.0"), "the notes list an earlier version's file");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("the upload command names the branch and the one tag, never every tag", () => {
  assert.equal(pushCommand("1.0.0"), "git push origin main v1.0.0");
  assert.ok(!pushCommand("1.0.0").includes("--tags"));
});
