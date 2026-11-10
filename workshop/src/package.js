import {sanitizePng, base64} from './images.js';
import catalog from './catalog.json' with {type:'json'};

export async function publicPackage(input) {
  const draft = input.draft;
  if (!draft || !Object.hasOwn(catalog.models, draft.modelPath) || !Array.isArray(draft.textures)
    || !draft.textures.length || draft.textures.length > 12 || draft.archivedFiles?.length) throw new Error('Choose an editable supported draft with 1–12 textures');
  const slots = catalog.models[draft.modelPath].slots;
  const parts = new Set(catalog.models[draft.modelPath].parts);
  const textures = [], used = new Set(), owners = new Set([draft.modelPath]);
  for (const texture of draft.textures) {
    if (!texture || !parts.has(texture.partPath)) throw new Error('Material belongs to an incompatible attachment');
    const allowed = catalog.materials[`${texture.partPath}|${texture.vmtPath}`];
    if (!allowed || texture.vtfPath !== allowed.vtf || used.has(texture.vmtPath)) throw new Error('Unsupported or duplicate workshop material');
    const image = await sanitizePng(texture.png);
    if (image.width !== allowed.width || image.height !== allowed.height) throw new Error('Texture dimensions do not match approved content');
    used.add(texture.vmtPath); owners.add(texture.partPath);
    textures.push({key:texture.vmtPath, partPath:texture.partPath, vmtPath:texture.vmtPath, vtfPath:allowed.vtf,
      vmtText:`VertexLitGeneric\n{\n "$basetexture" "${allowed.vtf.slice(10, -4)}"\n}\n`,
      png:`data:image/png;base64,${base64(image.bytes)}`, stripAlpha:texture.stripAlpha === true});
  }
  const selections = {};
  for (const [slot, value] of Object.entries(draft.slotSelections || {})) {
    if (!Object.hasOwn(slots, slot)) throw new Error('Unsupported attachment slot');
    if (!slots[slot].includes(value) && value !== null) throw new Error('Unsupported attachment selection');
    selections[slot] = value;
  }
  const preview = await sanitizePng(input.preview);
  if (preview.width > 1024 || preview.height > 1024) throw new Error('Preview must be at most 1024 pixels per side');
  return {draft:{version:1, id:null, name:input.title, modelPath:draft.modelPath, slotSelections:selections,
    activePartPath:owners.has(draft.activePartPath) ? draft.activePartPath : draft.modelPath,
    activeMaterialKey:textures[0].key, textures, archivedFiles:[]}, preview:preview.bytes};
}
