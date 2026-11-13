const encoder = new TextEncoder();
const signature = new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]);
const table = Array.from({length:256}, (_, n) => {
  for (let k = 0; k < 8; k++) n = n & 1 ? 0xedb88320 ^ (n >>> 1) : n >>> 1;
  return n >>> 0;
});
function crc(bytes) {
  let c = 0xffffffff;
  for (const byte of bytes) c = table[(c ^ byte) & 255] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function join(parts) {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) { out.set(part, offset); offset += part.length; }
  return out;
}
function chunk(type, data) {
  const out = new Uint8Array(data.length + 12), view = new DataView(out.buffer);
  view.setUint32(0, data.length);
  out.set(encoder.encode(type), 4); out.set(data, 8);
  view.setUint32(data.length + 8, crc(out.subarray(4, data.length + 8)));
  return out;
}
export async function boundedBytes(stream, maximum) {
  const reader = stream.getReader(), parts = [];
  let length = 0;
  try {
    while (true) {
      const {done, value} = await reader.read();
      if (done) break;
      length += value.length;
      if (length > maximum) throw new Error('Content exceeds its size limit');
      parts.push(value);
    }
  } finally { await reader.cancel(); reader.releaseLock(); }
  return join(parts);
}
export function decodeBase64(text, maximum) {
  if (typeof text !== 'string') throw new Error('Missing PNG');
  text = text.replace(/^data:image\/png;base64,/, '');
  if (text.length > Math.ceil(maximum / 3) * 4 || !/^[A-Za-z0-9+/]*={0,2}$/.test(text)) throw new Error('Invalid PNG encoding or size');
  return Uint8Array.from(atob(text), c => c.charCodeAt(0));
}
export function base64(bytes) {
  let text = '';
  for (let i = 0; i < bytes.length; i += 8192) text += String.fromCharCode(...bytes.subarray(i, i + 8192));
  return btoa(text);
}
export async function sanitizePng(text) {
  const bytes = decodeBase64(text, 4 * 1024 * 1024);
  if (bytes.length < 45 || !signature.every((value, i) => bytes[i] === value)) throw new Error('Invalid PNG signature');
  let offset = 8, chunks = 0, header, ended = false, started = false, closed = false, width, height, stride;
  const data = [];
  while (offset < bytes.length) {
    if (++chunks > 1024) throw new Error('Too many PNG chunks');
    if (offset + 12 > bytes.length) throw new Error('Truncated PNG');
    const view = new DataView(bytes.buffer, bytes.byteOffset + offset);
    const length = view.getUint32(0), end = offset + length + 12;
    if (end > bytes.length) throw new Error('Truncated PNG chunk');
    const type = String.fromCharCode(...bytes.subarray(offset + 4, offset + 8));
    if (crc(bytes.subarray(offset + 4, end - 4)) !== view.getUint32(length + 8)) throw new Error('PNG checksum mismatch');
    const value = bytes.subarray(offset + 8, end - 4);
    if (!header && type !== 'IHDR') throw new Error('PNG must start with IHDR');
    if (type === 'IHDR') {
      if (header || length !== 13) throw new Error('Invalid PNG header');
      header = value;
      const dimensions = new DataView(value.buffer, value.byteOffset);
      width = dimensions.getUint32(0); height = dimensions.getUint32(4);
      if (!width || !height || width > 4096 || height > 4096 || value[8] !== 8
        || ![2, 6].includes(value[9]) || value[10] || value[11] || value[12]) {
        throw new Error('Workshop requires non-interlaced RGB/RGBA 8-bit PNGs up to 4096 x 4096');
      }
      stride = width * (value[9] === 6 ? 4 : 3) + 1;
    } else if (type === 'IDAT') {
      if (closed) throw new Error('Noncontiguous PNG data');
      started = true; data.push(value);
    } else if (type === 'IEND') {
      if (length || !started || end !== bytes.length) throw new Error('Invalid PNG end');
      ended = true;
    } else {
      if (started) closed = true;
      if (!/^[a-z][A-Za-z]{3}$/.test(type)) throw new Error('Unsupported PNG chunk');
    }
    offset = end;
  }
  if (!ended) throw new Error('Missing PNG end');
  const compressed = join(data);
  const decoded = await boundedBytes(new Blob([compressed]).stream().pipeThrough(new DecompressionStream('deflate')), stride * height);
  if (decoded.length !== stride * height) throw new Error('Invalid PNG pixel length');
  for (let row = 0; row < height; row++) if (decoded[row * stride] > 4) throw new Error('Invalid PNG row filter');
  return {bytes:join([signature, chunk('IHDR', header), chunk('IDAT', compressed), chunk('IEND', new Uint8Array())]), width, height};
}
