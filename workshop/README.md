# Skin Workshop service

The desktop editor remains local. This optional service adds Steam OpenID sign-in,
an upload queue and a public catalogue. Publishing is controlled by
`PUBLISHING_ENABLED` in the deployment configuration. Set it to `false` and
redeploy to stop new uploads. No R2, paid Worker, larger runner or paid database is required.

## Trust boundaries

- Treat every desktop client as modified. Only server-verified Steam sessions can
  submit; all moderator decisions, bans and quotas are checked in the Worker.
- Pending lists are scoped to the authenticated uploader unless they are the
  configured moderator. Uploaders may cancel their own queued submissions and
  remove failed/cancelled entries; moderators can do so for any uploader.
  Removal retains quota usage and audit records. Failed cards show a recorded
  validation/transfer reason (older entries may have no recorded detail).
  Cancellation changes state before deleting draft assets;
  finalization rechecks state so a cancelled upload cannot enter the catalogue.
  Ban lists and cancel/ban/unban actions require server-side moderator access.
- Steam display names and approved-CDN avatar URLs are cached from the public
  profile XML for one hour. Failed refreshes are explicitly reported and retried
  after five minutes. Profile names never grant permissions.
- Steam completion redirects a one-time code to `127.0.0.1`, on the initiating
  app's ephemeral port and random path. Only that app can exchange it alongside
  its private verifier. No token or callback code is exposed through polling;
  creating a login link does not let someone collect another browser's session.
- GitHub App credentials and the validator secret never enter the desktop app.
  The App needs only Contents read/write, installed on
  `JoeyMarrey29/TextureHelper-Workshop`, not the application repository.
- A submission first becomes a **draft** GitHub release. The Worker streams at
  most 24 MiB into a single raw asset without decoding images on Workers Free.
  Draft releases require repository authorization; raw files are not published.
- A `repository_dispatch` workflow runs trusted validator code on a standard
  public-repository GitHub-hosted runner. Uploaded data is parsed as JSON/PNG,
  never evaluated, executed, extracted as an archive or used in shell commands.
- The validator strips metadata and private draft state, checks model/material
  ownership and aspect ratio against the generated catalogue, requires power-of-two
  texture dimensions up to 4096 pixels per side (not the original resolution), bounds PNG
  decompression, and substitutes a basic trusted material script.
  Existing 4 MiB compressed PNG and 24 MiB submission limits still apply.
- Only `skin.phskin` and `preview.png` may remain when the Worker publishes.
  Raw data is deleted first. The callback requires a separate secret; the Worker
  rechecks bans and submission state before exposing the catalogue entry.
- Failed or interrupted jobs are not success. Check recent submission status.
  Administrators must inspect and remove stale draft releases after infrastructure
  failures. Existing public downloads and caches cannot be recalled.

This is not a guarantee of harmless or legally shareable artwork. Automated
checks do not identify offensive images or establish ownership. CC BY consent is
for uploader-owned contributions; it does not relicense supplied game assets.

## Free-only deployment

1. Keep Cloudflare on **Workers Free** and **D1 Free**. Do not bind R2, enable
   paid Workers/D1, or configure paid overages. Free quota exhaustion should mean
   an outage, not an upgrade. Application quotas are abuse controls, not billing
   caps. The current caps are forty upload attempts/account/day and four thousand/day
   globally, resetting at midnight UTC. There is no lifetime upload cap.
   Failed and removed uploads still count toward that day's limits. Invalid
   metadata is rejected before consuming upload quota. These limits are starting
   abuse controls, not measured capacity guarantees; monitor requests, CPU and D1
   usage as the catalogue and browsing traffic grow.
2. Initialize the isolated **public** Workshop repository on branch `main`.
   Copy `src/images.js`, `src/package.js`, `src/catalog.json` under its `src/`,
   `publisher/validate.js` under `publisher/`, and
   `publisher/validate-skin.yml` to `.github/workflows/validate-skin.yml`.
   Include a `package.json` with `{"private":true,"type":"module"}`.
   Never add a workflow that runs uploaded content or a pull-request workflow
   with access to publishing secrets.
3. Use only the workflow's standard `ubuntu-latest` runner in that public
   repository. No larger runners or paid Actions services. Do not make the
   repository private without reconsidering the billing model.
4. Create a GitHub App with webhooks disabled and only Contents read/write.
   Install it on the Workshop repository alone. Enter `GITHUB_APP_ID`,
   `GITHUB_INSTALLATION_ID` and `GITHUB_APP_PRIVATE_KEY` as Cloudflare secrets.
   The key must be PKCS#8 (`BEGIN PRIVATE KEY`). If GitHub supplies PKCS#1,
   convert it locally with `openssl pkcs8 -topk8 -nocrypt -in <downloaded-key>
   -out <private-output>`; never print or commit either key.
5. Generate a random 32-byte value encoded as 64 lowercase hex characters.
   Set the same `VALIDATOR_SECRET` in Cloudflare and the Workshop repository's
   encrypted Actions secrets. Enter it through secure secret-entry tools; never
   paste it into chat or a tracked configuration file. Set
   `MODERATOR_STEAM_ID` as private service configuration.
6. From this directory, authenticate Wrangler, create the free D1 database,
   replace `CONFIGURE_BEFORE_DEPLOYMENT` in `wrangler.jsonc`, and run:

   ```powershell
   npx wrangler d1 migrations apply texture-helper-workshop --remote
   npx wrangler deploy
   ```

7. Leave `PUBLISHING_ENABLED` false until live Steam login, rejected signatures,
   bans, moderator permissions, a representative image queue, failed-job cleanup
   and Worker CPU usage have been checked. Obtain explicit consent before any
   real public sample upload. Then enable publishing and deploy again.

Workers Free's short CPU allowance is why image processing is queued, not
performed in the HTTP request. Local measurements alone do not prove a deployed
request fits that allowance; measure the deployed authentication, streaming and
finalization paths before enabling uploads. GitHub/Cloudflare may still impose
quotas, abuse restrictions or change their plans.

## Local checks

```powershell
npm ci
npm test
npx wrangler deploy --dry-run
```

From the repository root, run `python tools\test_workshop.py` and
`python tools\test_preview.py`, and run the Rust tests in `src-tauri`.
To regenerate the trusted path/dimension catalogue after a game content update,
set `PERP_TEST_CONTENT` to extracted content and run
`cargo test --offline generate_workshop_catalog -- --ignored` in `src-tauri`.
Deploy the same regenerated catalogue to the validator repository.
