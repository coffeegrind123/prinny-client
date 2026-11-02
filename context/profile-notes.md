# Profile notes (MSC4441 + MSC4483)

Private, per-user notes on other people's profiles, like Discord's "Note"
field. Only the author can read them; they sync across the author's devices.

## Spec status (checked 02.11.2026)

| MSC | What | State |
|---|---|---|
| [MSC4441](https://github.com/matrix-org/matrix-spec-proposals/pull/4441) User Profile Annotations | `m.profile_annotations` account data: `{ "<mxid>": { "m.text": [MSC1767 representations], ...unknown } }` | open, author asked for crypto review before implementations |
| [MSC4483](https://github.com/matrix-org/matrix-spec-proposals/pull/4483) Encrypted Account Data | ADK secret in 4S; account data encrypted with `m.secret_storage.v1.aes-hmac-sha2`, HKDF info = the account data type | open |

The only other implementation is Voyage (Android, `VoyageClient/Voyage@fdd0308`).
Ours matches it on every wire detail:

| | identifier |
|---|---|
| annotations (read stable first, write both) | `m.profile_annotations`, `dev.zirco.msc4441.profile_annotations` |
| ADK secret (read either, write both) | `m.account_data.key`, `dev.zirco.msc4483.account_data.key` |
| ADK | 32 random bytes, unpadded base64; the 4S secret value is that string |
| encrypted content | `{ "encrypted": { iv, ciphertext, mac } }` over `JSON.stringify(annotations)`, info = the type it is stored under (so the stable and unstable copies are two different ciphertexts) |
| note body | `[{mimetype:"text/html", body}, {body: <typed text>}]`, HTML only when markdown changed something |

Interop was verified against an independent Python implementation of the 4S
algorithm (both directions, padded and unpadded base64, wrong-info rejection).

## Code

| File | Role |
|---|---|
| `cinny/src/app/utils/profileNotes.ts` | Pure: types, MSC4483 encrypt/decrypt (js-sdk's `encryptAESSecretStorageItem`), reading/merging `m.text`, size check |
| `cinny/src/app/utils/profileNotesService.ts` | One per `MatrixClient`: loads + decrypts, finds the ADK, serialised writes |
| `cinny/src/app/hooks/useProfileNotes.ts` | `useSyncExternalStore` over the service |
| `cinny/src/app/components/user-profile/UserNote.tsx` | The "Note" section on the profile card (hidden on your own profile) |
| `cinny/src/app/components/UserRoomProfileRenderer.tsx` | Focus trap leaves Escape to the note editor |

## Decisions

- **Always encrypt when the account has secret storage**; write plaintext only
  when it has none (MSC4441 allows it), and say so in the editor. An existing
  plaintext map is encrypted on the next write.
- **Never make a device-local ADK.** Voyage does when the 4S key is not cached
  and uploads it later. Notes written meanwhile cannot be read by any other
  device, and if another client stores an ADK first, the two diverge. We ask
  for the recovery key instead. If we meet a Voyage device-local blob, the
  card says so (`LostReason.NoAdk`) and does not offer to delete it.
- **Never overwrite what we cannot read.** A write decrypts the current map
  first and refuses on failure. Replacing undecryptable notes is an explicit,
  confirmed "Start over".
- **ADK cache**: `localStorage["prinny.msc4483.adk:<mxid>"] = {adk, source}`,
  where `source` is the 4S ciphertext the ADK came from. When another client
  rotates the ADK, the ciphertext changes and the cache is ignored. Logout
  clears localStorage. The unencrypted rust crypto store next to it already
  holds the cross-signing private keys, so this adds no new exposure.
- **4S key**: whatever the session already has in `secretStorageKeys.js`
  (entered for verification or restore), or entered on the note card itself.
  Checked through the exported `cryptoCallbacks.getSecretStorageKey`.
- **Size**: 1024 characters per note; the whole encrypted map must stay under
  60 KiB, checked before sending.
- **Races**: account data is last-write-wins (MSC4441 accepts that). Writes in
  this client are serialised; each one re-reads the map just before writing.

## Lost states (`LostReason`)

| reason | when | offered |
|---|---|---|
| `no-adk` | encrypted, no ADK in 4S | explanation only (likely a Voyage device-local key not yet uploaded) |
| `stale-recovery-key` | ADK in 4S only under a replaced recovery key | Start over |
| `wrong-adk` | 4S ADK fails the MAC | Start over |
| `no-secret-storage` | encrypted, account has no 4S | Start over |
| `corrupt` | decrypts, not a JSON object | Start over |
