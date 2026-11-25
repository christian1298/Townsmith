---
title: Remote Transport and Authentication
project: Townsmith
status: proposed
updated: 2026-11-25
tags:
  - townsmith
  - multiplayer
  - security
  - networking
  - planning
---

# 83 — Remote Transport and Authentication

> [!abstract] What this note is
> A design proposal for the Phase 6 **remote transport** and **authentication** contract: TLS, account identity, server-issued session tokens, resume rotation, rate limits, bans, and audit. It is planning only. No TLS listener, account store, token issuer, or ban list exists.
>
> It does not change the protocol v1 shapes, the loopback smokes, `ServerWorld.content_hash`, or any accepted ADR (D-004, D-021, D-032, D-033, D-034, D-035, D-040). It proposes additive commands and a transport mode that are gated on D-041 acceptance.

Related: [[05 - Multiplayer]], [[10 - Decisions]] D-041, [[09 - Open Questions]] Q-125/Q-126, [[12 - Technical Architecture]], [[78 - Code Audit Findings]] F-039, [[80 - Phase 6 Remaining Work]], [[82 - Journal and Rollback]].

## 1. Problem

The current server is a **loopback/debug** path, not a remote service. It is authoritative (D-004) and deterministic (D-020), but everything that makes a public or friend-hosted session safe is missing:

| Need | Today | Gap |
|---|---|---|
| Transport | Plain TCP, length-prefixed JSON | No TLS, no certificate validation, no hostname check; any on-path peer can read or alter traffic |
| Peer identity | `hello { protocol_version, player_name, content_hash }` | A client chooses any display name; a name is a label, not an account; two accounts cannot be told apart |
| Player identity | `PlayerRegistry` resolves one stable `PlayerId` per case-insensitive display name per save | The mapping is per-save and name-keyed; it does not survive a move to another save and cannot be owned by an account |
| Resume | `session_token` from `welcome`, reused by `resume { session_id, last_tick }` | The token is `session_id_for_seed(seed)` — derivable, constant for the default seed, and handed to every client. It is a reconnect handle, not authentication (F-039) |
| Revocation | None | A leaked resume token is valid until the process exits or the 300-tick window expires; there is no logout, kick, or ban that invalidates it |
| Abuse control | Per-session token bucket for selected commands; `resume` is exempt (F-039) | An unauthenticated peer can hammer hello/resume; there is no per-IP or per-account budget |
| Bans | None | A banned player can reconnect with a new name and a fresh connection |
| Audit | 256-entry per-session ring behind Admin `audit_log` | Auth successes/failures, bans, revocations, and TLS failures are not recorded |

F-039 in [[78 - Code Audit Findings]] is the audit gap this proposal closes: resume must be rate-limited and must stop being an authentication substitute.

> [!warning] Proposal status
> D-041 is **Proposed**. Nothing here is implemented. The protocol shapes described as "additive" do not exist yet; this doc does not change the frozen v1 frames or the existing loopback transcripts.

## 2. Threat model

The v1 contract is scoped to a self-hosted server that a small group can reach over the internet or a LAN. It is not an MMO edge, a matchmaking service, or an anti-cheat system.

| # | Threat | Today | Proposed control |
|---|---|---|---|
| T1 | **Spoofed hello or display name** — a client claims another player's name | Names are free-form labels; a duplicate is rejected across connections (`player_name.duplicate`, F-040 fixed) but nothing ties a name to an owner | A display name is claimable only through an authenticated account; a different account receives `auth.invalid` or a name-conflict code and never inherits the `PlayerId` |
| T2 | **Stolen resume token** — a token copied from a log, a crash dump, or another client | Bearer token, derivable from the seed, valid for the process lifetime | Opaque random token, hashed at rest, short absolute expiry, channel-bound to the TLS connection, rotated on every resume, reuse detected and the session family revoked |
| T3 | **Replay** — re-send a captured auth or resume frame | No nonce, no expiry, no generation | Server nonce plus client nonce in the auth transcript; resume generation counter; `last_tick` must fall inside the retained window |
| T4 | **MITM / eavesdropping** — read or alter game traffic | Plain TCP | TLS with certificate and hostname validation; no plaintext fallback off loopback |
| T5 | **Malformed frames** — oversized length, bad JSON, truncated body, bad UTF-8 | Length cap and validation exist in protocol v1 (F-001 fixed) | Keep the existing bounded framing; apply the same bounds before auth, close on violation, count the rejection |
| T6 | **Resource exhaustion** — connection flood, slow handshakes, auth brute force | No pre-auth budget | Per-IP and per-account token buckets, pre-auth connection cap, handshake deadline, bounded pending-auth table, global accept budget |
| T7 | **Banned player reconnect** — same person, new name or new connection | Nothing to ban | Ban checked after account identification and again on resume; `auth.banned` before a token is issued |
| T8 | **Mod/content mismatch** — a client with a different pack joins | `--content-hash` hello pin rejects a mismatch before player registration (D-034) | Keep the pin and its fail-closed order; negotiate version/content before auth, then authenticate |
| T9 | **Server impersonation** — a fake server harvests credentials | Nothing verifies the server | Clients validate the certificate chain and hostname; self-hosted servers may pin a certificate fingerprint in the server entry, with a visible change warning |
| T10 | **Credential stuffing / password guessing** | No accounts | Argon2id password hashes, uniform `auth.invalid`, per-IP/per-account limits, lockout backoff, audit entries without passwords |

Out of scope for this threat model: DDoS scrubbing, kernel-level cheats, a malicious server operator (the operator owns the world by definition), and traffic analysis.

## 3. Design principles

1. **Encrypt every non-loopback byte.** TLS is required for any bind address that is not loopback. There is no implicit downgrade and no plaintext fallback by default.
2. **Authenticate the account, not the name.** A display name is presentation. The account is identity; the per-save `PlayerId` is the game-facing key.
3. **The server issues and owns every token.** Clients never mint, extend, or choose a token, expiry, account id, or role.
4. **Tokens are opaque and revocable.** A token the server cannot revoke is not a session token. The server stores a hash, never the token itself.
5. **Fail closed.** Missing certificate, invalid config, unavailable account store, or a failed ban check refuses the connection. A degraded auth mode is an explicit operator choice, never silent.
6. **Cheap rejects first.** Version and content negotiation run before authentication so a mismatched or wrong-version client is rejected without spending password or ticket verification.
7. **No secrets in logs.** Tokens, passwords, Steam tickets, and private keys never enter logs, metrics labels, audit entries, saves, or the journal.
8. **Keep the loopback path working.** Existing smokes and the single-player local server keep their plaintext loopback mode and their pinned transcripts.
9. **Additive protocol only.** Auth commands and fields are new; the v1 frames stay valid until a versioned change is accepted.

## 4. Proposed v1 contract

### 4.1 Transport

| Item | Proposal |
|---|---|
| Library | **rustls** (pure Rust, no OpenSSL dependency) with `tokio-rustls`; a platform-TLS backend may be added behind the same trait if a target needs it |
| Bind | `--listen HOST:PORT` (explicit host, not just port); `--tls-cert PATH --tls-key PATH`; TLS is mandatory when the host is not loopback |
| Minimum version | TLS 1.2 floor, TLS 1.3 preferred; no renegotiation; no compression |
| ALPN | `townsmith/1` |
| Certificate | PEM chain plus PKCS#8 key; startup validates the pair and fails closed on mismatch or expiry |
| Client validation | System trust store by default; an optional per-server fingerprint pin for self-hosted deployments; hostname required |
| Loopback | Plaintext remains allowed only for an explicit loopback bind (`127.0.0.1`/`::1`) in dev/loopback mode; it is not a fallback and cannot be enabled by a client request |
| Proxy | A TLS-terminating proxy is an operator option, not the default; when enabled, the process must trust a documented header only from a configured proxy address (Q-126) |

### 4.2 Account identity provider

One abstraction, `AccountProvider`, resolves an authentication request to an `AccountId` or a uniform failure:

| Method | v1 status | Notes |
|---|---|---|
| `local` account file | **First** | JSON or line-oriented file of `{ account_id, display_name, argon2id_hash, salt, created_at, disabled }`; file mode `0600`; no default passwords; an operator provisions accounts with a CLI helper |
| Steam ticket | Later | `steam:<steamid64>`; ticket verification behind the same provider; no Workshop auth |
| OpenID / OAuth | Later | Explicitly not at launch; adding one is a new provider, not a protocol change |
| Server console / operator token | Optional | Local-only break-glass admin; audited separately; never a network credential |

`AccountId` is namespaced: `local:<uuid>`, `steam:<steamid64>`. The namespace is part of the id so a provider migration is explicit and never silently reuses an id.

### 4.3 Session token

- 32 random bytes from the OS CSPRNG, encoded base64url; shown to the client once.
- Server-side record: `BLAKE3(token)` plus claims `{ account_id, server_id, issued_at, expires_at, generation, session_family, bound_channel }`. The plaintext token is never stored.
- `server_id` is a stable, non-secret instance id persisted with the server configuration/save. It prevents a token from one server being replayed against another; a mismatch is `server.identity_mismatch`.
- `bound_channel` is a TLS exporter value (RFC 9266) plus the client nonce from the auth transcript. A token presented on a different TLS connection fails `auth.invalid`. Binding is disabled only when TLS terminates at a trusted proxy, and that mode is visible in config and audit (Q-126).
- Absolute expiry is enforced server-side. Client clocks are never authoritative.
- The token maps to an account; the server resolves the per-save `PlayerId` on every welcome. The token never carries a `PlayerId`, role, or permission bit.

### 4.4 Resume rotation

- Every successful `resume` issues a new token (generation + 1) and invalidates the previous generation.
- A presented token older than the current generation is **reuse**: the server revokes the whole `session_family`, audits it, and returns `resume.revoked`. A lost rotation response requires a fresh authentication; there is no silent replay grace.
- Resume also requires the session to be inside the retained window and the `last_tick` to be at or before the last snapshot tick. A gap forces a full snapshot resync (D-040's rule); the token rotation does not extend the window.
- Disconnect, `logout`, Admin kick, ban, server restart, and token expiry all invalidate resume. Resume state remains process-local in v1, matching today's behavior.

### 4.5 Rate limits and abuse budgets

| Budget | Suggested default | Applies to |
|---|---|---|
| Handshake attempts | 10/min per IP, 5/min per account | `hello`, `auth_*`, `resume` before authentication |
| Concurrent pre-auth connections | 16 per IP, 256 global | New sockets that have not completed auth |
| Handshake deadline | 10 s | TLS + hello + auth |
| Authenticated commands | Existing per-session token bucket | Unchanged |
| Failed logins | Exponential backoff after 5 failures per account; 15 min ceiling | `local` provider |
| Ban check | Every connect and every resume | No cache that outlives a ban update |

Budgets are configuration, not protocol constants. A rate-limited request returns `auth.rate_limited` (or `tls.required` before TLS) and is counted; it never leaks whether an account exists.

### 4.6 Bans and revocation

- A server-local `bans.json` keyed by `account_id`, with optional `expires_at`, `reason`, `created_by`, and `created_at`. An optional IP/network entry is a separate, clearly labeled list; it is not account identity.
- Admin commands `ban`, `unban`, and `list_bans` (names illustrative) are additive. A ban is checked after account identification, before token issue, and again on resume.
- A ban revokes every live token for that account and closes its connections with `auth.banned`.
- An explicit `logout` or Admin `kick` revokes the session family immediately. Deleting the account record disables future logins but does not rewrite the save; the `PlayerId` remains for audit.

### 4.7 Audit

- Auth success, auth failure (code only), ban/unban, kick, token rotation failure, reuse detection, TLS handshake failure, and config change produce durable audit entries with `{ tick_or_wallclock, account_id, remote_addr, code }`.
- Tokens, passwords, tickets, and key material are never recorded. Chat and personal text are never part of an auth entry.
- General logs redact tokens and credentials; remote addresses are kept in the audit path, not in metrics labels.

## 5. Identity mapping

```
account_id  --(provider)-->  authenticated account
     |
     +-- persisted link in ServerSave --> per-save PlayerId (D-033)
     |
     +-- display_name (label, unique per save, changeable by its account)
```

- `AccountId` is global to the deployment; `PlayerId` is per save. The link is an optional additive `ServerSave` field, so a world that never enables accounts stays byte-identical (same rule as D-033/D-034).
- On first authenticated join, the server either finds the existing `account_id -> PlayerId` link or creates a new `PlayerId` and records the link. A name alone never creates or claims a link.
- Reconnect with a valid token resolves the same `PlayerId` even if the display name changed. The name change is audited; settlement ownership and permissions (D-033) stay with the `PlayerId`.
- A different account that asks for an already-linked display name is rejected; it cannot take over the name or the id. An Admin rename may move a label, never the link.
- If the account store is unavailable, the server refuses authenticated joins. It does not fall back to name-keyed identity.

> [!note] Relation to D-033
> D-033 already gives the server a stable `PlayerId` per case-insensitive display name and persists it. This proposal adds the missing owner above that id. It does not change settlement commands, permission bits, or save checksums.

## 6. Handshake order

**Choice: negotiate version and content first, then authenticate, then issue a session.** This keeps the existing fail-closed order and spends expensive verification only on clients that could actually join.

| Step | Frame (proposed unless marked existing) | Server behavior |
|---|---|---|
| 1 | TCP + TLS | Validate certificate/key config; reject plaintext off loopback with `tls.required`; handshake failure closes with `tls.handshake_failed` |
| 2 | `hello { protocol_version, player_name, content_hash }` — **existing shape** | Validate version and the `ServerWorld.content_hash` pin before registering a player. A mismatch is rejected without auth work. With auth enabled, a valid hello returns `auth.required` instead of `welcome`; it issues no token |
| 3 | `auth_begin { account_id, method }` — additive | Look up the account, apply the per-IP/per-account budget; return `auth_challenge { server_nonce }` or a uniform `auth.invalid` |
| 4 | `auth_proof { client_nonce, proof }` — additive | Verify the password/ticket over the TLS channel; bind the transcript to both nonces and the TLS exporter; on success issue `welcome { session_id, session_token, account_id, player_id, tick, protocol_version, resumed:false }` |
| 5 | `resume { session_id, last_tick }` — **existing frame, token now required** | Present the token in the resume credential field; check account, server id, expiry, generation, ban, and window; rotate the token and return a resumed `welcome` |
| 6 | Normal commands | Existing executor, roles, and per-session token bucket unchanged |

**Why negotiation before auth:**

- It preserves the existing `--content-hash` behavior (#331): a mismatched client is rejected before any player or session state is created.
- Version/content data is not secret; the server pin is already observable by any peer that can reach the port, and the protocol version is advertised in every hello.
- It protects the auth budget: a wrong-version or wrong-content client never reaches password verification.
- It keeps the security boundary where it belongs: no `PlayerId`, session token, role, or world state exists until auth succeeds.

The cost is that an unauthenticated scanner learns the protocol version and content hash. If an operator considers that unacceptable, an `auth_first` config can require authentication before content negotiation; it is not the default because it breaks the existing fail-closed placement/content flow and the loopback goldens.

## 7. Failure codes

Stable, bounded strings; no account detail and no token detail in the text.

| Code | When | Notes |
|---|---|---|
| `auth.required` | A valid hello on an auth-enabled server, or a command before auth | Not an error for a well-behaved client; it is the transition to `auth_begin` |
| `auth.invalid` | Unknown account, wrong password/ticket, bad proof, token not bound to this channel | Uniform for unknown account and wrong secret; no enumeration |
| `auth.expired` | The session token's absolute expiry passed | The client must authenticate again; resume cannot extend expiry |
| `auth.banned` | The account is on the ban list | Returned at connect and at resume; the connection closes |
| `auth.rate_limited` | Per-IP/per-account budget exhausted | Includes the retry-after hint; counted, not audited per attempt beyond a sampled entry |
| `tls.required` | Plaintext on a non-loopback bind, or a client that skipped TLS | The listener may also close without a frame; the code exists for the loopback/proxy edge |
| `tls.handshake_failed` | Certificate mismatch, unsupported version, bad SNI/ALPN | Never includes key material or the peer certificate body |
| `resume.revoked` | Old token generation, logout/kick, ban-revoked family, or server restart | The client must authenticate again; the family is dead |
| `server.identity_mismatch` | Token `server_id` differs from this instance | Prevents cross-server replay after a save or config move |

Existing codes (`hello.*`, `content_hash.*`, `role.forbidden`, `rate_limited`, `resume.*`) are unchanged. New codes are additive and frozen only when D-041 is accepted.

## 8. Non-goals

- **Public server browser or matchmaking.** Discovery, NAT punch-through, and listing services are separate products.
- **Steam Workshop authentication or entitlement checks.** Steam identity may arrive as an `AccountProvider`; Workshop auth does not.
- **Account recovery, password reset email, MFA, or parental controls.** An operator resets a local account out of band.
- **OAuth providers at launch.** No Discord/Google/Epic login in v1.
- **End-to-end encryption of game state beyond TLS.** The server is authoritative and must read every command; there is no client-to-client secrecy layer.
- **Anti-cheat, kernel attestation, and DDoS scrubbing.** Out of scope for a self-hosted co-op game.
- **A second identity system in the client UI.** The existing name field stays a label; account login is the gate.
- **Changing protocol v1 frames or the loopback transcripts.** All auth commands are additive.

## 9. Security and operations

**Secret storage**

- TLS private keys and the local account file are files with mode `0600`, referenced by path, never embedded in the save, a mod, or a blueprint.
- The account file stores Argon2id hashes with per-account salts; no plaintext or reversible password material.
- The server refuses to start with TLS enabled if the certificate, key, or trust configuration is missing or unreadable. It never generates a self-signed certificate silently.
- Tokens exist in memory as hashes plus claims; only the hash is persisted if resume ever becomes restart-persistent (it is not in v1).
- No secret is printed by `--help`, config dumps, audit, metrics, or crash messages.

**Certificate rotation**

- Reload the certificate/key on `SIGHUP` (or an Admin command) without dropping established TLS sessions; new handshakes use the new pair.
- Keep the old certificate valid for an overlap window; publish expiry and remaining-days metrics.
- ACME is a later integration; for v1, operators rotate files and reload.
- A failed reload keeps the old pair and logs an error; it never stops serving with a broken certificate.

**Logs**

- Structured fields, stable codes, and an explicit redaction list: token, password, ticket, private key, authorization header.
- General logs may include a truncated remote address; full addresses live in the audit path with a retention setting.
- Failed auth logs the code and the account id, never the attempted password or the token.

**Metrics**

- Counters: `auth_ok`, `auth_fail{code}`, `tls_fail{reason}`, `resume_rotated`, `resume_reuse_detected`, `banned_attempt`, `rate_limited{scope}`.
- Gauges: active authenticated sessions, pending pre-auth connections, certificate expiry seconds.
- No account ids, player names, or IP addresses as metric labels (high cardinality and privacy).

**Graceful shutdown**

- Stop accepting new connections, close the listener, send a best-effort close frame, finish in-flight commands, write the atomic save, then exit.
- Ephemeral session tokens are dropped. Because resume is process-local, a restart invalidates every token and requires authentication; that is current behavior, not a regression.
- A shutdown marker in the audit log records the reason (signal, Admin, fatal error).

**Clock skew**

- Expiry, `issued_at`, and ban windows use the server clock only.
- Auth transcripts carry both nonces and an optional client timestamp; the server tolerates ±5 minutes of client skew for display/diagnostics and rejects a transcript older than the handshake deadline.
- A skew warning metric and a startup check (NTP/offset) help operators; a client clock never extends a token.

## 10. Test plan

Unit and integration tests are deterministic; no test opens a real remote socket.

| Level | Cases |
|---|---|
| Unit — tokens | Claims encode/decode; absolute expiry at the boundary; wrong `server_id`; generation increment; family revocation on reuse; hash-at-rest round trip; no plaintext token in `Debug` output |
| Unit — accounts | Argon2id verify; unknown account and wrong password return the same `auth.invalid`; disabled account; namespace parsing (`local:`/`steam:`); account-file validation errors |
| Unit — bans | Active/expired ban; ban on connect; ban on resume; unban; ban revokes live family; IP list is separate from account list |
| Unit — limits | Per-IP and per-account buckets; handshake deadline; pre-auth cap; retry-after monotonic with an injected clock |
| TLS integration | Handshake with a test CA (`rcgen`); wrong hostname; expired certificate; untrusted chain; TLS 1.1 rejected; plaintext off loopback rejected with `tls.required` |
| Resume | Rotation issues a new token; old token reuse revokes the family (`resume.revoked`); expired token (`auth.expired`); `last_tick` gap forces resync; restart invalidates resume |
| Replay | Replayed `auth_proof` with a stale nonce fails; token copied to a second connection fails channel binding; captured frame replayed after rotation fails |
| Identity | Same account reconnects to the same `PlayerId`; different account cannot claim the name; name change keeps the id; account link survives save/load; base-only save stays byte-identical |
| Malformed frames | Oversized length, truncated body, invalid JSON/UTF-8, unknown auth method, auth frame before hello, hello after auth — all close or reject with a bounded code and no panic |
| Load | 64 concurrent authenticated connections on loopback/TLS with a fixed script: no unbounded memory, handshake p95 within budget, per-session state isolated |
| Determinism | Fake clock, in-memory account store, and an injected transport for every unit test; no wall-clock sleeps, no real DNS, no network in `cargo test` |

Existing pins must stay green: protocol v1 loopback goldens (`SERVER_PROTOCOL`, `SERVER_AUTHORITY`, `SERVER_ROLES`, `SERVER_RESUME`), the interest/vote/placement smokes, and `ServerWorld.content_hash` behavior. The auth path is exercised by new tests plus one loopback TLS harness, not by changing the old transcripts.

## 11. Existing pins and compatibility

| Pin | Status under this proposal |
|---|---|
| Protocol v1 framing and validation | Unchanged. Auth frames are additive and only active when auth is configured |
| Loopback smokes and `--loopback-smoke` / `--resume-smoke` | Unchanged; they remain plaintext loopback and keep their checksums |
| `ServerWorld.content_hash` hello pin (#331) | Unchanged and still fail-closed before player registration |
| `resume` token semantics | Changed only in auth-enabled mode: the token becomes a credential and rotates. Loopback mode keeps the current handle |
| D-004 (authoritative server) | Unchanged; auth decides who may connect, not what the server accepts |
| D-021 (time control) | Unchanged; role checks and votes keep their current model |
| D-032 (server placement) | Unchanged; an authenticated `PlayerId` feeds the existing role and settlement checks |
| D-033 (settlement ownership) | Unchanged; the account link sits above the persisted `PlayerId` |
| D-034/D-035 (content identity) | Unchanged; negotiation stays before auth and preserves fail-closed load/join |
| D-040 (journal) | Unchanged; auth and ban events are audit entries, not journal command entries |

> [!important] No protocol shape changes here
> This doc does not add fields to `hello`, `welcome`, `resume`, or any world command. `auth_begin`/`auth_proof`/`auth_challenge`, the failure codes, and the token-in-resume credential are proposals for a later versioned change. Until D-041 is accepted and that change is specified, the frozen v1 shapes and checksums remain the contract.

## 12. Open questions

| ID | Question | Current recommendation |
|---|---|---|
| Q-125 | Which account provider ships at launch, and is a local account file enough? | Ship the local account file with Argon2id; treat Steam as the first later provider; no OAuth at launch |
| Q-126 | What are the token lifetime and renewal rules, and does TLS terminate in the process or at a proxy? | 24 h absolute token, rotation on every resume, reuse revokes the family; rustls in-process by default, proxy termination only with an explicit trusted-proxy config and documented channel-binding caveat |
| — (unfiled) | Is a ban scoped to one server or to an account across servers? | Server-local v1; a shared ban list needs an operator service and is not launch scope. File a Q when a second server deployment exists |
| — (unfiled) | Is plaintext allowed on a LAN, not just loopback? | No by default. An explicit `--allow-plaintext-lan` dev flag may exist, but it must warn and must never be reachable from the default config. File a Q if a LAN-only deployment ships |

The two unfiled items will take the next Q IDs if they remain open when D-041 is reviewed; they are listed here so the gap is visible rather than silently answered.

## 13. Review trigger

Accept D-041 before the first non-loopback listener, account file, or token issuer lands. Revisit when Q-125/Q-126 are answered, when a TLS-terminating proxy or shared ban list is proposed, when the protocol version changes, or when a deployment needs restart-persistent resume. If the contract cannot be implemented without changing a frozen v1 frame, stop and write a protocol-version ADR instead of extending this proposal.
