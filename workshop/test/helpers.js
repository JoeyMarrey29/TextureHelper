import {DatabaseSync} from 'node:sqlite';
import {readFileSync} from 'node:fs';
import {deflateSync} from 'node:zlib';
import {createHash} from 'node:crypto';
import catalog from '../src/catalog.json' with {type:'json'};

export const origin = 'https://workshop.example';
export const actor = '76561198000000000';
export const token = 'b'.repeat(64);
export const hash = value => createHash('sha256').update(value).digest('hex');

export function png(width = 1, height = 1, {filter = 0, metadata = false, decodedLength} = {}) {
  function chunk(name, data) {
    const bytes = Buffer.alloc(data.length + 12);
    bytes.writeUInt32BE(data.length); bytes.write(name, 4); data.copy(bytes, 8);
    let crc = 0xffffffff;
    for (const byte of bytes.subarray(4, -4)) {
      crc ^= byte;
      for (let bit = 0; bit < 8; bit++) crc = crc & 1 ? 0xedb88320 ^ (crc >>> 1) : crc >>> 1;
    }
    bytes.writeUInt32BE((crc ^ 0xffffffff) >>> 0, bytes.length - 4);
    return bytes;
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width); header.writeUInt32BE(height, 4); header[8] = 8; header[9] = 6;
  const rows = Buffer.alloc(decodedLength ?? (width * 4 + 1) * height);
  for (let row = 0; row < height && row * (width * 4 + 1) < rows.length; row++) rows[row * (width * 4 + 1)] = filter;
  return Buffer.concat([Buffer.from([137,80,78,71,13,10,26,10]), chunk('IHDR', header),
    ...(metadata ? [chunk('tEXt', Buffer.from('Author\0private metadata'))] : []),
    chunk('IDAT', deflateSync(rows)), chunk('IEND', Buffer.alloc(0))]);
}

export function submission() {
  const [key, material] = Object.entries(catalog.materials)
    .filter(([key, value]) => catalog.models[key.split('|')[0]] && value.width <= 2048 && value.height <= 2048)
    .sort((a, b) => a[1].width * a[1].height - b[1].width * b[1].height)[0];
  if (!material) throw new Error('Generate the trusted catalogue before testing publication');
  const [modelPath, vmtPath] = key.split('|');
  return {title:'Test community skin', description:'Local test only', consent:true, license:'CC-BY-4.0',
    preview:png().toString('base64'),
    draft:{modelPath, textures:[{partPath:modelPath, vmtPath, vtfPath:material.vtf,
      png:png(material.width, material.height, {metadata:true}).toString('base64'),
      vmtText:'UntrustedShader { "$include" "evil" }'}], slotSelections:{}, aiOriginals:['private']}};
}

export function database() {
  const db = new DatabaseSync(':memory:');
  db.exec(readFileSync(new URL('../migrations/0001_workshop.sql', import.meta.url), 'utf8'));
  db.exec(readFileSync(new URL('../migrations/0002_profiles.sql', import.meta.url), 'utf8'));
  db.exec(readFileSync(new URL('../migrations/0003_failure_reasons.sql', import.meta.url), 'utf8'));
  return {
    db,
    prepare(sql) {
      return {bind(...values) {
        const run = () => db.prepare(sql);
        return {
          first:async () => run().get(...values) ?? null,
          all:async () => ({results:run().all(...values)}),
          run:async () => ({meta:run().run(...values)}),
        };
      }};
    },
    async batch(statements) {
      db.exec('BEGIN');
      try {
        const results = [];
        for (const statement of statements) results.push(await statement.run());
        db.exec('COMMIT'); return results;
      } catch (error) { db.exec('ROLLBACK'); throw error; }
    }
  };
}

export function environment(t) {
  const DB = database();
  t.after(() => DB.db.close());
  DB.db.prepare('INSERT INTO sessions VALUES (?,?,?)').run(hash(token), actor, Math.floor(Date.now() / 1000) + 3600);
  DB.db.prepare('INSERT INTO profiles VALUES (?,?,?,?)').run(actor, 'Test uploader',
    `https://avatars.steamstatic.com/${'a'.repeat(40)}_medium.jpg`, Math.floor(Date.now() / 1000) + 3600);
  return {DB, PUBLIC_ORIGIN:origin, MODERATOR_STEAM_ID:'76561198000000001', PUBLISHING_ENABLED:'false'};
}

export function request(path, {body, authenticated = true, method = body === undefined ? 'GET' : 'POST', headers = {}} = {}) {
  return new Request(origin + path, {method, headers:{...(authenticated ? {Authorization:`Bearer ${token}`} : {}),
    ...headers}, ...(body === undefined ? {} : {body:JSON.stringify(body)})});
}
