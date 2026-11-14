// Release driver behind release.bat. See `release.bat --help`.
//
// 1. Reads the Conventional Commits since the last vX.Y.Z tag and decides the bump
//    (BREAKING -> major, feat -> minor, fix/perf -> patch); stops if nothing is releasable.
// 2. Shows the new version, the files it changes and the CHANGELOG entry, and asks to go on.
//    --dry-run stops after the preview and changes nothing.
// 3. Sets the version in every manifest and prepends the CHANGELOG entry.
// 4. Runs the full test suite and a release build (installers); any failure restores the files
//    and aborts.
// 5. Commits "chore(release): vX.Y.Z", tags vX.Y.Z, shows what would be published, then asks
//    before pushing and before creating a GitHub Release with the installers attached.
//    Answer no and `release.bat --publish` picks up from there.
//
// With --github-build, steps 4 and 5 shrink to: refresh Cargo.lock, commit, tag, and print the
// one command that uploads the branch and the tag. Nothing is tested, built or pushed on this
// PC: the Release workflow on GitHub runs the checks, builds the installers and publishes.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { createInterface } from "node:readline/promises";
import { stdin, stdout } from "node:process";
import { fileURLToPath } from "node:url";

import {
  APP_EXE,
  bumpVersion,
  cargoWorkspaceVersion,
  changelogSection,
  decideBump,
  fillReleaseNotes,
  installersFor,
  jsonVersion,
  KNOWN,
  needsShell,
  parseCommit,
  prependChangelog,
  pushCommand,
  releaseSection,
  setCargoWorkspaceVersion,
  setJsonVersion,
  STABLE_PORTABLE_NAME,
  STABLE_SETUP_NAME,
  stableSetupSource,
} from "./lib.mjs";

const ROOT = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const CHANGELOG = "CHANGELOG.md";
/** Every file that carries the app version. Cargo.lock follows the workspace version. */
const VERSION_FILES = [
  { path: "Cargo.toml", get: cargoWorkspaceVersion, set: setCargoWorkspaceVersion },
  { path: "src-tauri/tauri.conf.json", get: jsonVersion, set: setJsonVersion },
  { path: "package.json", get: jsonVersion, set: setJsonVersion },
  { path: "ui/package.json", get: jsonVersion, set: setJsonVersion },
];
/** Same checks as CI, then the installer build. */
const CHECKS = [
  // Only the workspace's own entries move, for the new version; every dependency stays as locked.
  ["cargo", ["update", "--workspace", "--offline"]],
  ["pnpm", ["install", "--frozen-lockfile"]],
  ["pnpm", ["--dir", "ui", "install", "--frozen-lockfile"]],
  ["cargo", ["fmt", "--all", "--", "--check"]],
  ["cargo", ["clippy", "--locked", "--workspace", "--all-targets", "--", "-D", "warnings"]],
  ["cargo", ["test", "--locked", "--workspace"]],
  ["node", ["--test", "tools/release/lib.test.mjs"]],
  ["node", ["tools/check-repo-hygiene.mjs"]],
  ["pnpm", ["--dir", "ui", "test"]],
  ["pnpm", ["--dir", "ui", "check"]],
  ["pnpm", ["tauri", "build", "--", "--locked"]],
];

const HELP = `Usage: release.bat [--dry-run] [--from <ref>] [--bump <kind>] [--notes-file <path>] [--prerelease]
       release.bat --github-build [--from <ref>] [--bump <kind>] [--notes-file <path>]
       release.bat --publish [--prerelease]

  --dry-run            show what would be released and stop; nothing is changed
  --from <ref>         compare against <ref> instead of the last vX.Y.Z tag (for previews)
  --bump <kind>        major, minor or patch, instead of the bump the commits imply
  --notes-file <path>  use this CHANGELOG entry instead of the generated one; it must start
                       with the "## [x.y.z] - date" line of the new version. Without this,
                       docs/release-notes/<version>.md is used when it exists. {{date}} in
                       the entry becomes the release date.
  --prerelease         mark the GitHub Release as a pre-release
  --publish            push and publish the release already tagged at HEAD
  --github-build       only bump, commit and tag here, then print the command that uploads
                       them; GitHub runs the checks, builds the installers and publishes

Commit messages must follow Conventional Commits. A "Changelog: <text>" footer
replaces the changelog line taken from the subject.`;

class Abort extends Error {}

function run(cmd, args, { capture = false } = {}) {
  const r = spawnSync(cmd, args, {
    cwd: ROOT,
    encoding: "utf8",
    shell: needsShell(cmd, process.platform),
    // Children never read stdin: it holds the answers to this script's own questions.
    stdio: capture ? ["ignore", "pipe", "pipe"] : ["ignore", "inherit", "inherit"],
  });
  if (r.error) throw new Abort(`${cmd} could not be started: ${r.error.message}`);
  return { ok: r.status === 0, out: (r.stdout ?? "").trim(), err: (r.stderr ?? "").trim() };
}

function git(...args) {
  const r = run("git", args, { capture: true });
  if (!r.ok) throw new Abort(`git ${args.join(" ")} failed: ${r.err}`);
  return r.out;
}

function parseArgs(argv) {
  const a = {
    dryRun: false,
    from: null,
    bump: null,
    notesFile: null,
    prerelease: false,
    publish: false,
    githubBuild: false,
  };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--dry-run") a.dryRun = true;
    else if (argv[i] === "--from") a.from = argv[++i] ?? null;
    else if (argv[i] === "--bump") a.bump = argv[++i] ?? null;
    else if (argv[i] === "--notes-file") a.notesFile = argv[++i] ?? null;
    else if (argv[i] === "--prerelease") a.prerelease = true;
    else if (argv[i] === "--publish") a.publish = true;
    else if (argv[i] === "--github-build") a.githubBuild = true;
    else if (argv[i] === "--help" || argv[i] === "-h") {
      console.log(HELP);
      process.exit(0);
    } else throw new Abort(`unknown argument ${argv[i]}\n\n${HELP}`);
  }
  if (a.from === null && argv.includes("--from")) throw new Abort("--from needs a ref");
  if (argv.includes("--bump") && !["major", "minor", "patch"].includes(a.bump)) {
    throw new Abort("--bump needs major, minor or patch");
  }
  if (a.notesFile === null && argv.includes("--notes-file")) throw new Abort("--notes-file needs a path");
  // The Release workflow only takes vX.Y.Z tags and publishes them as full releases.
  if (a.githubBuild && (a.prerelease || a.publish)) {
    throw new Abort("--github-build can't be combined with --prerelease or --publish");
  }
  return a;
}

/** Reads one answer per question from a terminal or a pipe. No answer left means no. */
function asker(rl) {
  const lines = rl[Symbol.asyncIterator]();
  return async (question) => {
    stdout.write(`${question} [y/N] `);
    const { value, done } = await lines.next();
    if (done) stdout.write("(no answer)\n");
    const answer = done ? "" : value.trim().toLowerCase();
    return answer === "y" || answer === "yes";
  };
}

function readCommits(base) {
  const range = base ? `${base}..HEAD` : "HEAD";
  const raw = git("log", "--reverse", "--format=%H%x1f%B%x1e", range);
  return raw
    .split("\x1e")
    .map((r) => r.trim())
    .filter(Boolean)
    .map((r) => {
      const [hash, message] = r.split("\x1f");
      return { hash: hash.trim(), message: message.trim(), parsed: parseCommit(message.trim()) };
    });
}

function today() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

function installers(version) {
  const found = [];
  for (const dir of ["target/release/bundle/nsis", "target/release/bundle/msi"]) {
    const abs = join(ROOT, dir);
    if (!existsSync(abs)) continue;
    for (const f of installersFor(readdirSync(abs), version)) found.push(join(abs, f));
  }
  return found;
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

/** Shows what would go out, then pushes and creates the GitHub Release, asking before each. */
async function publish(ask, version, section, prerelease) {
  const built = installers(version);
  if (!built.length) throw new Abort(`no installers for ${version} in target/release/bundle`);
  // The README's Download button links to this name on the newest release, so every release
  // carries a copy of the setup under it. A re-run overwrites the copy.
  const setup = stableSetupSource(built);
  if (!setup.source) throw new Abort(`${setup.problem}: the README's Download button needs exactly one`);
  const stable = join(ROOT, "target", STABLE_SETUP_NAME);
  copyFileSync(setup.source, stable);
  // The app's own exe runs without installing: the README's portable link serves this copy.
  const app = join(ROOT, APP_EXE);
  if (!existsSync(app)) throw new Abort(`no ${APP_EXE}: the README's portable link needs the app's own exe`);
  const portable = join(ROOT, "target", STABLE_PORTABLE_NAME);
  copyFileSync(app, portable);
  const files = [...built, stable, portable];
  const remote = run("git", ["remote", "get-url", "origin"], { capture: true });
  if (!remote.ok) throw new Abort("no 'origin' remote: add it, then run release.bat --publish");

  console.log(`\n===== Ready to publish v${version}${prerelease ? " (pre-release)" : ""} =====`);
  console.log(`Remote: ${remote.out}`);
  console.log(`Commit: ${git("log", "-1", "--format=%h %s")}`);
  console.log("Installers:");
  for (const f of files) {
    console.log(`  ${basename(f)}  ${(statSync(f).size / 1048576).toFixed(1)} MB  sha256 ${sha256(f)}`);
  }
  console.log(`\n----- Release notes -----\n${section}\n-------------------------`);

  if (!(await ask(`Push main and v${version} to ${remote.out}?`))) {
    console.log("Not pushed. The commit and tag are local; run release.bat --publish to go on.");
    return;
  }
  if (!run("git", ["push", "origin", "main"]).ok || !run("git", ["push", "origin", `v${version}`]).ok) {
    throw new Abort("push failed; the release commit and tag are still local");
  }
  const kind = prerelease ? "GitHub pre-release" : "GitHub Release";
  if (!(await ask(`Create a ${kind} v${version} with ${files.length} files attached (${STABLE_SETUP_NAME} is the copy the README links to)?`))) return;
  const notes = join(ROOT, "target", `release-notes-v${version}.md`);
  writeFileSync(notes, section);
  const flags = prerelease ? ["--prerelease"] : [];
  const r = run("gh", ["release", "create", `v${version}`, "--title", `v${version}`, "--notes-file", notes, ...flags, ...files]);
  if (!r.ok) throw new Abort("gh release create failed; the tag is pushed, run release.bat --publish again");
}

async function publishTagged(args, current) {
  const tag = run("git", ["tag", "--points-at", "HEAD", "--list", `v${current}`], { capture: true }).out;
  if (tag !== `v${current}`) throw new Abort(`HEAD isn't tagged v${current}: nothing to publish`);
  const section = releaseSection(readFileSync(join(ROOT, CHANGELOG), "utf8"), current);
  if (!section) throw new Abort(`${CHANGELOG} has no entry for ${current}`);
  const rl = createInterface({ input: stdin, output: stdout });
  try {
    await publish(asker(rl), current, section, args.prerelease);
  } finally {
    rl.close();
  }
  return 0;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));

  // Preconditions.
  const branch = git("rev-parse", "--abbrev-ref", "HEAD");
  if (!args.dryRun && branch !== "main") throw new Abort(`releases are made from main (on ${branch})`);
  if (!args.dryRun && git("status", "--porcelain")) throw new Abort("the working tree has uncommitted changes");

  const versions = VERSION_FILES.map((f) => ({ ...f, version: f.get(readFileSync(join(ROOT, f.path), "utf8")) }));
  const current = versions[0].version;
  const odd = versions.filter((v) => v.version !== current);
  if (odd.length) {
    throw new Abort(
      `versions disagree: ${versions.map((v) => `${v.path}=${v.version}`).join(", ")}. Fix them by hand first.`,
    );
  }

  if (args.publish) return publishTagged(args, current);

  const lastTag = run("git", ["describe", "--tags", "--abbrev=0", "--match", "v[0-9]*"], { capture: true });
  const base = args.from ?? (lastTag.ok ? lastTag.out : null);
  if (!base) throw new Abort("no vX.Y.Z tag yet: tag the first release by hand (git tag -a v0.1.0 -m v0.1.0)");
  if (!args.from && lastTag.out !== `v${current}`) {
    throw new Abort(`the last tag is ${lastTag.out} but the manifests say ${current}`);
  }

  const commits = readCommits(base);
  const parsed = commits.filter((c) => c.parsed && KNOWN.has(c.parsed.type)).map((c) => c.parsed);
  const unconventional = commits.filter((c) => !c.parsed || !KNOWN.has(c.parsed.type));
  const bump = decideBump(parsed);
  console.log(`\nLast release: ${base}  (${commits.length} commit(s) since)`);
  for (const c of unconventional) {
    console.log(`  ! not a Conventional Commit, ignored: ${c.hash.slice(0, 7)} ${c.message.split("\n")[0]}`);
  }
  if (!bump) {
    console.log("Nothing releasable since the last release (only chore/docs/test/refactor commits).");
    return 0;
  }

  const chosen = args.bump ?? bump;
  const next = bumpVersion(current, chosen);
  let section = changelogSection(next, today(), parsed);
  const notesFile = args.notesFile ?? join("docs", "release-notes", `${next}.md`);
  if (args.notesFile || existsSync(join(ROOT, notesFile))) {
    section = fillReleaseNotes(readFileSync(resolve(ROOT, notesFile), "utf8"), today());
    if (!section.startsWith(`## [${next}] - `)) {
      throw new Abort(`${notesFile} must start with "## [${next}] - <date>"`);
    }
    console.log(`\nRelease notes: ${notesFile}`);
  }
  const note = args.bump && args.bump !== bump ? `, the commits imply ${bump}` : "";
  console.log(`\nRelease: ${current} -> ${next}  (${chosen}${note})${args.prerelease ? "  pre-release" : ""}`);
  console.log(`Files: ${[...VERSION_FILES.map((f) => f.path), "Cargo.lock", CHANGELOG].join(", ")}`);
  console.log(`\n----- ${CHANGELOG} entry -----\n${section}\n------------------------------`);
  if (args.dryRun) {
    console.log("\nDry run: nothing was changed.");
    return 0;
  }

  const rl = createInterface({ input: stdin, output: stdout });
  const ask = asker(rl);
  try {
    const what = args.githubBuild
      ? "This only commits and tags here; GitHub tests, builds and publishes after you upload."
      : "This runs the full test suite and a release build.";
    if (!(await ask(`\nRelease v${next}? ${what}`))) {
      console.log("Cancelled; nothing was changed.");
      return 1;
    }

    // Bump every manifest and the changelog in one step.
    const changed = [...VERSION_FILES.map((f) => f.path), CHANGELOG];
    for (const f of VERSION_FILES) {
      const p = join(ROOT, f.path);
      writeFileSync(p, f.set(readFileSync(p, "utf8"), next));
    }
    const clPath = join(ROOT, CHANGELOG);
    const hadChangelog = existsSync(clPath);
    writeFileSync(clPath, prependChangelog(hadChangelog ? readFileSync(clPath, "utf8") : "", section));
    const restore = () => {
      const tracked = changed.filter((p) => p !== CHANGELOG || hadChangelog);
      run("git", ["reset", "-q"], { capture: true });
      run("git", ["checkout", "--", ...tracked, "Cargo.lock"], { capture: true });
      if (!hadChangelog) rmSync(clPath, { force: true });
    };

    // With --github-build only the lock file is brought in step here: a tag whose Cargo.lock
    // lags the new version fails every --locked command on GitHub.
    const lockInStep = [
      CHECKS[0],
      ["cargo", ["metadata", "--locked", "--offline", "--format-version", "1", "--no-deps"]],
    ];
    for (const [cmd, a] of args.githubBuild ? lockInStep : CHECKS) {
      console.log(`\n> ${cmd} ${a.join(" ")}`);
      // `cargo metadata` prints the whole workspace as JSON: only its verdict matters here.
      if (!run(cmd, a, { capture: a[0] === "metadata" }).ok) {
        restore();
        throw new Abort(`"${cmd} ${a.join(" ")}" failed; the version and changelog changes were undone`);
      }
    }

    try {
      git("add", ...changed, "Cargo.lock");
      git("commit", "-q", "-m", `chore(release): v${next}`);
    } catch (e) {
      restore();
      throw e;
    }
    git("tag", "-a", "--cleanup=verbatim", `v${next}`, "-m", `v${next}\n\n${section}`);
    console.log(`\nCommitted and tagged v${next}.`);

    if (args.githubBuild) {
      console.log("Nothing was pushed. To upload it, and have GitHub build and publish the release:");
      console.log(`\n  ${pushCommand(next)}\n`);
      return 0;
    }
    await publish(ask, next, section, args.prerelease);
    console.log(`\nDone: v${next}.`);
    return 0;
  } finally {
    rl.close();
  }
}

main()
  .then((code) => process.exit(code))
  .catch((e) => {
    console.error(`\nrelease: ${e instanceof Abort ? e.message : e.stack}`);
    process.exit(1);
  });
