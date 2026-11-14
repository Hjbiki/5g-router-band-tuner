// The Release workflow's helper. It runs on GitHub after the installers are built, never on a PC:
//
//   check-tag <tag>   stops the run when the tag isn't the version in the manifests, or the
//                     changelog has no entry for it
//   collect           copies the installers to target/release-upload, adds the fixed-name copy
//                     the README's Download button links to and the app's own exe as the
//                     portable copy, and writes the release notes (the changelog entry and
//                     each file's SHA-256) beside them

import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  APP_EXE,
  cargoWorkspaceVersion,
  installersFor,
  publishedNotes,
  releaseSection,
  STABLE_PORTABLE_NAME,
  STABLE_SETUP_NAME,
  stableSetupSource,
  tagMatchesVersion,
} from "./lib.mjs";

const ROOT = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const OUT = join(ROOT, "target", "release-upload");

function fail(message) {
  console.error(`release: ${message}`);
  process.exit(1);
}

const version = cargoWorkspaceVersion(readFileSync(join(ROOT, "Cargo.toml"), "utf8"));
const entry = releaseSection(readFileSync(join(ROOT, "CHANGELOG.md"), "utf8"), version);
const [mode, tag] = process.argv.slice(2);

if (mode === "check-tag") {
  if (!tagMatchesVersion(tag ?? "", version)) fail(`the tag is ${tag} but the manifests say ${version}`);
  if (!entry) fail(`CHANGELOG.md has no entry for ${version}`);
  console.log(`${tag} is the release tag of version ${version}`);
} else if (mode === "collect") {
  const built = [];
  for (const dir of ["target/release/bundle/nsis", "target/release/bundle/msi"]) {
    const abs = join(ROOT, dir);
    if (!existsSync(abs)) continue;
    for (const name of installersFor(readdirSync(abs), version)) built.push(join(abs, name));
  }
  if (!built.length) fail(`no installers for ${version} in target/release/bundle`);
  const setup = stableSetupSource(built);
  if (!setup.source) fail(`${setup.problem}: the README's Download button needs exactly one`);
  const app = join(ROOT, APP_EXE);
  if (!existsSync(app)) fail(`no ${APP_EXE}: the README's portable link needs the app's own exe`);

  // The build folder comes back from the workflow's cache, and with it whatever an earlier build
  // collected: start empty, so only this version's files are uploaded.
  rmSync(OUT, { recursive: true, force: true });
  mkdirSync(OUT, { recursive: true });
  const files = [];
  const put = (from, name) => {
    copyFileSync(from, join(OUT, name));
    files.push({ name, sha256: createHash("sha256").update(readFileSync(from)).digest("hex") });
  };
  for (const f of built) put(f, basename(f));
  put(setup.source, STABLE_SETUP_NAME);
  // The app's own exe runs without installing: the portable download.
  put(app, STABLE_PORTABLE_NAME);
  // A trial build of a commit that isn't a release has no changelog entry of its own.
  const notes = publishedNotes(entry ?? `Trial build of version ${version}. Not a release.`, files);
  writeFileSync(join(OUT, "notes.md"), notes);
  for (const f of files) console.log(`${f.name}  sha256 ${f.sha256}`);
} else {
  fail("usage: ci-release.mjs check-tag <tag> | collect");
}
