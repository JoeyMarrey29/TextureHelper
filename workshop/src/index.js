import {boundedBytes, base64} from './images.js';

const REPO = 'JoeyMarrey29/TextureHelper-Workshop';
const STEAM = 'https://steamcommunity.com/openid/login';
const NS = 'http://specs.openid.net/auth/2.0';
const encoder = new TextEncoder();
const now = () => Math.floor(Date.now() / 1000);
const random = () => crypto.randomUUID().replaceAll('-', '') + crypto.randomUUID().replaceAll('-', '');
const hex = bytes => [...bytes].map(value => value.toString(16).padStart(2, '0')).join('');
const sha = async text => hex(new Uint8Array(await crypto.subtle.digest('SHA-256', typeof text === 'string' ? encoder.encode(text) : text)));
const b64url = bytes => base64(bytes).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '');
const json = (value, status = 200) => new Response(JSON.stringify(value), {status, headers:{
  'Content-Type':'application/json', 'Cache-Control':'no-store', 'X-Content-Type-Options':'nosniff'
}});
class HttpError extends Error {
  constructor(status, message) { super(message); this.status = status; }
}
const fail = (status, message) => { throw new HttpError(status, message); };
const statement = (env, sql, ...values) => env.DB.prepare(sql).bind(...values);
async function body(request, max = 4096) {
  if (!request.body) fail(400, 'Missing request');
  if (Number(request.headers.get('Content-Length')) > max) fail(413, 'Request too large');
  try { return JSON.parse(new TextDecoder().decode(await boundedBytes(request.body, max))); }
  catch { fail(400, 'Invalid or oversized JSON request'); }
}
async function quota(env, key, limit, message) {
  const result = await statement(env, `INSERT INTO quotas(key,count,expires) VALUES (?,1,?)
    ON CONFLICT(key) DO UPDATE SET count=count+1 WHERE count < ? RETURNING count`,
    key, now() + 172800, limit).first();
  if (!result) fail(429, `${message} Resets at midnight UTC.`);
}
async function session(request, env) {
  const token = request.headers.get('Authorization')?.match(/^Bearer ([a-f0-9]{64})$/)?.[1];
  if (!token) fail(401, 'Sign in through Steam first');
  const entry = await statement(env, 'SELECT steam_id FROM sessions WHERE hash=? AND expires>?', await sha(token), now()).first();
  if (!entry) fail(401, 'Workshop session expired. Sign in again.');
  if (await statement(env, 'SELECT steam_id FROM bans WHERE steam_id=?', entry.steam_id).first()) fail(403, 'This Steam account is banned from workshop access');
  return entry.steam_id;
}
const admin = (id, env) => typeof env.MODERATOR_STEAM_ID === 'string' && id === env.MODERATOR_STEAM_ID;
export function steamProfile(xml, steamId) {
  const field = name => {
    const value = xml.match(new RegExp(`<${name}>([\\s\\S]*?)</${name}>`))?.[1]?.trim();
    if (!value) return '';
    if (value.startsWith('<![CDATA[') && value.endsWith(']]>')) return value.slice(9, -3);
    return value.replace(/&#(x[0-9a-f]+|\d+);|&(amp|lt|gt|quot|apos);/gi, (match, numeric, named) => {
      if (numeric) {
        const code = numeric[0].toLowerCase() === 'x' ? parseInt(numeric.slice(1), 16) : Number(numeric);
        return code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : '';
      }
      return {amp:'&',lt:'<',gt:'>',quot:'"',apos:"'"}[named.toLowerCase()];
    });
  };
  if (field('steamID64') !== steamId) throw new Error('Steam profile identity mismatch');
  const name = field('steamID').replace(/[\u0000-\u001f]/g, '').trim().slice(0, 100);
  const avatar = field('avatarMedium');
  if (!name || !/^https:\/\/avatars(?:\.(?:akamai|fastly|cloudflare))?\.steamstatic\.com\/[a-f0-9]+(?:_medium)?\.jpg$/.test(avatar)) {
    let host = 'invalid URL';
    try { host = new URL(avatar).hostname.replace(/[^a-z0-9.-]/g, '').slice(0, 100); } catch { /* Report malformed profile data below. */ }
    throw new Error(`Steam profile metadata unavailable (name ${name ? 'present' : 'missing'}, avatar host ${host})`);
  }
  // Use Steam's canonical avatar host so released clients can display every CDN variant.
  return {name, avatar:avatar.replace('https://avatars.cloudflare.steamstatic.com/', 'https://avatars.steamstatic.com/')};
}
async function profile(env, steamId) {
  let entry = await statement(env, 'SELECT name,avatar,expires FROM profiles WHERE steam_id=?', steamId).first();
  if (!entry || entry.expires < now()) {
    try {
      const response = await fetch(`https://steamcommunity.com/profiles/${steamId}/?xml=1`, {
        redirect:'manual', signal:AbortSignal.timeout(10000)
      });
      if (!response.ok) throw new Error(`Steam profile request failed (HTTP ${response.status})`);
      entry = steamProfile(new TextDecoder().decode(await boundedBytes(response.body, 128 * 1024)), steamId);
      await statement(env, `INSERT INTO profiles(steam_id,name,avatar,expires) VALUES (?,?,?,?)
        ON CONFLICT(steam_id) DO UPDATE SET name=excluded.name,avatar=excluded.avatar,expires=excluded.expires`,
        steamId, entry.name, entry.avatar, now() + 3600).run();
    } catch (error) {
      const detail = /^Steam profile (request failed \(HTTP \d{3}\)|identity mismatch|metadata unavailable \(name (present|missing), avatar host [a-z0-9 .-]{0,100}\))$/.test(error?.message || '')
        ? error.message : 'Steam profile request timed out or returned unreadable data';
      console.error('Steam profile refresh failed', {type:error?.name || 'UnknownError', detail});
      await statement(env, `INSERT INTO profiles(steam_id,name,avatar,expires) VALUES (?,NULL,NULL,?)
        ON CONFLICT(steam_id) DO UPDATE SET expires=excluded.expires`, steamId, now() + 300).run();
      return {name:entry?.name || null, avatar:entry?.avatar || null, profileError:`${detail}. Showing cached details or SteamID.`};
    }
  }
  return {name:entry.name, avatar:entry.avatar,
    ...(!entry.name ? {profileError:'Steam profile details are temporarily unavailable. Showing SteamID.'} : {})};
}
async function loginStart(request, env) {
  const input = await body(request);
  if (!/^[a-f0-9]{64}$/.test(input.challenge)) fail(400, 'Invalid sign-in challenge');
  if (!Number.isInteger(input.port) || input.port < 1024 || input.port > 65535
    || !/^\/workshop\/[a-f0-9]{64}$/.test(input.callbackPath || '')) fail(400, 'Invalid local app callback');
  const day = Math.floor(now() / 86400);
  const ip = request.headers.get('CF-Connecting-IP');
  if (!ip) fail(403, 'Cloudflare connection identity missing');
  await quota(env, `login-ip:${day}:${await sha(ip)}`, 30, 'Daily Steam sign-in limit for this connection reached (30 attempts).');
  await quota(env, `login-global:${day}`, 1000, 'Workshop daily Steam sign-in limit reached (1,000 attempts).');
  await env.DB.batch([
    statement(env, 'DELETE FROM logins WHERE expires<?', now()),
    statement(env, 'DELETE FROM nonces WHERE expires<?', now()),
    statement(env, 'DELETE FROM sessions WHERE expires<?', now()),
    statement(env, 'DELETE FROM quotas WHERE expires IS NOT NULL AND expires<?', now())
  ]);
  const id = random();
  await statement(env, 'INSERT INTO logins(id,challenge,loopback_port,callback_path,expires) VALUES (?,?,?,?,?)',
    id, input.challenge, input.port, input.callbackPath, now() + 600).run();
  return json({id, url:`${env.PUBLIC_ORIGIN}/auth/steam?id=${id}`});
}
async function steamStart(url, env) {
  const id = url.searchParams.get('id');
  if (!/^[a-f0-9]{64}$/.test(id || '')) fail(400, 'Invalid sign-in');
  const browser = random();
  const result = await statement(env, 'UPDATE logins SET browser_hash=? WHERE id=? AND expires>? AND browser_hash IS NULL RETURNING id',
    await sha(browser), id, now()).first();
  if (!result) fail(400, 'Sign-in already opened or expired; start again in the app');
  const target = new URL(STEAM);
  const fields = {'openid.ns':NS, 'openid.mode':'checkid_setup', 'openid.realm':`${env.PUBLIC_ORIGIN}/`,
    'openid.return_to':`${env.PUBLIC_ORIGIN}/auth/callback?id=${id}`,
    'openid.identity':'http://specs.openid.net/auth/2.0/identifier_select',
    'openid.claimed_id':'http://specs.openid.net/auth/2.0/identifier_select'};
  for (const [key, value] of Object.entries(fields)) target.searchParams.set(key, value);
  return new Response(null, {status:302, headers:{Location:target.href, 'Cache-Control':'no-store',
    'Set-Cookie':`workshop_login=${browser}; Path=/auth; HttpOnly; Secure; SameSite=Lax; Max-Age=600`}});
}
export function steamIdentity(url, origin) {
  const params = url.searchParams, id = params.get('id');
  for (const key of params.keys()) if (params.getAll(key).length !== 1) fail(400, 'Duplicate OpenID field');
  if (!/^[a-f0-9]{64}$/.test(id || '') || params.get('openid.ns') !== NS
    || params.get('openid.mode') !== 'id_res' || params.get('openid.op_endpoint') !== STEAM
    || params.get('openid.return_to') !== `${origin}/auth/callback?id=${id}`) fail(400, 'Invalid Steam response');
  const identity = params.get('openid.claimed_id') || '';
  const match = /^https:\/\/steamcommunity\.com\/openid\/id\/(7656119\d{10})$/.exec(identity);
  if (!match || params.get('openid.identity') !== identity) fail(400, 'Invalid Steam identity');
  const signed = new Set((params.get('openid.signed') || '').split(','));
  for (const key of ['op_endpoint', 'claimed_id', 'identity', 'return_to', 'response_nonce', 'assoc_handle']) {
    if (!signed.has(key)) fail(400, 'Incomplete signed Steam response');
  }
  const nonce = params.get('openid.response_nonce') || '';
  const time = Date.parse(nonce.slice(0, 20));
  if (!Number.isFinite(time) || Math.abs(Date.now() - time) > 5 * 60 * 1000 || nonce.length > 256) fail(400, 'Expired Steam response');
  return {id, steamId:match[1], nonce};
}
async function steamCallback(request, url, env) {
  const {id, steamId, nonce} = steamIdentity(url, env.PUBLIC_ORIGIN);
  const cookie = request.headers.get('Cookie')?.match(/(?:^|; )workshop_login=([a-f0-9]{64})(?:;|$)/)?.[1];
  if (!cookie) fail(403, 'Sign-in browser cookie missing');
  const login = await statement(env, 'SELECT id,loopback_port,callback_path FROM logins WHERE id=? AND browser_hash=? AND expires>? AND consumed=0 AND steam_id IS NULL',
    id, await sha(cookie), now()).first();
  if (!login) fail(403, 'Sign-in expired or browser does not match');
  const verify = new URLSearchParams();
  for (const [key, value] of url.searchParams) if (key.startsWith('openid.')) verify.set(key, value);
  verify.set('openid.mode', 'check_authentication');
  let response;
  try {
    response = await fetch(STEAM, {method:'POST', body:verify, redirect:'manual', signal:AbortSignal.timeout(15000)});
  } catch (error) {
    console.error('Steam verification request failed', {type:error?.name || 'UnknownError'});
    fail(502, 'Steam verification could not be reached. Start sign-in again in the app.');
  }
  if (!response.ok) fail(502, 'Steam could not verify sign-in');
  const text = new TextDecoder().decode(await boundedBytes(response.body, 4096));
  if (!text.split(/\r?\n/).includes('is_valid:true')) fail(403, 'Steam rejected sign-in');
  if (await statement(env, 'SELECT nonce FROM nonces WHERE nonce=?', nonce).first()) fail(403, 'Steam response already used');
  const code = random();
  await env.DB.batch([
    statement(env, 'INSERT INTO nonces(nonce,expires) VALUES (?,?)', nonce, now() + 600),
    statement(env, 'UPDATE logins SET steam_id=?,handoff_hash=? WHERE id=? AND steam_id IS NULL', steamId, await sha(code), id)
  ]);
  return new Response(null, {status:302, headers:{
    Location:`http://127.0.0.1:${login.loopback_port}${login.callback_path}?id=${id}&code=${code}`,
    'Cache-Control':'no-store', 'Referrer-Policy':'no-referrer',
    'Set-Cookie':'workshop_login=; Path=/auth; HttpOnly; Secure; SameSite=Lax; Max-Age=0'
  }});
}
async function loginExchange(request, env) {
  const input = await body(request);
  if (!/^[a-f0-9]{64}$/.test(input.id || '') || !/^[a-f0-9]{64}$/.test(input.verifier || '')
    || !/^[a-f0-9]{64}$/.test(input.code || '')) fail(400, 'Invalid local sign-in proof');
  const login = await statement(env, 'SELECT * FROM logins WHERE id=? AND challenge=? AND handoff_hash=? AND expires>? AND consumed=0',
    input.id, await sha(input.verifier), await sha(input.code), now()).first();
  if (!login) fail(401, 'Sign-in expired; start again');
  if (!login.steam_id) fail(401, 'Steam sign-in has not completed');
  if (await statement(env, 'SELECT steam_id FROM bans WHERE steam_id=?', login.steam_id).first()) fail(403, 'This Steam account is banned');
  const claimed = await statement(env, 'UPDATE logins SET consumed=1 WHERE id=? AND consumed=0 RETURNING id', input.id).first();
  if (!claimed) fail(401, 'Sign-in already completed');
  const token = random();
  await statement(env, 'INSERT INTO sessions(hash,steam_id,expires) VALUES (?,?,?)', await sha(token), login.steam_id, now() + 86400).run();
  return json({token, steamId:login.steam_id, moderator:admin(login.steam_id, env)});
}
async function github(env, path, options = {}) {
  const jwtHeader = b64url(encoder.encode(JSON.stringify({alg:'RS256', typ:'JWT'})));
  const payload = b64url(encoder.encode(JSON.stringify({iat:now() - 30, exp:now() + 300, iss:env.GITHUB_APP_ID})));
  const pem = env.GITHUB_APP_PRIVATE_KEY?.replace(/-----[^-]+-----|\s/g, '');
  if (!pem || !/^\d+$/.test(env.GITHUB_INSTALLATION_ID || '')) fail(503, 'Workshop publisher is not configured');
  const key = await crypto.subtle.importKey('pkcs8', Uint8Array.from(atob(pem), c => c.charCodeAt(0)),
    {name:'RSASSA-PKCS1-v1_5', hash:'SHA-256'}, false, ['sign']);
  const unsigned = `${jwtHeader}.${payload}`;
  const signature = await crypto.subtle.sign('RSASSA-PKCS1-v1_5', key, encoder.encode(unsigned));
  const authResponse = await fetch(`https://api.github.com/app/installations/${env.GITHUB_INSTALLATION_ID}/access_tokens`, {
    method:'POST', headers:{Authorization:`Bearer ${unsigned}.${b64url(new Uint8Array(signature))}`,
      Accept:'application/vnd.github+json', 'User-Agent':'TextureHelper-Workshop', 'Content-Type':'application/json'},
    body:JSON.stringify({repositories:['TextureHelper-Workshop'], permissions:{contents:'write'}}),
    redirect:'manual', signal:AbortSignal.timeout(15000)
  });
  if (!authResponse.ok) fail(503, 'GitHub publisher authentication failed');
  const auth = await authResponse.json();
  const upload = path.startsWith('/uploads/');
  const route = upload ? path.slice('/uploads'.length) : path;
  const response = await fetch(`https://${upload ? 'uploads' : 'api'}.github.com/repos/${REPO}${route}`, {
    ...options, headers:{Authorization:`Bearer ${auth.token}`, Accept:'application/vnd.github+json',
      'User-Agent':'TextureHelper-Workshop', ...options.headers}, redirect:'manual', signal:AbortSignal.timeout(30000)
  });
  if (options.method === 'DELETE' && response.status === 404) return null;
  if (!response.ok) fail(502, 'GitHub publication failed; your upload may need administrator cleanup');
  return response.status === 204 ? null : response.json();
}
async function publish(request, env, actor) {
  if (env.PUBLISHING_ENABLED !== 'true') fail(503, 'Workshop publishing is not enabled yet');
  const day = Math.floor(now() / 86400);
  const input = await body(request);
  if (typeof input.title !== 'string' || input.title.trim().length < 3 || input.title.length > 80
    || /[\u0000-\u001f]/.test(input.title) || typeof input.description !== 'string'
    || input.description.length > 1000 || input.license !== 'CC-BY-4.0' || input.consent !== true) fail(400, 'Confirm sharing rights and CC BY 4.0; use a 3–80 character title and at most 1000 description characters');
  let parent = null;
  if (input.parent) {
    if (!/^[a-f0-9]{32}$/.test(input.parent)) fail(400, 'Invalid remix source');
    parent = await statement(env, "SELECT id,steam_id FROM skins WHERE id=? AND status='published'", input.parent).first();
    if (!parent) fail(400, 'Remix source is unavailable');
  }
  await quota(env, `publish-user:${day}:${actor}`, 40, 'Your daily upload limit reached (40 attempts).');
  await quota(env, `publish-global:${day}`, 4000, 'Workshop daily upload limit reached (4,000 attempts across all users).');
  const id = crypto.randomUUID().replaceAll('-', '');
  const attribution = {steamId:actor, profile:`https://steamcommunity.com/profiles/${actor}`, license:'CC-BY-4.0',
    parent:parent?.id || null, parentSteamId:parent?.steam_id || null, title:input.title, description:input.description};
  await statement(env, `INSERT INTO skins(id,steam_id,title,description,model,license,parent,hash,preview_hash,status,created)
    VALUES (?,?,?,?,'','CC-BY-4.0',?,'','','awaiting_upload',?)`, id, actor, input.title, input.description,
    parent?.id || null, now()).run();
  let release;
  try {
    release = await github(env, '/releases', {method:'POST', headers:{'Content-Type':'application/json'},
      body:JSON.stringify({tag_name:`skin-${id}`, name:`Skin ${id}`, draft:true,
        body:JSON.stringify(attribution), make_latest:'false'})});
    const active = await statement(env, `UPDATE skins SET release_id=? WHERE id=? AND status='awaiting_upload'
      AND NOT EXISTS (SELECT 1 FROM bans WHERE steam_id=?) RETURNING id`, release.id, id, actor).first();
    if (!active) fail(409, 'Submission was cancelled or uploader banned');
    return json({id, status:'awaiting_upload'}, 201);
  } catch (error) {
    await statement(env, "UPDATE skins SET status='failed',failure_reason='Could not create the upload storage. Try publishing again later.' WHERE id=? AND status='awaiting_upload'", id).run();
    if (release) {
      try { await github(env, `/releases/${release.id}`, {method:'DELETE'}); }
      catch { fail(502, `Upload failed; administrator cleanup required for submission ${id}`); }
    }
    throw error;
  }
}

async function upload(request, env, actor, id) {
  if (env.PUBLISHING_ENABLED !== 'true') fail(503, 'Workshop publishing is not enabled yet');
  const length = Number(request.headers.get('Content-Length'));
  if (!Number.isSafeInteger(length) || length < 1 || length > 24 * 1024 * 1024 || !request.body) fail(413, 'Upload requires a known length of at most 24 MiB');
  const skin = await statement(env, `UPDATE skins SET status='uploading'
    WHERE id=? AND steam_id=? AND status='awaiting_upload' AND release_id IS NOT NULL AND created>?
    RETURNING release_id`, id, actor, now() - 600).first();
  if (!skin) fail(409, 'Submission is unavailable, expired or already uploaded');
  const transferController = new AbortController();
  try {
    const stream = new FixedLengthStream(length);
    await Promise.all([
      request.body.pipeTo(stream.writable, {signal:transferController.signal}),
      github(env, `/uploads/releases/${skin.release_id}/assets?name=submission.json`,
        {method:'POST', body:stream.readable, headers:{'Content-Type':'application/json', 'Content-Length':String(length)}})
    ]);
    const queued = await statement(env, `UPDATE skins SET status='validating'
      WHERE id=? AND status='uploading' AND NOT EXISTS (SELECT 1 FROM bans WHERE steam_id=?) RETURNING id`, id, actor).first();
    if (!queued) fail(403, 'Submission removed or uploader banned during upload');
    await github(env, '/dispatches', {method:'POST', headers:{'Content-Type':'application/json'},
      body:JSON.stringify({event_type:'validate-skin', client_payload:{id, releaseId:skin.release_id}})});
    return json({id, status:'validating'}, 202);
  } catch (error) {
    await statement(env, "UPDATE skins SET status='failed',failure_reason='Upload transfer or validation queue failed. Try publishing again later.' WHERE id=? AND status IN ('uploading','validating')", id).run();
    try { await github(env, `/releases/${skin.release_id}`, {method:'DELETE'}); }
    catch { fail(502, `Upload failed; administrator cleanup needed for submission ${id}`); }
    throw error;
  } finally { transferController.abort(); }
}

async function validated(request, env) {
  const secret = env.VALIDATOR_SECRET;
  const supplied = request.headers.get('Authorization')?.match(/^Bearer ([a-f0-9]{64})$/)?.[1] || '';
  if (!/^[a-f0-9]{64}$/.test(secret || '') || await sha(supplied) !== await sha(secret)) fail(403, 'Validator authorization required');
  const input = await body(request);
  if (!/^[a-f0-9]{32}$/.test(input.id || '') || typeof input.accepted !== 'boolean') fail(400, 'Invalid validation result');
  if (input.reason !== undefined && (typeof input.reason !== 'string' || input.reason.length > 300 || /[\u0000-\u001f]/.test(input.reason))) fail(400, 'Invalid failure reason');
  const skin = await statement(env, `UPDATE skins SET status='finalizing'
    WHERE id=? AND status='validating' RETURNING *`, input.id).first();
  if (!skin) fail(409, 'Submission no longer awaits validation');
  try {
    if (!input.accepted) {
      await github(env, `/releases/${skin.release_id}`, {method:'DELETE'});
      await statement(env, "UPDATE skins SET status='failed',failure_reason=? WHERE id=? AND status='finalizing'",
        input.reason || 'Automated validation failed. No detailed reason was recorded.', skin.id).run();
      return json({id:skin.id, status:'failed'});
    }
    if (!/^[a-f0-9]{64}$/.test(input.hash || '') || !/^[a-f0-9]{64}$/.test(input.previewHash || '')
      || typeof input.model !== 'string' || input.model.length > 512 || !input.model.startsWith('models/')) fail(400, 'Invalid sanitized metadata');
    const release = await github(env, `/releases/${skin.release_id}`);
    const assets = release.assets || [];
    if (!release.draft || release.tag_name !== `skin-${skin.id}` || assets.length !== 2
      || !assets.some(asset => asset.name === 'skin.phskin' && asset.size > 0 && asset.size <= 24 * 1024 * 1024)
      || !assets.some(asset => asset.name === 'preview.png' && asset.size > 0 && asset.size <= 4 * 1024 * 1024)) fail(409, 'Release still contains raw or unexpected assets');
    if (await statement(env, 'SELECT steam_id FROM bans WHERE steam_id=?', skin.steam_id).first()) fail(403, 'Uploader banned during validation');
    await github(env, `/releases/${skin.release_id}`, {method:'PATCH', headers:{'Content-Type':'application/json'}, body:JSON.stringify({draft:false})});
    const published = await statement(env, `UPDATE skins SET status='published',model=?,hash=?,preview_hash=?
      WHERE id=? AND status='finalizing' AND NOT EXISTS (SELECT 1 FROM bans WHERE steam_id=?) RETURNING id`,
      input.model, input.hash, input.previewHash, skin.id, skin.steam_id).first();
    if (!published) fail(403, 'Submission removed or uploader banned during publication');
    return json({id:skin.id, status:'published'});
  } catch (error) {
    await statement(env, "UPDATE skins SET status='failed',failure_reason='Publication or cleanup failed after validation. Try again later; contact a moderator if it continues.' WHERE id=? AND status='finalizing'", skin.id).run();
    try { await github(env, `/releases/${skin.release_id}`, {method:'DELETE'}); }
    catch { fail(502, `Validation failed; administrator cleanup needed for submission ${skin.id}`); }
    throw error;
  }
}
async function uploaderProfiles(env, rows) {
  // Bound legacy-profile lookups per request; subsequent refreshes fill remaining gaps.
  const missing = [...new Set(rows.filter(row => !row.uploaderName).map(row => row.steamId))].slice(0, 5);
  const profiles = new Map(await Promise.all(missing.map(async id => [id, await profile(env, id)])));
  for (const row of rows) {
    const value = profiles.get(row.steamId);
    if (value) {
      row.uploaderName = value.name; row.uploaderAvatar = value.avatar;
      if (value.profileError) row.profileError = value.profileError;
    }
  }
}
async function moderate(request, env, actor, id) {
  const input = await body(request);
  const personal = input.action === 'cancel' || input.action === 'remove';
  if (!personal && !admin(actor, env)) fail(403, 'Moderator access required');
  if (personal && !admin(actor, env)) {
    const own = await statement(env, 'SELECT steam_id FROM skins WHERE id=?', id).first();
    if (!own || own.steam_id !== actor) fail(403, 'You can only manage your own submissions');
  }
  if (input.action === 'ban' || input.action === 'unban') {
    if (!/^7656119\d{10}$/.test(input.steamId || '') || input.steamId === actor) fail(400, 'Invalid ban target');
    const reason = input.reason === undefined ? 'Workshop abuse' : input.reason;
    if (input.action === 'ban' && (typeof reason !== 'string' || !reason.trim() || reason.length > 300
      || /[\u0000-\u001f]/.test(reason))) fail(400, 'Use a ban reason of 1–300 characters');
    await env.DB.batch([
      input.action === 'ban'
        ? statement(env, 'INSERT OR REPLACE INTO bans(steam_id,reason,created) VALUES (?,?,?)', input.steamId, reason.trim(), now())
        : statement(env, 'DELETE FROM bans WHERE steam_id=?', input.steamId),
      statement(env, 'INSERT INTO audit(actor,action,target,created) VALUES (?,?,?,?)', actor, input.action, input.steamId, now())
    ]);
    return json({ok:true});
  }
  if (personal) {
    if (!/^[a-f0-9]{32}$/.test(id || '')) fail(400, 'Invalid submission ID');
    const allowed = input.action === 'remove' ? "'failed','cancelled'" : "'awaiting_upload','uploading','validating','finalizing','cancelled'";
    const skin = await statement(env, `UPDATE skins SET status='cancelled' WHERE id=?
      AND status IN (${allowed}) AND (steam_id=? OR ?=1) RETURNING release_id`, id, actor, admin(actor, env) ? 1 : 0).first();
    if (!skin) fail(409, 'Submission is no longer pending; refresh the list');
    await statement(env, 'INSERT INTO audit(actor,action,target,created) VALUES (?,?,?,?)', actor, input.action, id, now()).run();
    if (skin.release_id) await github(env, `/releases/${skin.release_id}`, {method:'DELETE'});
    if (input.action === 'remove') await statement(env, "UPDATE skins SET status='removed' WHERE id=? AND status='cancelled'", id).run();
    return json({ok:true});
  }
  if (input.action !== 'hide' || !/^[a-f0-9]{32}$/.test(id || '')) fail(400, 'Invalid moderation action');
  const skin = await statement(env, 'SELECT release_id FROM skins WHERE id=?', id).first();
  if (!skin) fail(404, 'Skin not found');
  await env.DB.batch([
    statement(env, "UPDATE skins SET status='hidden' WHERE id=?", id),
    statement(env, 'INSERT INTO audit(actor,action,target,created) VALUES (?,?,?,?)', actor, 'hide', id, now())
  ]);
  if (skin.release_id) await github(env, `/releases/${skin.release_id}`, {method:'DELETE'});
  return json({ok:true});
}
export default {
  async fetch(request, env) {
    try {
      const url = new URL(request.url);
      if (url.origin !== env.PUBLIC_ORIGIN || !url.origin.startsWith('https://')) fail(400, 'Invalid workshop origin');
      if (request.method === 'GET' && url.pathname === '/') return json({service:'Texture Helper Workshop', publishing:env.PUBLISHING_ENABLED === 'true'});
      if (request.method === 'POST' && url.pathname === '/auth/start') return await loginStart(request, env);
      if (request.method === 'GET' && url.pathname === '/auth/steam') return await steamStart(url, env);
      if (request.method === 'GET' && url.pathname === '/auth/callback') return await steamCallback(request, url, env);
      if (request.method === 'POST' && url.pathname === '/auth/exchange') return await loginExchange(request, env);
      if (request.method === 'POST' && url.pathname === '/internal/validated') return await validated(request, env);
      if (request.method === 'GET' && url.pathname === '/skins') {
        const cursor = /^(\d{1,12})-([a-f0-9]{32})$/.exec(url.searchParams.get('before') || '');
        if (url.searchParams.get('before') && !cursor) fail(400, 'Invalid page cursor');
        const time = cursor ? Number(cursor[1]) : now() + 1;
        const rows = await statement(env, `SELECT id,steam_id AS steamId,title,description,model,license,parent,hash,preview_hash AS previewHash,created,
          (SELECT name FROM profiles WHERE profiles.steam_id=skins.steam_id) AS uploaderName,
          (SELECT avatar FROM profiles WHERE profiles.steam_id=skins.steam_id) AS uploaderAvatar
          FROM skins WHERE status='published' AND (created<? OR (created=? AND id<?)) ORDER BY created DESC,id DESC LIMIT 50`,
          time, time, cursor?.[2] || '').all();
        const last = rows.results.at(-1);
        await uploaderProfiles(env, rows.results);
        return json({skins:rows.results, next:rows.results.length === 50 ? `${last.created}-${last.id}` : null});
      }
      const asset = /^\/skins\/([a-f0-9]{32})\/(skin\.phskin|preview\.png)$/.exec(url.pathname);
      if (request.method === 'GET' && asset) {
        const row = await statement(env, "SELECT id FROM skins WHERE id=? AND status='published'", asset[1]).first();
        if (!row) fail(404, 'Skin is unavailable');
        return new Response(null, {status:302, headers:{Location:`https://github.com/${REPO}/releases/download/skin-${asset[1]}/${asset[2]}`,
          'Cache-Control':'no-store'}});
      }
      const actor = await session(request, env);
      if (request.method === 'POST' && url.pathname === '/logout') {
        await statement(env, 'DELETE FROM sessions WHERE hash=?', await sha(request.headers.get('Authorization').slice(7))).run();
        return json({ok:true});
      }
      if (request.method === 'GET' && url.pathname === '/me') return json({steamId:actor, moderator:admin(actor, env), ...await profile(env, actor)});
      if (request.method === 'GET' && url.pathname === '/submissions') {
        const moderator = admin(actor, env);
        const rows = await statement(env, `SELECT id,title,description,model,steam_id AS steamId,status,created,failure_reason AS failureReason,
          (SELECT name FROM profiles WHERE profiles.steam_id=skins.steam_id) AS uploaderName,
          (SELECT avatar FROM profiles WHERE profiles.steam_id=skins.steam_id) AS uploaderAvatar
          FROM skins WHERE ${moderator ? '1=1' : 'steam_id=?'}
          AND status IN ('awaiting_upload','uploading','validating','finalizing','failed','cancelled')
          ORDER BY created DESC,id DESC LIMIT 300`, ...(moderator ? [] : [actor])).all();
        await uploaderProfiles(env, rows.results);
        return json({submissions:rows.results});
      }
      if (request.method === 'GET' && url.pathname === '/bans') {
        if (!admin(actor, env)) fail(403, 'Moderator access required');
        const after = url.searchParams.get('after') || '';
        if (after && !/^7656119\d{10}$/.test(after)) fail(400, 'Invalid ban page cursor');
        const rows = await statement(env, `SELECT steam_id AS steamId,reason,created,
          (SELECT name FROM profiles WHERE profiles.steam_id=bans.steam_id) AS name FROM bans
          WHERE steam_id>? ORDER BY steam_id LIMIT 50`, after).all();
        return json({bans:rows.results, next:rows.results.length === 50 ? rows.results.at(-1).steamId : null});
      }
      if (request.method === 'POST' && url.pathname === '/skins') return await publish(request, env, actor);
      const submission = /^\/skins\/([a-f0-9]{32})\/upload$/.exec(url.pathname);
      if (request.method === 'POST' && submission) return await upload(request, env, actor, submission[1]);
      if (request.method === 'POST' && url.pathname.startsWith('/moderate/')) return await moderate(request, env, actor, url.pathname.split('/')[2]);
      fail(404, 'Unknown workshop route');
    } catch (error) {
      if (error instanceof HttpError) return json({error:error.message}, error.status);
      console.error('Workshop internal failure', {route:new URL(request.url).pathname, type:error?.name || 'UnknownError'});
      return json({error:'Workshop service unavailable. No success is assumed; refresh before retrying.'}, 503);
    }
  }
};
