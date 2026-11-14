// Pure release logic: commit parsing, version bump, changelog text, version-file edits.
// No I/O, so tools/release/lib.test.mjs can cover it.

const TYPE_RE = /^(?<type>[a-z]+)(?:\((?<scope>[^)]*)\))?(?<bang>!)?:\s*(?<subject>.+)$/;

/** Commit types that never reach the changelog or decide a bump. */
export const NOISE = new Set(["chore", "docs", "test", "style", "ci", "build", "refactor"]);

/** Types that count; anything else is reported and ignored. */
export const KNOWN = new Set(["feat", "fix", "perf", ...NOISE]);

/** Changelog groups, in display order. */
export const GROUPS = [
  { title: "Added", types: ["feat"] },
  { title: "Fixed", types: ["fix"] },
  { title: "Improved", types: ["perf"] },
];

/**
 * Parses one commit message (subject line + body) as a Conventional Commit.
 * Returns null for subjects that don't follow the convention.
 * A `Changelog: <text>` footer (repeatable) overrides the user-facing line.
 */
export function parseCommit(message) {
  const [subjectLine, ...rest] = message.replace(/\r\n/g, "\n").split("\n");
  const m = TYPE_RE.exec(subjectLine.trim());
  if (!m) return null;
  const body = rest.join("\n");
  const breaking = Boolean(m.groups.bang) || /^BREAKING[ -]CHANGE:/m.test(body);
  const changelog = [...body.matchAll(/^Changelog:\s*(.+)$/gim)].map((x) => x[1].trim());
  return {
    type: m.groups.type,
    scope: m.groups.scope ?? null,
    subject: m.groups.subject.trim(),
    breaking,
    changelog,
  };
}

/** "major" | "minor" | "patch" | null (nothing releasable). */
export function decideBump(commits) {
  if (commits.some((c) => c.breaking)) return "major";
  if (commits.some((c) => c.type === "feat")) return "minor";
  if (commits.some((c) => c.type === "fix" || c.type === "perf")) return "patch";
  return null;
}

export function bumpVersion(version, bump) {
  const m = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (!m) throw new Error(`not a plain x.y.z version: ${version}`);
  let [major, minor, patch] = m.slice(1).map(Number);
  if (bump === "major") [major, minor, patch] = [major + 1, 0, 0];
  else if (bump === "minor") [minor, patch] = [minor + 1, 0];
  else if (bump === "patch") patch += 1;
  else throw new Error(`unknown bump ${bump}`);
  return `${major}.${minor}.${patch}`;
}

/** Old commits use the app's earlier name, and they can't be reworded now that they're pushed. */
function currentName(text) {
  return text.replace(/(?<!5G )Router Band Tuner/g, "5G Router Band Tuner");
}

/** Turns a commit subject into a changelog line: no prefix, capitalised, one sentence. */
export function humanize(subject) {
  let s = currentName(subject.trim().replace(/\s+/g, " "));
  s = s.charAt(0).toUpperCase() + s.slice(1);
  return /[.!?]$/.test(s) ? s : `${s}.`;
}

/** The lines a commit contributes to the changelog (none for noise). */
export function changelogLines(c) {
  if (NOISE.has(c.type)) return [];
  const lines = c.changelog.length ? c.changelog.map(humanize) : [humanize(c.subject)];
  return c.breaking ? lines.map((l) => `**Breaking:** ${l}`) : lines;
}

/** Markdown section for one release. */
export function changelogSection(version, date, commits) {
  const out = [`## [${version}] - ${date}`, ""];
  for (const g of GROUPS) {
    const lines = commits
      .filter((c) => g.types.includes(c.type))
      .flatMap(changelogLines)
      .filter((l, i, all) => all.indexOf(l) === i);
    if (!lines.length) continue;
    out.push(`### ${g.title}`, "", ...lines.map((l) => `- ${l}`), "");
  }
  // A breaking change of another type (e.g. refactor!) still has to be announced.
  const otherBreaking = commits
    .filter((c) => c.breaking && !GROUPS.some((g) => g.types.includes(c.type)))
    .map((c) => `- **Breaking:** ${humanize(c.changelog[0] ?? c.subject)}`);
  if (otherBreaking.length) out.push("### Changed", "", ...otherBreaking, "");
  return out.join("\n");
}

const CHANGELOG_HEADER =
  "# Changelog\n\nAll notable changes to 5G Router Band Tuner. Versions follow [Semantic Versioning](https://semver.org/).\n";

/** Inserts `section` as the newest entry of an existing (or empty) CHANGELOG.md. */
export function prependChangelog(existing, section) {
  const body = (existing ?? "").replace(/\r\n/g, "\n");
  if (!body.trim()) return `${CHANGELOG_HEADER}\n${section}`;
  const at = body.search(/^## /m);
  if (at < 0) return `${body.trimEnd()}\n\n${section}`;
  return `${body.slice(0, at)}${section}\n${body.slice(at)}`;
}

/** A hand-written CHANGELOG entry from docs/release-notes, with its date filled in. */
export function fillReleaseNotes(text, date) {
  return `${text.replace(/\r\n/g, "\n").replaceAll("{{date}}", date).trimEnd()}\n`;
}

/** The entry for `version` in a CHANGELOG, or null. */
export function releaseSection(changelog, version) {
  const body = changelog.replace(/\r\n/g, "\n");
  const at = body.indexOf(`## [${version}]`);
  if (at < 0) return null;
  const next = body.indexOf("\n## ", at);
  return `${(next < 0 ? body.slice(at) : body.slice(at, next)).trimEnd()}\n`;
}

/** Windows needs a shell for pnpm's .cmd only. A shell splits arguments at spaces and newlines. */
export function needsShell(cmd, platform) {
  return platform === "win32" && cmd === "pnpm";
}

/** Version in the `[workspace.package]` table of the root Cargo.toml. */
export function cargoWorkspaceVersion(text) {
  const table = /^\[workspace\.package\][^[]*/m.exec(text);
  const v = table && /^version\s*=\s*"([^"]+)"/m.exec(table[0]);
  if (!v) throw new Error("no version in [workspace.package]");
  return v[1];
}

export function setCargoWorkspaceVersion(text, version) {
  return text.replace(/^(\[workspace\.package\][^[]*?^version\s*=\s*")[^"]+(")/m, `$1${version}$2`);
}

/** First top-level "version" of a JSON file, edited as text to keep its formatting. */
export function jsonVersion(text) {
  const v = /^\s{2}"version"\s*:\s*"([^"]+)"/m.exec(text);
  if (!v) throw new Error('no top-level "version"');
  return v[1];
}

export function setJsonVersion(text, version) {
  return text.replace(/^(\s{2}"version"\s*:\s*")[^"]+(")/m, `$1${version}$2`);
}

/**
 * The installer's name on every release. The README's Download button links to this name on the
 * newest release, so the link never changes; the installers' own names carry the version.
 */
export const STABLE_SETUP_NAME = "5G-Router-Band-Tuner-Setup.exe";

/**
 * The one installer the fixed-name copy is made from: the NSIS setup, not the MSI. With none, or
 * with more than one (the release would be guessing what the README's button serves), `source`
 * is null and `problem` says why.
 */
export function stableSetupSource(files) {
  const setups = files.filter((f) => /-setup\.exe$/i.test(f));
  if (setups.length === 1) return { source: setups[0], problem: null };
  const problem = setups.length
    ? `${setups.length} setup .exe files match this version; delete the stale ones from target/release/bundle/nsis`
    : "no NSIS setup .exe among the installers";
  return { source: null, problem };
}

/** Whether a pushed tag is the release tag of the version in the manifests. */
export function tagMatchesVersion(tag, version) {
  return tag === `v${version}`;
}

/**
 * The installers of `version` among the file names of a bundle folder: the names carry the
 * number between underscores, so 1.0.0 never picks up 11.0.0 or 1.0.0-rc1.
 */
export function installersFor(names, version) {
  return names.filter((f) => f.includes(`_${version}_`) && /\.(exe|msi)$/i.test(f));
}

/** What a GitHub Release says: the changelog entry, then each attached file's SHA-256. */
export function publishedNotes(section, files) {
  const sums = files.map((f) => `- \`${f.name}\`: \`${f.sha256}\``).join("\n");
  return `${section.trim()}\n\n### SHA-256\n\n${sums}\n`;
}

/** The one command that uploads a tagged release: the branch and that tag, never every tag. */
export function pushCommand(version) {
  return `git push origin main v${version}`;
}
