// Independent SCRAM-SHA256 (SCRAMJS-style) vector generator. Node built-ins only.
// Deterministic, synthetic values only. Writes crates/router-core/tests/fixtures/scram_vectors.json
import { pbkdf2Sync, createHmac, createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const hmac = (key, msg) => createHmac("sha256", key).update(msg).digest();

function compute({ password, first_nonce, salt, iterations, server_nonce }) {
  const authMessage = `${first_nonce},${server_nonce},${server_nonce}`;
  const salted = pbkdf2Sync(Buffer.from(password, "utf8"), Buffer.from(salt, "hex"), iterations, 32, "sha256");
  const clientKey = hmac("Client Key", salted);
  const storedKey = createHash("sha256").update(clientKey).digest();
  const clientSignature = hmac(authMessage, storedKey);
  const proof = Buffer.from(clientKey.map((b, i) => b ^ clientSignature[i]));
  const serverKey = hmac("Server Key", salted);
  const serverSignature = hmac(authMessage, serverKey);
  return { clientproof: proof.toString("hex"), serversignature: serverSignature.toString("hex") };
}

const rep = (s, n) => s.repeat(n);
const cases = [
  { name: "ascii_password", password: "correct horse", first_nonce: rep("0a1b2c3d", 8), salt: rep("5e", 32), iterations: 1000, tail: "SyntheticServerNonceA1" },
  { name: "arabic_password", password: "كلمة-سر-٢٠٢٦", first_nonce: rep("1f2e3d4c", 8), salt: rep("a7", 32), iterations: 1000, tail: "SyntheticServerNonceB2" },
  { name: "low_iterations", password: "x", first_nonce: rep("99aabbcc", 8), salt: rep("01", 16), iterations: 1, tail: "SyntheticServerNonceC3" },
];

const out = cases.map(({ tail, ...c }) => {
  const v = { ...c, server_nonce: c.first_nonce + tail };
  const r = compute(v);
  return {
    name: v.name, password: v.password, first_nonce: v.first_nonce, salt: v.salt,
    iterations: v.iterations, server_nonce: v.server_nonce, ...r,
  };
});

const here = dirname(fileURLToPath(import.meta.url));
const dest = join(here, "..", "crates", "router-core", "tests", "fixtures", "scram_vectors.json");
writeFileSync(dest, JSON.stringify(out, null, 2) + "\n");
console.log(`wrote ${dest}`);
