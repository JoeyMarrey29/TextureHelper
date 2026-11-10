import {test} from 'node:test';
import assert from 'node:assert/strict';
import {deflateSync} from 'node:zlib';
import {generateKeyPairSync} from 'node:crypto';
import {sanitizePng, base64, boundedBytes} from '../src/images.js';
import service, {steamIdentity, steamProfile} from '../src/index.js';
import {publicPackage} from '../src/package.js';
import {validateSubmission} from '../publisher/validate.js';
import {png as makePng, submission, environment, request, origin, actor, token, hash} from './helpers.js';
import catalog from '../src/catalog.json' with {type:'json'};

const png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg==';
test('image validator rejects corrupt PNGs, SVGs and oversized data', async () => {
  for (const value of ['not png', base64(new TextEncoder().encode('<svg/>')), png, 'a'.repeat(6 * 1024 * 1024)]) {
    await assert.rejects(() => sanitizePng(value));
  }
});
test('bounded stream terminates oversized decompression', async () => {
  const compressed = deflateSync(Buffer.alloc(1024 * 1024));
  const stream = new Blob([compressed]).stream().pipeThrough(new DecompressionStream('deflate'));
  await assert.rejects(() => boundedBytes(stream, 1024));
});
test('OpenID refuses unsigned identity, wrong provider, stale nonce and duplicate fields', () => {
  const origin = 'https://workshop.example';
  const url = new URL(`${origin}/auth/callback`);
  const id = 'a'.repeat(64), steamId = '76561198000000000';
  const fields = {id, 'openid.ns':'http://specs.openid.net/auth/2.0', 'openid.mode':'id_res',
    'openid.op_endpoint':'https://steamcommunity.com/openid/login',
    'openid.return_to':`${origin}/auth/callback?id=${id}`,
    'openid.identity':`https://steamcommunity.com/openid/id/${steamId}`,
    'openid.claimed_id':`https://steamcommunity.com/openid/id/${steamId}`,
    'openid.response_nonce':new Date().toISOString().replace(/\.\d{3}Z$/, 'Z') + 'nonce',
    'openid.signed':'op_endpoint,claimed_id,identity,return_to,response_nonce,assoc_handle'};
  for (const [key, value] of Object.entries(fields)) url.searchParams.set(key, value);
  assert.equal(steamIdentity(url, origin).steamId, steamId);
  for (const [key, value] of [['openid.signed','return_to'], ['openid.op_endpoint','https://evil.example/'],
    ['openid.claimed_id','https://evil.example/76561198000000000'],
    ['openid.response_nonce','2000-01-01T00:00:00Znonce'], ['openid.return_to','https://evil.example/']]) {
    const invalid = new URL(url); invalid.searchParams.set(key, value);
    assert.throws(() => steamIdentity(invalid, origin));
  }
  url.searchParams.append('openid.claimed_id', fields['openid.claimed_id']);
  assert.throws(() => steamIdentity(url, origin));
});
test('service is closed until configured; origin and auth required', async () => {
  const env = {PUBLIC_ORIGIN:'https://workshop.example', PUBLISHING_ENABLED:'false'};
  assert.equal((await service.fetch(new Request('https://wrong.example/'), env)).status, 400);
  const root = await service.fetch(new Request('https://workshop.example/'), env);
  assert.equal((await root.json()).publishing, false);
  assert.equal((await service.fetch(new Request('https://workshop.example/skins',{method:'POST'}), env)).status, 401);
});
test('public schema rejects opaque backups and arbitrary model/material paths', async () => {
  await assert.rejects(() => publicPackage({draft:{modelPath:'models/evil.mdl', textures:[]}}));
});

test('valid PNGs survive sanitizing but metadata does not', async () => {
  const input = makePng(8, 4, {metadata:true});
  const result = await sanitizePng(input.toString('base64'));
  assert.equal(result.width, 8); assert.equal(result.height, 4);
  assert.ok(!Buffer.from(result.bytes).includes(Buffer.from('private metadata')));
  const second = await sanitizePng(base64(result.bytes));
  assert.deepEqual(second.bytes, result.bytes);
  for (const bytes of [makePng(2049, 1), makePng(1, 1, {filter:5}),
    makePng(1, 1, {decodedLength:100}), makePng().subarray(0, -1), Buffer.concat([makePng(), Buffer.of(0)])]) {
    await assert.rejects(() => sanitizePng(bytes.toString('base64')));
  }
});

test('publication reconstructs a minimal trusted package and rejects forged ownership', async () => {
  const input = submission();
  const {draft} = await publicPackage(input);
  assert.equal(draft.aiOriginals, undefined);
  assert.equal(draft.textures[0].vmtText.includes('UntrustedShader'), false);
  assert.equal(draft.textures[0].vmtText.includes('$include'), false);
  assert.match(draft.textures[0].vmtText, /^VertexLitGeneric/);
  assert.equal(draft.id, null);
  for (const mutate of [
    input => input.draft.textures.push(input.draft.textures[0]),
    input => input.draft.textures[0].vtfPath = 'materials/evil.vtf',
    input => input.draft.textures[0].partPath = 'models/evil.mdl',
    input => input.draft.modelPath = '__proto__',
    input => input.draft.slotSelections = {__proto__:null, constructor:null},
    input => input.draft.archivedFiles = [{path:'evil.exe'}],
    input => input.draft.textures[0].png = makePng(1, 1).toString('base64')
  ]) {
    const forged = structuredClone(input); mutate(forged);
    await assert.rejects(() => publicPackage(forged));
  }
});

test('gun publications accept compatible attachments and required rails, not unrelated parts', async () => {
  const input = submission();
  input.draft.modelPath = 'models/perpheads/weapons/c_ak101.mdl';
  input.draft.slotSelections = {SLOT_STOCK:'stock_ak101', SLOT_MAGAZINE:null};
  for (const part of ['stock_ak101', 'rifle_rail_1']) {
    const [key, material] = Object.entries(catalog.materials).find(([key]) => key.startsWith(
      `models/perpheads/weapons/attachments/${part}.mdl|`));
    const [partPath, vmtPath] = key.split('|');
    input.draft.textures = [{partPath, vmtPath, vtfPath:material.vtf,
      png:makePng(material.width, material.height).toString('base64')}];
    await publicPackage(input);
  }
  input.draft.slotSelections.SLOT_STOCK = 'stock_m4a1';
  await assert.rejects(() => publicPackage(input), /Unsupported attachment selection/);
});

test('katana skin is supported without granting attachment or arbitrary-material access', async () => {
  const modelPath = 'models/perpheads/weapons/c_katana.mdl';
  assert.ok(catalog.models[modelPath], 'Editor-compatible katana must be in the trusted catalogue');
  const entries = Object.entries(catalog.materials).filter(([key]) => key.startsWith(`${modelPath}|`));
  assert.ok(entries.length > 0);
  const input = submission();
  input.draft.modelPath = modelPath;
  input.draft.slotSelections = {};
  input.draft.textures = entries.map(([key, material]) => ({
    partPath:modelPath, vmtPath:key.split('|')[1], vtfPath:material.vtf,
    png:makePng(material.width, material.height).toString('base64')
  }));
  const result = await publicPackage(input);
  assert.equal(result.draft.modelPath, modelPath);
  assert.equal(result.draft.textures.length, entries.length);
  input.draft.slotSelections = {SLOT_STOCK:'stock_ak101'};
  await assert.rejects(() => publicPackage(input), /Unsupported attachment slot/);
  input.draft.slotSelections = {};
  input.draft.textures[0].vtfPath = 'materials/evil.vtf';
  await assert.rejects(() => publicPackage(input), /Unsupported or duplicate/);
});

test('modified clients cannot claim moderator rights or bypass a ban', async t => {
  const env = environment(t);
  const me = await service.fetch(request('/me'), env);
  assert.deepEqual(await me.json(), {steamId:actor, moderator:false, name:'Test uploader',
    avatar:`https://avatars.steamstatic.com/${'a'.repeat(40)}_medium.jpg`});
  assert.equal((await service.fetch(request('/moderate/account', {body:{action:'ban',
    steamId:env.MODERATOR_STEAM_ID, moderator:true, steamIdOfCaller:env.MODERATOR_STEAM_ID}}), env)).status, 403);
  assert.equal((await service.fetch(request('/skins', {body:submission()}), env)).status, 503);
  env.DB.db.prepare('INSERT INTO bans VALUES (?,?,?)').run(actor, 'test', 0);
  for (const path of ['/me', '/skins', '/moderate/account']) {
    assert.equal((await service.fetch(request(path, {body:path === '/me' ? undefined : {moderator:true}}), env)).status, 403);
  }
  assert.equal((await service.fetch(request('/me', {headers:{Authorization:`Bearer ${'c'.repeat(64)}`}}), env)).status, 401);
});

test('Steam names and avatars are bound to the verified ID and safe image hosts', () => {
  const xml = (id = actor, avatar = `https://avatars.steamstatic.com/${'a'.repeat(40)}_medium.jpg`) =>
    `<profile><steamID64>${id}</steamID64><steamID><![CDATA[Name <not markup>]]></steamID><avatarMedium><![CDATA[${avatar}]]></avatarMedium></profile>`;
  assert.equal(steamProfile(xml(), actor).name, 'Name <not markup>');
  assert.ok(steamProfile(xml(actor, `https://avatars.fastly.steamstatic.com/${'a'.repeat(40)}_medium.jpg`), actor).avatar);
  assert.throws(() => steamProfile(xml('76561198000000002'), actor));
  assert.throws(() => steamProfile(xml(actor, 'https://evil.example/track.jpg'), actor));
  assert.throws(() => steamProfile(xml(actor, 'https://avatars.steamstatic.com.evil.example/a.jpg'), actor));
  assert.throws(() => steamProfile(xml(actor, 'javascript:alert(1)'), actor));
});

test('pending lists enforce ownership and moderator cancellation cannot publish or cancel a finished skin', async t => {
  const env = environment(t);
  const add = env.DB.db.prepare(`INSERT INTO skins(id,steam_id,title,description,model,license,hash,preview_hash,status,created)
    VALUES (?,?,?,'','model','CC-BY-4.0','hash','preview',?,?)`);
  const own = 'a'.repeat(32), other = 'b'.repeat(32), published = 'c'.repeat(32);
  add.run(own, actor, 'Mine', 'validating', 1);
  add.run(other, '76561198000000002', 'Someone else', 'awaiting_upload', 2);
  add.run(published, actor, 'Already published', 'published', 3);
  env.DB.db.prepare('INSERT INTO profiles VALUES (?,?,?,?)').run('76561198000000002', 'Other uploader',
    `https://avatars.steamstatic.com/${'a'.repeat(40)}_medium.jpg`, Math.floor(Date.now()/1000)+3600);
  const listing = await (await service.fetch(request('/submissions?moderator=true'), env)).json();
  assert.deepEqual(listing.submissions.map(item => item.id), [own]);
  assert.equal(listing.submissions[0].uploaderName, 'Test uploader');
  assert.ok(listing.submissions[0].uploaderAvatar);
  assert.equal((await service.fetch(request(`/moderate/${other}`, {body:{action:'cancel'}}), env)).status, 403);
  assert.equal((await service.fetch(request(`/moderate/${other}`, {body:{action:'remove'}}), env)).status, 403);
  assert.equal((await service.fetch(request(`/moderate/${own}`, {body:{action:'remove'}}), env)).status, 409);
  assert.equal((await service.fetch(request(`/moderate/${own}`, {body:{action:'cancel'}}), env)).status, 200);
  assert.equal((await service.fetch(request(`/moderate/${published}`, {body:{action:'remove'}}), env)).status, 409);
  assert.equal((await service.fetch(request('/bans'), env)).status, 403);
  env.MODERATOR_STEAM_ID = actor;
  const all = await (await service.fetch(request('/submissions'), env)).json();
  assert.deepEqual(all.submissions.map(item => item.id), [other, own]);
  const cancel = await service.fetch(request(`/moderate/${other}`, {body:{action:'cancel'}}), env);
  assert.equal(cancel.status, 200);
  assert.equal(env.DB.db.prepare('SELECT status FROM skins WHERE id=?').get(other).status, 'cancelled');
  assert.equal((await service.fetch(request(`/skins/${other}/preview.png`), env)).status, 404);
  assert.equal((await service.fetch(request(`/moderate/${published}`, {body:{action:'cancel'}}), env)).status, 409);
  assert.equal((await service.fetch(request(`/moderate/${other}`, {body:{action:'cancel'}}), env)).status, 200);
  assert.equal(env.DB.db.prepare("SELECT COUNT(*) AS count FROM audit WHERE action='cancel'").get().count, 3);
  env.MODERATOR_STEAM_ID = '76561198000000001';
  env.DB.db.prepare("UPDATE skins SET status='failed' WHERE id=?").run(own);
  assert.equal((await service.fetch(request(`/moderate/${own}`, {body:{action:'remove'}}), env)).status, 200);
  const remaining = await (await service.fetch(request('/submissions'), env)).json();
  assert.equal(remaining.submissions.length, 0);
  assert.equal(env.DB.db.prepare('SELECT status FROM skins WHERE id=?').get(own).status, 'removed');
});

test('moderator ban table supports reasons, pagination, unbanning and rejects self-ban', async t => {
  const env = environment(t); env.MODERATOR_STEAM_ID = actor;
  const target = '76561198000000002';
  const ban = (steamId, reason) => service.fetch(request('/moderate/account', {body:{action:'ban', steamId, reason}}), env);
  assert.equal((await ban(actor, 'test')).status, 400);
  assert.equal((await ban(target, '')).status, 400);
  assert.equal((await ban(target, 'x'.repeat(301))).status, 400);
  assert.equal((await ban(target, 'Abusive uploads')).status, 200);
  let result = await (await service.fetch(request('/bans'), env)).json();
  assert.equal(result.bans[0].reason, 'Abusive uploads');
  assert.equal(result.bans[0].steamId, target);
  assert.equal((await service.fetch(request('/moderate/account', {body:{action:'unban', steamId:target}}), env)).status, 200);
  const insert = env.DB.db.prepare('INSERT INTO bans VALUES (?,?,?)');
  for (let i = 0; i < 51; i++) insert.run(`76561198${String(100000000 + i).padStart(9, '0')}`, 'test', 1);
  result = await (await service.fetch(request('/bans'), env)).json();
  assert.equal(result.bans.length, 50); assert.ok(result.next);
  const second = await (await service.fetch(request(`/bans?after=${result.next}`), env)).json();
  assert.equal(second.bans.length, 1);
  assert.notEqual(result.bans.at(-1).steamId, second.bans[0].steamId);
  assert.equal((await service.fetch(request('/bans?after=bad'), env)).status, 400);
});

test('login quotas persist across accounts and expire daily counters', async t => {
  const env = environment(t), day = Math.floor(Date.now() / 86400000);
  env.DB.db.prepare('INSERT INTO quotas VALUES (?,?,?)').run('lifetime-assets', 300, null);
  env.DB.db.prepare('INSERT INTO quotas VALUES (?,?,?)').run('expired', 1, 1);
  const login = () => service.fetch(request('/auth/start', {body:{challenge:'a'.repeat(64), port:49152, callbackPath:`/workshop/${'c'.repeat(64)}`},
    authenticated:false, headers:{'CF-Connecting-IP':'192.0.2.1'}}), env);
  for (let i = 0; i < 30; i++) assert.equal((await login()).status, 200);
  assert.equal((await login()).status, 429);
  assert.equal(env.DB.db.prepare('SELECT count FROM quotas WHERE key=?').get('lifetime-assets').count, 300);
  assert.equal(env.DB.db.prepare('SELECT count FROM quotas WHERE key=?').get(`login-global:${day}`).count, 30);
  assert.equal(env.DB.db.prepare('SELECT count FROM quotas WHERE key=?').get('expired'), undefined);
});

test('upload quotas allow forty per user and four thousand globally with no lifetime cap', async t => {
  const day = Math.floor(Date.now() / 86400000);
  for (const [kind, limit] of [['user', 40], ['global', 4000]]) {
    const env = environment(t);
    env.PUBLISHING_ENABLED = 'true';
    const key = kind === 'user' ? `publish-user:${day}:${actor}` : `publish-global:${day}`;
    env.DB.db.prepare('INSERT INTO quotas VALUES (?,?,?)').run(key, limit - 1, Math.floor(Date.now()/1000) + 172800);
    // A legacy lifetime counter must no longer prevent attempts.
    env.DB.db.prepare('INSERT INTO quotas VALUES (?,?,?)').run('lifetime-assets', 300, null);
    const {draft, preview, ...metadata} = submission();
    const invalid = await service.fetch(request('/skins', {body:{...metadata, consent:false}}), env);
    assert.equal(invalid.status, 400);
    assert.equal(env.DB.db.prepare('SELECT count FROM quotas WHERE key=?').get(key).count, limit - 1);
    const admitted = await service.fetch(request('/skins', {body:metadata}), env);
    // This fixture has no GitHub credentials: storage fails only after quota admission.
    assert.notEqual(admitted.status, 429);
    assert.equal(env.DB.db.prepare('SELECT count FROM quotas WHERE key=?').get(key).count, limit);
    assert.equal(env.DB.db.prepare('SELECT count FROM quotas WHERE key=?').get('lifetime-assets').count, 300);
    const blocked = await service.fetch(request('/skins', {body:metadata}), env);
    assert.equal(blocked.status, 429);
    const error = (await blocked.json()).error;
    assert.match(error, kind === 'user' ? /40 attempts/ : /4,000 attempts across all users/);
    assert.match(error, /midnight UTC/);
    assert.equal(env.DB.db.prepare('SELECT count FROM quotas WHERE key=?').get(key).count, limit);
  }
});

test('session exchange requires both local callback code and app verifier and is consumed once', async t => {
  const env = environment(t), id = 'a'.repeat(64), verifier = 'd'.repeat(64), code = 'e'.repeat(64);
  env.DB.db.prepare('INSERT INTO logins(id,challenge,steam_id,handoff_hash,loopback_port,callback_path,expires) VALUES (?,?,?,?,?,?,?)')
    .run(id, hash(verifier), actor, hash(code), 49152, `/workshop/${'c'.repeat(64)}`, Math.floor(Date.now() / 1000) + 600);
  const exchange = (proof, handoff) => service.fetch(request('/auth/exchange', {body:{id, verifier:proof, code:handoff}, authenticated:false}), env);
  assert.equal((await exchange('f'.repeat(64), code)).status, 401);
  assert.equal((await exchange(verifier)).status, 400);
  assert.equal((await exchange(verifier, 'f'.repeat(64))).status, 401);
  assert.equal((await service.fetch(request('/auth/poll', {authenticated:false, body:{id, verifier}}), env)).status, 401);
  const result = await exchange(verifier, code);
  assert.equal(result.status, 200);
  const identity = await result.json();
  assert.match(identity.token, /^[a-f0-9]{64}$/);
  assert.equal(identity.steamId, actor);
  assert.equal((await exchange(verifier, code)).status, 401);
  const logout = await service.fetch(request('/logout', {body:{}}), env);
  assert.equal(logout.status, 200);
  assert.equal(env.DB.db.prepare('SELECT hash FROM sessions WHERE hash=?').get(hash(token)), undefined);
});

test('hidden assets cannot be downloaded and paging preserves timestamp ties', async t => {
  const env = environment(t);
  const insert = env.DB.db.prepare(`INSERT INTO skins(id,steam_id,title,description,model,license,hash,preview_hash,status,created)
    VALUES (?,?,?,'','model','CC-BY-4.0','hash','preview',?,?)`);
  const created = Math.floor(Date.now() / 1000);
  for (let i = 0; i < 51; i++) insert.run(i.toString(16).padStart(32, '0'), actor, `Skin ${i}`, 'published', created);
  const first = await (await service.fetch(request('/skins'), env)).json();
  const second = await (await service.fetch(request(`/skins?before=${first.next}`), env)).json();
  assert.equal(first.skins.length, 50); assert.equal(second.skins.length, 1);
  assert.notEqual(first.skins.at(-1).id, second.skins[0].id);
  const id = first.skins[0].id;
  assert.equal((await service.fetch(request(`/skins/${id}/skin.phskin`), env)).status, 302);
  env.DB.db.prepare("UPDATE skins SET status='hidden' WHERE id=?").run(id);
  assert.equal((await service.fetch(request(`/skins/${id}/skin.phskin`), env)).status, 404);
});

test('Steam callback requires browser/provider proof and hands code only to a fixed local app callback', async t => {
  const env = environment(t), id = 'a'.repeat(64), browser = 'e'.repeat(64);
  const verifier = 'd'.repeat(64), callbackPath = `/workshop/${'c'.repeat(64)}`;
  env.DB.db.prepare('INSERT INTO logins(id,challenge,browser_hash,loopback_port,callback_path,expires) VALUES (?,?,?,?,?,?)')
    .run(id, hash(verifier), hash(browser), 49152, callbackPath, Math.floor(Date.now() / 1000) + 600);
  const url = new URL(`${origin}/auth/callback`);
  const fields = {id, 'openid.ns':'http://specs.openid.net/auth/2.0', 'openid.mode':'id_res',
    'openid.op_endpoint':'https://steamcommunity.com/openid/login',
    'openid.return_to':`${origin}/auth/callback?id=${id}`,
    'openid.identity':`https://steamcommunity.com/openid/id/${actor}`,
    'openid.claimed_id':`https://steamcommunity.com/openid/id/${actor}`,
    'openid.assoc_handle':'test',
    'openid.response_nonce':new Date().toISOString().replace(/\.\d{3}Z$/, 'Z') + 'nonce',
    'openid.signed':'op_endpoint,claimed_id,identity,return_to,response_nonce,assoc_handle',
    'openid.sig':'forged-signature'};
  for (const [key, value] of Object.entries(fields)) url.searchParams.set(key, value);
  let valid = false, calls = 0;
  t.mock.method(globalThis, 'fetch', async (target, options) => {
    calls++;
    assert.equal(target, 'https://steamcommunity.com/openid/login');
    assert.equal(options.redirect, 'manual');
    assert.equal(options.body.get('openid.mode'), 'check_authentication');
    return new Response(`ns:http://specs.openid.net/auth/2.0\nis_valid:${valid}\n`);
  });
  const callback = cookie => service.fetch(new Request(url, {headers:cookie ? {Cookie:`workshop_login=${cookie}`} : {}}), env);
  assert.equal((await callback()).status, 403);
  assert.equal((await callback('f'.repeat(64))).status, 403);
  assert.equal(calls, 0);
  assert.equal((await callback(browser)).status, 403);
  assert.equal(env.DB.db.prepare('SELECT steam_id FROM logins WHERE id=?').get(id).steam_id, null);
  valid = true;
  const completed = await callback(browser);
  assert.equal(completed.status, 302);
  const handoff = new URL(completed.headers.get('Location'));
  assert.equal(handoff.origin, 'http://127.0.0.1:49152');
  assert.equal(handoff.pathname, callbackPath);
  assert.equal(handoff.searchParams.get('id'), id);
  const code = handoff.searchParams.get('code');
  assert.match(code, /^[a-f0-9]{64}$/);
  assert.equal(env.DB.db.prepare('SELECT handoff_hash FROM logins WHERE id=?').get(id).handoff_hash, hash(code));
  assert.equal(env.DB.db.prepare('SELECT steam_id FROM logins WHERE id=?').get(id).steam_id, actor);
  assert.equal((await callback(browser)).status, 403);
  const attacker = await service.fetch(request('/auth/exchange', {body:{id, verifier}, authenticated:false}), env);
  assert.equal(attacker.status, 400);
  const app = await service.fetch(request('/auth/exchange', {body:{id, verifier, code}, authenticated:false}), env);
  assert.equal(app.status, 200);
});

test('login cannot redirect the one-time callback code to an external server', async t => {
  const env = environment(t);
  for (const fields of [{port:80}, {port:65536}, {port:'49152'},
    {callbackPath:'https://attacker.example/'}, {callbackPath:'/workshop/../../redirect'}, {callbackPath:'//attacker.example/'}]) {
    const response = await service.fetch(request('/auth/start', {authenticated:false,
      headers:{'CF-Connecting-IP':'192.0.2.1'}, body:{challenge:'a'.repeat(64),
        port:49152, callbackPath:`/workshop/${'c'.repeat(64)}`, ...fields}}), env);
    assert.equal(response.status, 400);
  }
});

test('queued validation publishes only sanitized assets and closes authorization and removal races', async t => {
  const {privateKey} = generateKeyPairSync('rsa', {modulusLength:2048,
    privateKeyEncoding:{type:'pkcs8', format:'pem'}, publicKeyEncoding:{type:'spki', format:'pem'}});
  for (const scenario of ['success', 'upload-failure', 'invalid-image', 'ban-during-publish', 'hide-during-publish', 'cancel-during-publish', 'raw-asset-left']) {
    await t.test(scenario, async t => {
      const env = environment(t);
      Object.assign(env, {PUBLISHING_ENABLED:'true', GITHUB_APP_ID:'123', GITHUB_INSTALLATION_ID:'456',
        GITHUB_APP_PRIVATE_KEY:privateKey, VALIDATOR_SECRET:'f'.repeat(64)});
      const calls = [], assets = [];
      let releaseBody, draft = true, tag, assetId = 0;
      globalThis.FixedLengthStream = class extends TransformStream {
        constructor(length) {
          let received = 0;
          super({
            transform(chunk, controller) {
              received += chunk.length;
              if (received > length) throw new Error('Exceeded upload length');
              controller.enqueue(chunk);
            },
            flush() { if (received !== length) throw new Error('Short upload'); }
          });
        }
      };
      t.after(() => { delete globalThis.FixedLengthStream; });
      t.mock.method(globalThis, 'fetch', async (target, options) => {
        const url = new URL(target); calls.push([options.method, url.pathname]);
        assert.ok(['api.github.com', 'uploads.github.com'].includes(url.hostname));
        if (url.pathname === '/app/installations/456/access_tokens') {
          assert.deepEqual(JSON.parse(options.body), {repositories:['TextureHelper-Workshop'], permissions:{contents:'write'}});
          return Response.json({token:'test-installation-token'});
        }
        assert.ok(url.pathname.startsWith('/repos/JoeyMarrey29/TextureHelper-Workshop/'));
        if (url.pathname.endsWith('/dispatches')) return new Response(null, {status:204});
        if (options.method === 'DELETE') {
          if (url.pathname.includes('/assets/')) assets.splice(assets.findIndex(asset => asset.id === Number(url.pathname.split('/').at(-1))), 1);
          return new Response(null, {status:204});
        }
        if (url.hostname === 'uploads.github.com') {
          if (scenario === 'upload-failure') return new Response('fail', {status:500});
          const bytes = options.body instanceof ReadableStream ? await boundedBytes(options.body, 24 * 1024 * 1024) : options.body;
          const asset = {id:++assetId, name:url.searchParams.get('name'), size:bytes.length, bytes};
          assets.push(asset);
          return Response.json({id:asset.id});
        }
        if (options.method === 'PATCH') {
          if (scenario === 'ban-during-publish') env.DB.db.prepare('INSERT INTO bans VALUES (?,?,?)').run(actor, 'test', 0);
          if (scenario === 'hide-during-publish') env.DB.db.exec("UPDATE skins SET status='hidden'");
          if (scenario === 'cancel-during-publish') {
            env.MODERATOR_STEAM_ID = actor;
            const id = env.DB.db.prepare('SELECT id FROM skins').get().id;
            assert.equal((await service.fetch(request(`/moderate/${id}`, {body:{action:'cancel'}}), env)).status, 200);
          }
          draft = false; return Response.json({id:100});
        }
        if (url.pathname.includes('/assets/')) {
          return new Response(assets.find(asset => asset.id === Number(url.pathname.split('/').at(-1))).bytes);
        }
        if (!options.method || options.method === 'GET') {
          return Response.json({id:100, draft, tag_name:tag, body:releaseBody,
            assets:assets.map(({bytes, ...asset}) => asset)});
        }
        assert.equal(JSON.parse(options.body).draft, true);
        releaseBody = JSON.parse(options.body).body; tag = JSON.parse(options.body).tag_name;
        return Response.json({id:100});
      });
      const {draft:skin, preview, ...metadata} = submission();
      const created = await service.fetch(request('/skins', {body:metadata}), env);
      assert.equal(created.status, 201);
      const {id} = await created.json();
      const raw = JSON.stringify({draft:skin, preview:scenario === 'invalid-image' ? 'not-an-image' : preview});
      const upload = await service.fetch(new Request(`${origin}/skins/${id}/upload`, {method:'POST',
        headers:{Authorization:`Bearer ${token}`, 'Content-Length':String(Buffer.byteLength(raw))}, body:raw}), env);
      const entry = () => env.DB.db.prepare('SELECT * FROM skins').get();
      if (scenario === 'upload-failure') {
        assert.equal(upload.status, 502);
      } else {
        assert.equal(upload.status, 202);
        assert.equal(entry().status, 'validating');
        assert.equal((await service.fetch(request(`/skins/${id}/skin.phskin`), env)).status, 404);
        const report = async value => {
          const response = await service.fetch(request('/internal/validated', {body:value,
            headers:{Authorization:`Bearer ${env.VALIDATOR_SECRET}`}}), env);
          if (!response.ok) throw new Error(`Result rejected (${response.status})`);
        };
        assert.equal((await service.fetch(request('/internal/validated', {body:{id, accepted:true}}), env)).status, 403);
        if (scenario === 'raw-asset-left') {
          await assert.rejects(() => report({id, accepted:true, model:skin.modelPath, hash:'a'.repeat(64), previewHash:'b'.repeat(64)}));
        } else {
          const run = () => validateSubmission({id, releaseId:100, report,
            github:(path, {upload = false, ...options} = {}) => fetch(`https://${upload ? 'uploads' : 'api'}.github.com/repos/JoeyMarrey29/TextureHelper-Workshop${path}`,
              {method:'GET', ...options})});
          if (scenario === 'success') await run();
          else await assert.rejects(run);
        }
      }
      if (scenario === 'success') {
        assert.equal(entry().status, 'published'); assert.equal(draft, false);
        assert.deepEqual(assets.map(asset => asset.name), ['skin.phskin', 'preview.png']);
        const packageBytes = assets.find(asset => asset.name === 'skin.phskin').bytes;
        const pkg = JSON.parse(new TextDecoder().decode(packageBytes));
        assert.equal(hash(packageBytes), entry().hash);
        assert.equal(pkg.draft.workshopSource.steamId, actor);
        assert.equal(pkg.draft.aiOriginals, undefined);
      } else {
        assert.notEqual(entry().status, 'published');
        if (scenario === 'invalid-image') {
          assert.equal(entry().failure_reason, 'Invalid PNG encoding or size');
          const listed = await (await service.fetch(request('/submissions'), env)).json();
          assert.equal(listed.submissions[0].failureReason, 'Invalid PNG encoding or size');
        }
        if (scenario === 'upload-failure') assert.match(entry().failure_reason, /Upload transfer or validation queue failed/);
        if (scenario === 'ban-during-publish') assert.ok(env.DB.db.prepare('SELECT steam_id FROM bans WHERE steam_id=?').get(actor));
        if (scenario === 'hide-during-publish') assert.equal(entry().status, 'hidden');
        if (scenario === 'cancel-during-publish') assert.equal(entry().status, 'cancelled');
        assert.ok(calls.some(([method, path]) => method === 'DELETE' && path.endsWith('/100')));
        assert.equal((await service.fetch(request(`/skins/${id}/skin.phskin`), env)).status, 404);
      }
    });
  }
});
