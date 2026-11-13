import {boundedBytes} from '../src/images.js';
import {publicPackage} from '../src/package.js';
import {createHash} from 'node:crypto';
import {pathToFileURL} from 'node:url';

const REPO = 'JoeyMarrey29/TextureHelper-Workshop';
const ORIGIN = 'https://texture-helper-workshop.work-855.workers.dev';
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const encode = value => new TextEncoder().encode(JSON.stringify(value));
const safeFailures = new Set([
  'Choose an editable supported draft with 1–12 textures', 'Material belongs to an incompatible attachment',
  'Unsupported or duplicate workshop material', 'Textures require power-of-two dimensions with the approved aspect ratio',
  'Unsupported attachment slot', 'Unsupported attachment selection', 'Preview must be at most 1024 pixels per side',
  'Content exceeds its size limit', 'Missing PNG', 'Invalid PNG encoding or size', 'Invalid PNG signature',
  'Too many PNG chunks', 'Truncated PNG', 'Truncated PNG chunk', 'PNG checksum mismatch',
  'PNG must start with IHDR', 'Invalid PNG header', 'Workshop requires non-interlaced RGB/RGBA 8-bit PNGs up to 4096 x 4096',
  'Noncontiguous PNG data', 'Invalid PNG end', 'Unsupported PNG chunk', 'Missing PNG end',
  'Invalid PNG pixel length', 'Invalid PNG row filter', 'Sanitized package too large'
]);

export async function validateSubmission({id, releaseId, github, report}) {
  if (!/^[a-f0-9]{32}$/.test(id || '') || !Number.isSafeInteger(releaseId) || releaseId < 1) throw new Error('Invalid trusted dispatch');
  let phase = 'Could not read the queued upload from storage.';
  try {
    const release = await (await github(`/releases/${releaseId}`)).json();
    if (!release.draft || release.tag_name !== `skin-${id}`) throw new Error('Submission is not an unpublished draft');
    const metadata = JSON.parse(release.body);
    if (!/^7656119\d{10}$/.test(metadata.steamId || '') || metadata.license !== 'CC-BY-4.0'
      || typeof metadata.title !== 'string' || metadata.title.length > 80) throw new Error('Invalid trusted attribution');
    const assets = release.assets || [];
    if (assets.length !== 1 || assets[0].name !== 'submission.json' || assets[0].size < 1
      || assets[0].size > 24 * 1024 * 1024 || !Number.isSafeInteger(assets[0].id)) throw new Error('Unexpected raw assets');
    const raw = await github(`/releases/assets/${assets[0].id}`, {headers:{Accept:'application/octet-stream'}});
    const input = JSON.parse(new TextDecoder().decode(await boundedBytes(raw.body, 24 * 1024 * 1024)));
    phase = 'The uploaded images or skin data could not be validated. Re-export the textures as PNG and try again.';
    const prepared = await publicPackage({...input, title:metadata.title});
    const packageBytes = encode({format:'perpheads-texture-helper-skin', version:1,
      draft:{...prepared.draft, workshopSource:{id, steamId:metadata.steamId, license:'CC-BY-4.0',
        ...(metadata.parent ? {parent:metadata.parent, parentSteamId:metadata.parentSteamId} : {})}}});
    if (packageBytes.length > 24 * 1024 * 1024) throw new Error('Sanitized package too large');
    phase = 'Could not store the validated files or confirm publication. Try again later.';
    for (const [name, bytes, type] of [['skin.phskin', packageBytes, 'application/json'], ['preview.png', prepared.preview, 'image/png']]) {
      await github(`/releases/${releaseId}/assets?name=${name}`, {method:'POST', body:bytes, upload:true, headers:{'Content-Type':type}});
    }
    await github(`/releases/assets/${assets[0].id}`, {method:'DELETE'});
    await report({id, accepted:true, model:prepared.draft.modelPath, hash:hash(packageBytes), previewHash:hash(prepared.preview)});
  } catch (error) {
    try { await report({id, accepted:false, reason:safeFailures.has(error.message) ? error.message : phase}); }
    catch (reportError) { throw new AggregateError([error, reportError], 'Validation failed and server cleanup could not be confirmed'); }
    throw error;
  }
}

async function main() {
  const token = process.env.GITHUB_TOKEN, secret = process.env.VALIDATOR_SECRET;
  if (!token || !/^[a-f0-9]{64}$/.test(secret || '')) throw new Error('Validator credentials missing');
  const github = async (path, {upload = false, ...options} = {}) => {
    const response = await fetch(`https://${upload ? 'uploads' : 'api'}.github.com/repos/${REPO}${path}`, {
      ...options, headers:{Authorization:`Bearer ${token}`, Accept:'application/vnd.github+json',
        'X-GitHub-Api-Version':'2022-11-28', ...options.headers}, signal:AbortSignal.timeout(60000),
      redirect:path.startsWith('/releases/assets/') && options.method !== 'DELETE' ? 'follow' : 'error'
    });
    if (!response.ok) throw new Error(`GitHub validator request failed (${response.status})`);
    return response;
  };
  const report = async value => {
    const response = await fetch(`${ORIGIN}/internal/validated`, {method:'POST', redirect:'error',
      headers:{Authorization:`Bearer ${secret}`, 'Content-Type':'application/json'},
      body:JSON.stringify(value), signal:AbortSignal.timeout(60000)});
    if (!response.ok) throw new Error(`Validation result not acknowledged (${response.status})`);
  };
  await validateSubmission({id:process.env.SUBMISSION_ID, releaseId:Number(process.env.RELEASE_ID), github, report});
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(() => { console.error('Submission validation failed; inspect submission status and remove stale draft releases if necessary.'); process.exitCode = 1; });
}
