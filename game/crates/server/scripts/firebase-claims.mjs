#!/usr/bin/env node
// Grant SurrealDB routing claims to Firebase users.
//
// SurrealDB routes a JWT to its access method from the token's own claims,
// so every player needs custom claims { ns, db, ac } on top of the standard
// iss/aud/sub. Firebase only mints iss/aud/sub — set the routing claims once
// per user with the Admin SDK (requires a real Firebase project; this cannot
// run against an emulator or in CI without credentials):
//
//   cd scripts/firebase && npm install
//   GOOGLE_APPLICATION_CREDENTIALS=/path/to/service-account.json \
//     node ../firebase-claims.mjs <uid> [<uid> ...]
//
// Users must refresh their ID token afterwards (sign out/in, or
// user.getIdToken(true)) before connecting the game client.

import admin from "firebase-admin";

const projectId = process.env.DUNGEON_FIREBASE_PROJECT_ID;
const ns = process.env.DUNGEON_SURREAL_NS ?? "party_dungeon";
const db = process.env.DUNGEON_SURREAL_DB ?? "main";
const ac = process.env.DUNGEON_SURREAL_AC ?? "firebase_jwt";
const uids = process.argv.slice(2);

if (!projectId || uids.length === 0) {
  console.error(
    "usage: DUNGEON_FIREBASE_PROJECT_ID=<id> node firebase-claims.mjs <uid> [<uid> ...]",
  );
  process.exit(1);
}

admin.initializeApp({
  credential: admin.credential.applicationDefault(),
  projectId,
});

for (const uid of uids) {
  await admin.auth().setCustomUserClaims(uid, { ns, db, ac });
  console.log(`claims set for ${uid}: { ns: ${ns}, db: ${db}, ac: ${ac} }`);
}
console.log("Done. Affected users must refresh their ID token before connecting.");
