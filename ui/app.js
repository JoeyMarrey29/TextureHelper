import * as THREE from './vendor/three.module.js';
import {attachmentData} from './attachment-data.js';
import {startUpdates} from './updates.js';

const invoke = (command, args = {}) => window.__TAURI__.core.invoke(command, args);
const $ = id => document.getElementById(id);
const canvas = $('viewport');
const scene = new THREE.Scene();
const camera = new THREE.PerspectiveCamera(45, 1, 0.01, 100000);
// The canvas is opaque so additive materials (sight reticles) blend onto real pixels instead of
// leaving zero-alpha texels over the page. The backdrop reproduces the .viewport-wrap CSS gradient.
const renderer = new THREE.WebGLRenderer({canvas, antialias: true, alpha: false});
renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
renderer.outputColorSpace = THREE.SRGBColorSpace;
scene.background = viewportBackdrop();
function viewportBackdrop() {
  // CSS: radial-gradient(ellipse at 50% 45%, #29303a 0%, #181d24 42%, #11151b 80%), farthest-corner.
  // Drawn in normalized box space so stretching the texture to the canvas gives the same ellipse.
  const size = 512, backdrop = document.createElement('canvas');
  backdrop.width = backdrop.height = size;
  const context = backdrop.getContext('2d');
  const scale = Math.hypot(1, .55 / .45);
  context.translate(.5 * size, .45 * size);
  context.scale(.5 * scale * size, .45 * scale * size);
  const gradient = context.createRadialGradient(0, 0, 0, 0, 0, 1);
  gradient.addColorStop(0, '#29303a'); gradient.addColorStop(.42, '#181d24'); gradient.addColorStop(.8, '#11151b'); gradient.addColorStop(1, '#11151b');
  context.fillStyle = gradient;
  context.fillRect(-2, -2, 4, 4);
  const texture = new THREE.CanvasTexture(backdrop);
  texture.colorSpace = THREE.SRGBColorSpace;
  return texture;
}
scene.add(new THREE.HemisphereLight(0xffffff, 0x546174, 2.2));
const keyLight = new THREE.DirectionalLight(0xffffff, 2.5);
keyLight.position.set(4, 6, 8);
scene.add(keyLight);
const group = new THREE.Group();
scene.add(group);
let model = null, models = [], customSkins = [], selectedPath = null, materialIndex = 0, importedPng = null, importedImage = null;
const assembly = new Map(), edits = new Map();
const hiddenUvLayers = new Set();
const uvColors = ['#ff765c', '#62c4ff', '#b799ff', '#79d99a', '#ffd166', '#ff8fce'];
const partMounts = new Map();
let highlightedMaterial = null;
const selectionOverlayMaterials = uvColors.map(color => new THREE.MeshBasicMaterial({
  color, transparent: true, opacity: .65, depthWrite: false,
  polygonOffset: true, polygonOffsetFactor: -1, polygonOffsetUnits: -1,
}));
const slotSelections = new Map(), partCache = new Map();
let activePartPath = null, selectionSerial = 0, loadoutSerial = 0, resolvedLoadout = new Map();
let libraryView = 'models', category = 'All', compatibleOnly = true, contentScanned = false, installAvailable = false;
let scanSummary = 'Choose Garry\'s Mod to index installed addons and Workshop content.';
let drafts = [], draftSession = null, draftSaveQueue = Promise.resolve();
let distance = 10, fitDistance = 10, center = new THREE.Vector3();
const slotOrder = Object.keys(attachmentData.slots);
const attachmentCategories = {
  SLOT_SIGHT: 'Sight', SLOT_STOCK: 'Stock', SLOT_SILENCER: 'Muzzle',
  SLOT_UNDERBARREL: 'Underbarrel', SLOT_LEFT_RAIL: 'Laser', SLOT_MAGAZINE: 'Magazine',
  SLOT_SCOPE_RINGS: 'Rails / Mounts', SLOT_TOP_RAIL: 'Rails / Mounts',
  SLOT_IRON_FRONT: 'Iron sights', SLOT_IRON_REAR: 'Iron sights', SLOT_BARREL: 'Barrel',
};
const categoryByModel = new Map(Object.values(attachmentData.attachments)
  .map(info => [info.model, attachmentCategories[info.slot] || 'Attachments']));
const imageCache = new Map();
function texturePreviewImage(source, showAlpha = false) {
  if (!source) return null;
  const key = `${showAlpha ? 'rgba' : 'rgb'}:${source}`;
  if (!imageCache.has(key)) {
    const image = new Image(); imageCache.set(key, image);
    image.onload = () => renderMaterials();
    image.onerror = () => toast('Could not display the texture preview.', true);
    if (showAlpha) image.src = source;
    else invoke('texture_rgb_preview', {pngBase64: source})
      .then(png => { image.src = png; })
      .catch(error => toast(`Could not prepare the RGB texture preview: ${error}`, true));
  }
  return imageCache.get(key);
}
function configureSourceTexture(texture) {
  // Source model UVs use the image's top edge as V=0. Three.js flips image
  // uploads by default, which puts the opposite half of a skin on each face.
  texture.flipY = false;
  texture.colorSpace = THREE.SRGBColorSpace;
  texture.wrapS = texture.wrapT = THREE.RepeatWrapping;
  return texture;
}
function vmtValue(text, name) {
  return text?.match(new RegExp(`^\\s*"?\\$${name}"?\\s+"?([^"\\s]+)`, 'im'))?.[1];
}
function applySourceMaterialFlags(material, info) {
  const enabled = value => /^(1|true|yes)$/i.test(value || '');
  const additive = enabled(vmtValue(info.vmtText, 'additive'));
  if (enabled(vmtValue(info.vmtText, 'alphatest'))) {
    const reference = vmtValue(info.vmtText, 'alphatestreference');
    const threshold = Number(reference);
    material.alphaTest = reference !== undefined && Number.isFinite(threshold) ? Math.max(0, Math.min(threshold, 1)) : .5;
  }
  if (additive) {
    material.blending = THREE.CustomBlending;
    material.blendSrc = THREE.SrcAlphaFactor;
    material.blendDst = THREE.OneFactor;
    // Source $additive: black texels add nothing; coverage stays that of the opaque backdrop or part behind.
    material.blendSrcAlpha = THREE.ZeroFactor;
    material.blendDstAlpha = THREE.OneFactor;
  }
  if (additive || enabled(vmtValue(info.vmtText, 'translucent'))) {
    material.transparent = true;
    material.depthWrite = false;
  }
  return material;
}
function createSourceMaterial(info, color = 0xffffff) {
  const material = /^\s*"?UnlitGeneric"?\s*\{/im.test(info.vmtText || '')
    ? new THREE.MeshBasicMaterial({color, side: THREE.DoubleSide})
    : new THREE.MeshStandardMaterial({color, side: THREE.DoubleSide, roughness: .85});
  return applySourceMaterialFlags(material, info);
}
const thumbnailCache = new Map(), thumbnailPending = new Set(), thumbnailQueue = [];
const draftThumbnailVersions = new Map();
let thumbnailWorking = false;
const thumbnailCanvas = document.createElement('canvas'); thumbnailCanvas.width = thumbnailCanvas.height = 80;
let thumbnailRenderer;
const thumbnailObserver = new IntersectionObserver(entries => {
  for (const entry of entries) {
    if (!entry.isIntersecting) continue;
    thumbnailObserver.unobserve(entry.target);
    const path = entry.target.dataset.thumbnailKey || entry.target.dataset.path;
    if (!thumbnailCache.has(path) && !thumbnailPending.has(path)) { thumbnailPending.add(path); thumbnailQueue.push({path, target: entry.target}); processThumbnails(); }
  }
}, {root: $('model-list'), rootMargin: '100px'});

async function processThumbnails() {
  if (thumbnailWorking) return;
  thumbnailWorking = true;
  while (thumbnailQueue.length) {
    const {path, target} = thumbnailQueue.shift();
    try {
      if (!target.isConnected) { thumbnailPending.delete(path); continue; }
      const draft = target.dataset.draftId ? await invoke('load_draft', {id:target.dataset.draftId}) : null;
      const detail = draft ? await draftThumbnailModel(draft) : await invoke('open_model', {path});
      const dataUrl = await makeThumbnail(detail);
      thumbnailCache.set(path, dataUrl);
      if (target.isConnected) { const image = document.createElement('img'); image.src = dataUrl; image.alt = ''; target.querySelector('.model-thumb').replaceChildren(image); }
      for (const card of $('model-list').querySelectorAll('[data-thumbnail-key]')) {
        if (card.dataset.thumbnailKey === path) {
          const image = document.createElement('img'); image.src = dataUrl; image.alt = '';
          card.querySelector('.model-thumb').replaceChildren(image);
        }
      }
    } catch (error) {
      console.warn(`Thumbnail unavailable for ${path}`, error);
      if (target.isConnected) { target.querySelector('.model-thumb').textContent = 'Preview unavailable'; target.title = `Preview unavailable: ${error}`; }
    }
    thumbnailPending.delete(path);
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  thumbnailWorking = false;
}

async function draftThumbnailModel(draft) {
  const gun = await invoke('open_model', {path:draft.modelPath});
  const parts = [{detail:gun, matrix:new THREE.Matrix4()}];
  const weapon = attachmentData.weapons[draft.modelPath];
  if (weapon) {
    const resolved = resolveLoadout(weapon, new Map(Object.entries(draft.slotSelections)));
    for (const entry of resolved.values()) {
      const info = attachmentInfo(entry.name);
      const sequence = info.sequences?.[resolved.has('SLOT_SIGHT') ? 'withSight' : 'withoutSight'];
      const detail = await loadPart(info.model, sequence);
      const mount = bonemergeMount(gun, detail);
      if (!mount) throw new Error(`No mounting bone for ${entry.name}`);
      parts.push({detail, matrix:mount.matrix});
    }
  }
  const materials = [], meshes = [];
  for (const {detail, matrix} of parts) {
    const offset = materials.length;
    materials.push(...detail.materials.map(material => {
      const edit = draft.textures.find(texture => texture.partPath === detail.path && texture.vmtPath === material.vmtPath)
        || draft.textures.find(texture => texture.vtfPath && texture.vtfPath === material.vtfPath);
      return edit ? {...material, image:edit.png} : material;
    }));
    for (const mesh of detail.meshes) {
      const positions = new THREE.Float32BufferAttribute(mesh.positions, 3);
      const normals = new THREE.Float32BufferAttribute(mesh.normals, 3);
      positions.applyMatrix4(matrix); normals.applyNormalMatrix(new THREE.Matrix3().getNormalMatrix(matrix));
      meshes.push({...mesh, materialIndex:mesh.materialIndex + offset, positions:positions.array, normals:normals.array});
    }
  }
  return {materials, meshes};
}

async function makeThumbnail(detail) {
  if (!thumbnailRenderer) {
    thumbnailRenderer = new THREE.WebGLRenderer({canvas: thumbnailCanvas, antialias: true, preserveDrawingBuffer: true});
    thumbnailRenderer.setSize(80, 80, false);
    thumbnailRenderer.outputColorSpace = THREE.SRGBColorSpace;
  }
  const thumbScene = new THREE.Scene(); thumbScene.background = new THREE.Color(0x2a3139);
  thumbScene.add(new THREE.HemisphereLight(0xffffff, 0x697482, 2.4));
  const light = new THREE.DirectionalLight(0xffffff, 2.2); light.position.set(3, 5, 8); thumbScene.add(light);
  const pivot = new THREE.Group(); thumbScene.add(pivot);
  const bounds = new THREE.Box3();
  for (const mesh of detail.meshes) {
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute('position', new THREE.Float32BufferAttribute(mesh.positions, 3));
    geometry.setAttribute('normal', new THREE.Float32BufferAttribute(mesh.normals, 3));
    geometry.setAttribute('uv', new THREE.Float32BufferAttribute(mesh.uvs, 2));
    geometry.computeBoundingBox(); bounds.union(geometry.boundingBox);
    const materialInfo = detail.materials[mesh.materialIndex];
    const material = createSourceMaterial(materialInfo, 0xb8c2cb);
    if (materialInfo.image) {
      try {
        const texture = configureSourceTexture(await new THREE.TextureLoader().loadAsync(materialInfo.image));
        material.color.setHex(0xffffff); material.map = texture;
      } catch { /* Keep the neutral material. */ }
    }
    pivot.add(new THREE.Mesh(geometry, material));
  }
  const center = bounds.getCenter(new THREE.Vector3());
  for (const object of pivot.children) object.position.copy(center).multiplyScalar(-1);
  pivot.rotation.set(.25, .6, 0);
  const size = Math.max(bounds.getSize(new THREE.Vector3()).length(), .01);
  const thumbCamera = new THREE.PerspectiveCamera(45, 1, .001, size * 100);
  thumbCamera.position.set(0, 0, size * 1.8); thumbCamera.lookAt(0, 0, 0);
  thumbnailRenderer.render(thumbScene, thumbCamera);
  const dataUrl = thumbnailCanvas.toDataURL('image/png');
  for (const object of pivot.children) { object.geometry.dispose(); object.material.map?.dispose(); object.material.dispose(); }
  return dataUrl;
}

function toast(message, error = false) {
  const el = $('toast');
  el.textContent = message;
  el.className = `toast${error ? ' error' : ''}`;
  clearTimeout(toast.timeout);
  toast.timeout = setTimeout(() => el.classList.add('hidden'), 6000);
}

function resize() {
  const rect = canvas.getBoundingClientRect();
  if (!rect.width || !rect.height) return;
  renderer.setSize(rect.width, rect.height, false);
  camera.aspect = rect.width / rect.height;
  camera.updateProjectionMatrix();
}
new ResizeObserver(resize).observe(canvas);
function frame() {
  requestAnimationFrame(frame);
  camera.position.set(0, 0, distance);
  camera.lookAt(0, 0, 0);
  renderer.render(scene, camera);
}
frame();
const viewBases = {
  front: [[0, 0, 1], [1, 0, 0], [0, 1, 0]],
  side: [[-1, 0, 0], [0, 0, 1], [0, 1, 0]],
  top: [[0, 1, 0], [-1, 0, 0], [0, 0, 1]]
};
function setView(view) {
  if (!model) return;
  const basis = viewBases[view === 'default' ? 'front' : view];
  group.quaternion.setFromRotationMatrix(new THREE.Matrix4().makeBasis(...basis.map(axis => new THREE.Vector3(...axis))));
  if (view === 'default') {
    group.quaternion.premultiply(new THREE.Quaternion().setFromEuler(new THREE.Euler(.18, .5, 0, 'YXZ')));
    distance = fitDistance;
  }
}
document.querySelectorAll('[data-view]').forEach(button => button.addEventListener('click', () => setView(button.dataset.view)));
function trackballPoint(event) {
  const rect = canvas.getBoundingClientRect(), radius = Math.min(rect.width, rect.height) * .5;
  const x = (event.clientX - rect.left - rect.width * .5) / radius;
  const y = (rect.top + rect.height * .5 - event.clientY) / radius;
  const r2 = x * x + y * y;
  return r2 <= 1 ? new THREE.Vector3(x, y, Math.sqrt(1 - r2)) : new THREE.Vector3(x, y, 0).normalize();
}
let drag = null;
let selectionClick = null;
const partRaycaster = new THREE.Raycaster();
function selectTextureLayer(layer) {
  activePartPath = layer.part.path; materialIndex = layer.index;
  highlightLayers(activeTextureLayers());
  renderRelated(); renderMaterials();
  saveDraft().catch(error => toast(`Draft was not saved: ${error}`, true));
}
function pickTexture(event) {
  const rect = canvas.getBoundingClientRect();
  camera.position.set(0, 0, distance); camera.lookAt(0, 0, 0);
  camera.updateMatrixWorld(); group.updateMatrixWorld(true);
  partRaycaster.setFromCamera(new THREE.Vector2(
    (event.clientX - rect.left) / rect.width * 2 - 1,
    1 - (event.clientY - rect.top) / rect.height * 2), camera);
  const hit = partRaycaster.intersectObjects(group.children, false)[0];
  if (!hit) { highlightedMaterial = null; updateMaterialHighlight(); return; }
  const part = assembly.get(hit.object.userData.partPath);
  const index = hit.object.userData.materialIndex;
  if (!part?.materials[index]) { toast('The selected mesh has no material to inspect.', true); return; }
  selectTextureLayer({part, material: part.materials[index], index});
}
canvas.addEventListener('contextmenu', event => event.preventDefault());
canvas.addEventListener('pointerdown', event => {
  if (!model || (event.button !== 0 && event.button !== 2)) return;
  if (drag || selectionClick) return;
  if (event.button === 0) selectionClick = {id: event.pointerId, x: event.clientX, y: event.clientY, moved: false};
  else {
    drag = {id: event.pointerId, mode: event.shiftKey ? 'roll' : 'trackball', point: trackballPoint(event), x: event.clientX};
    canvas.classList.add('rotating');
  }
  canvas.setPointerCapture(event.pointerId);
});
function endPointer(event) {
  if (drag?.id === event.pointerId) { drag = null; canvas.classList.remove('rotating'); }
  if (selectionClick?.id === event.pointerId) {
    const click = selectionClick; selectionClick = null;
    if (event.type === 'pointerup' && !click.moved
      && Math.hypot(event.clientX - click.x, event.clientY - click.y) <= 5) pickTexture(event);
  }
  if (canvas.hasPointerCapture(event.pointerId)) canvas.releasePointerCapture(event.pointerId);
}
canvas.addEventListener('pointerup', endPointer);
canvas.addEventListener('pointercancel', endPointer);
canvas.addEventListener('lostpointercapture', endPointer);
canvas.addEventListener('pointermove', event => {
  if (selectionClick?.id === event.pointerId
    && Math.hypot(event.clientX - selectionClick.x, event.clientY - selectionClick.y) > 5) selectionClick.moved = true;
  if (!drag || drag.id !== event.pointerId) return;
  if (drag.mode === 'roll') {
    const angle = (event.clientX - drag.x) * .01;
    group.quaternion.premultiply(new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(0, 0, 1), angle));
  } else {
    const point = trackballPoint(event);
    group.quaternion.premultiply(new THREE.Quaternion().setFromUnitVectors(drag.point, point));
    drag.point = point;
  }
  drag.x = event.clientX;
});
canvas.addEventListener('wheel', event => { event.preventDefault(); distance = Math.max(.05, distance * (event.deltaY > 0 ? 1.12 : .89)); }, {passive: false});

function clearGroup() {
  for (const object of [...group.children]) {
    object.geometry?.dispose(); object.material?.map?.dispose(); object.material?.dispose(); group.remove(object);
  }
}

function updateMaterialHighlight() {
  for (const object of group.children) {
    for (const child of [...object.children]) if (child.userData.selectionOverlay) object.remove(child);
    const selected = highlightedMaterial?.find(item => item.path === object.userData.partPath
      && item.index === object.userData.materialIndex);
    if (!selected) continue;
    // Reuse the mounted geometry without replacing or mutating the texture material.
    const overlay = new THREE.Mesh(object.geometry, selectionOverlayMaterials[selected.colorIndex]);
    overlay.userData.selectionOverlay = true;
    overlay.renderOrder = 1;
    object.add(overlay);
  }
}

function skinKey(material) { return material.vmtPath || material.vtfPath; }
function textureEdit(material) {
  return edits.get(skinKey(material)) || (material.vtfPath
    ? [...edits.values()].find(edit => edit.vtfPath?.toLowerCase() === material.vtfPath.toLowerCase()) : null);
}
function currentEdit() { return textureEdit(activeMaterial()); }
function textureKey(material) { return material.vtfPath?.toLowerCase() || skinKey(material); }
function textureGroups() {
  const groups = new Map();
  for (const part of assembly.values()) part.materials.forEach((material, index) => {
    const key = textureKey(material) || `${part.path}:${index}`;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push({part, material, index});
  });
  return [...groups.values()];
}
function activeTextureLayers() {
  return textureGroups().find(layers => layers.some(layer =>
    layer.part.path === activePartPath && layer.index === materialIndex)) || [];
}
function uvLayerKey(layer) { return `${layer.part.path}:${layer.index}`; }
function layerLabel(layer) { return `${attachmentName(layer.part)} · ${layer.material.name}`; }
function uvColorIndex(layer) {
  const siblings = textureGroups().find(layers => layers.some(item => uvLayerKey(item) === uvLayerKey(layer)));
  return siblings.findIndex(item => uvLayerKey(item) === uvLayerKey(layer)) % uvColors.length;
}
function highlightLayers(layers) {
  highlightedMaterial = layers.map(layer => ({path: layer.part.path, index: layer.index, colorIndex: uvColorIndex(layer)}));
  updateMaterialHighlight();
}
function skinTargetModel(path) { return /^models\/perpheads\/weapons\/(?:c_[^/]+|attachments\/[^/]+|magazines\/[^/]+)\.mdl$/i.test(path || ''); }
function skinMaterialPath(path) {
  return /^(materials\/models\/weapons\/v_models\/|materials\/models\/jarheads\/weapons\/)/.test(path || '')
    && !path.includes('reticles') && !/[\\:]/.test(path)
    && path.split('/').every(part => part && part !== '.' && part !== '..');
}
function supportedMaterial(material, path = activePartPath) {
  return skinTargetModel(path) && skinMaterialPath(material?.vmtPath) && skinMaterialPath(material?.vtfPath);
}
function syncSharedTextureEdits() {
  for (const part of assembly.values()) for (const material of part.materials) {
    const key = skinKey(material), edit = textureEdit(material);
    if (key && edit && !edits.has(key)) edits.set(key, {...edit, key, partPath:part.path,
      vmtPath:material.vmtPath, vtfPath:material.vtfPath, vmtText:material.vmtText});
  }
}
function updateInstallAll() {
  const ready = [...assembly.values()].flatMap(part => part.materials.filter(material => supportedMaterial(material, part.path) && textureEdit(material)));
  $('install-all').disabled = ready.length === 0;
  $('install-all').textContent = `Install all edited textures (${new Set(ready.map(textureKey)).size})`;
}
function renderAssembly() {
  clearGroup();
  const bounds = new THREE.Box3();
  for (const detail of assembly.values()) for (const mesh of detail.meshes) {
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute('position', new THREE.Float32BufferAttribute(mesh.positions, 3));
    geometry.setAttribute('normal', new THREE.Float32BufferAttribute(mesh.normals, 3));
    geometry.setAttribute('uv', new THREE.Float32BufferAttribute(mesh.uvs, 2));
    geometry.computeBoundingBox();
    const info = detail.materials[mesh.materialIndex];
    const object = new THREE.Mesh(geometry, makeMaterial(info));
    object.userData.materialIndex = mesh.materialIndex;
    object.userData.partPath = detail.path;
    object.userData.skinKey = skinKey(info);
    const mount = partMounts.get(detail.path);
    if (mount) { geometry.applyMatrix4(mount.matrix); geometry.computeBoundingBox(); }
    bounds.union(geometry.boundingBox);
    const edit = textureEdit(info);
    if (edit) { object.material.map?.dispose(); object.material.map = configureSourceTexture(new THREE.TextureLoader().load(edit.png)); object.material.needsUpdate = true; }
    group.add(object);
  }
  if (bounds.isEmpty()) return;
  center = bounds.getCenter(new THREE.Vector3());
  for (const object of group.children) object.position.copy(center).multiplyScalar(-1);
  updateMaterialHighlight();
  const size = bounds.getSize(new THREE.Vector3()).length();
  distance = fitDistance = Math.max(size * 1.4, .1);
  camera.near = Math.max(distance / 10000, .001); camera.far = Math.max(distance * 100, 100); camera.updateProjectionMatrix();
  setView('default');
  $('triangle-count').textContent = `${[...assembly.values()].reduce((sum, item) => sum + item.triangleCount, 0).toLocaleString()} triangles`;
}

function makeMaterial(info) {
  const material = createSourceMaterial(info);
  if (info.image) {
    const texture = configureSourceTexture(new THREE.TextureLoader().load(info.image));
    material.map = texture;
    material.needsUpdate = true;
  } else material.color.setHex(0x9eaab7);
  return material;
}

async function displayModel(detail, serial, selections = []) {
  highlightedMaterial = null;
  hiddenUvLayers.clear();
  assembly.clear(); edits.clear(); partMounts.clear(); slotSelections.clear(); partCache.clear(); resolvedLoadout = new Map(); assembly.set(detail.path, detail);
  for (const [slot, name] of selections) slotSelections.set(slot, name);
  draftSession = {id: null, name: `${weaponData(detail)?.name || attachmentName(detail)} draft`, status: ''};
  model = detail; activePartPath = detail.path; importedPng = null; importedImage = null; materialIndex = 0;
  renderAssembly();
  if (weaponData(detail) && !await applyLoadout(serial)) return;
  if (serial !== selectionSerial) return;
  renderAssembly();
  $('viewport-empty').classList.add('hidden');
  $('model-name').textContent = detail.path.split('/').pop().replace(/\.mdl$/i, '');
  $('model-path').textContent = detail.path;
  $('model-origin').textContent = detail.origin;
  $('triangle-count').textContent = `${detail.triangleCount.toLocaleString()} triangles`;
  $('copy-path').disabled = false;
  renderMaterials();
  renderRelated();
  if (!thumbnailCache.has(detail.path)) makeThumbnail(detail).then(dataUrl => {
    thumbnailCache.set(detail.path, dataUrl);
    const button = [...document.querySelectorAll('.model-item')].find(item => item.dataset.path === detail.path);
    if (button) { const image = document.createElement('img'); image.src = dataUrl; image.alt = ''; button.querySelector('.model-thumb').replaceChildren(image); }
  }).catch(error => console.warn('Selected model thumbnail unavailable', error));
}

function weaponData(detail = model) { return detail ? attachmentData.weapons[detail.path.toLowerCase()] : null; }
function attachmentInfo(name) { return attachmentData.attachments[name]; }
function attachmentLabel(name) { const info = attachmentInfo(name); return info?.label || name; }
function installedModel(path) { return models.some(item => item.path.toLowerCase() === path); }

// Mirrors ITEM:GetAttachmentList in PERPHEADS base_weapon.lua: chosen parts, then their direct
// requirements for free slots, then the gun's defaults (and their requirements) for empty slots.
// A null selection explicitly hides a default magazine in the preview.
function resolveLoadout(weapon, selections) {
  const bySlot = new Map();
  for (const [slot, name] of selections) if (name) bySlot.set(slot, {name, reason: 'selected'});
  const addRequirements = name => {
    for (const requirement of weapon.requirements[name] || []) {
      const slot = attachmentInfo(requirement)?.slot;
      if (!slot || selections.get(slot) === null || bySlot.has(slot) || [...bySlot.values()].some(entry => entry.name === requirement)) continue;
      bySlot.set(slot, {name: requirement, reason: `required by ${attachmentLabel(name)}`});
    }
  };
  for (const entry of [...bySlot.values()]) addRequirements(entry.name);
  for (const name of weapon.defaults) {
    const slot = attachmentInfo(name)?.slot;
    if (!slot || selections.get(slot) === null || bySlot.has(slot)) continue;
    bySlot.set(slot, {name, reason: 'gun default'});
    addRequirements(name);
  }
  return bySlot;
}

// In game every part is a ClientsideModel bonemerged (EF_BONEMERGE) onto the viewmodel, so its
// root bone takes the pose of the gun bone with the same name.
function bonemergeMount(gun, part) {
  const root = part.bones?.[0];
  if (!root) return null;
  const name = root.name.toLowerCase();
  const target = gun.bones?.find(bone => bone.name.toLowerCase() === name)
    || gun.attachments?.find(attachment => attachment.name.toLowerCase() === name);
  if (!target) return null;
  const inverseRoot = new THREE.Matrix4().fromArray(root.transform).invert();
  return {name: target.name, matrix: new THREE.Matrix4().fromArray(target.transform).multiply(inverseRoot)};
}

function loadPart(path, sequence) {
  const cacheKey = `${path}:${sequence ?? 'idle'}`;
  if (!partCache.has(cacheKey)) partCache.set(cacheKey, invoke('open_model', {path, sequence})
    .catch(error => { partCache.delete(cacheKey); throw error; }));
  return partCache.get(cacheKey);
}

async function applyLoadout(serial = selectionSerial) {
  const weapon = weaponData(), gun = model;
  if (!weapon || !gun) return false;
  const run = ++loadoutSerial;
  const resolved = resolveLoadout(weapon, slotSelections);
  const loaded = await Promise.all([...resolved].map(async ([slot, entry]) => {
    const info = attachmentInfo(entry.name);
    const sequence = info.sequences?.[resolved.has('SLOT_SIGHT') ? 'withSight' : 'withoutSight'];
    try { return {slot, entry, detail: await loadPart(info.model, sequence)}; }
    catch (error) { return {slot, entry, error: String(error)}; }
  }));
  if (run !== loadoutSerial || serial !== selectionSerial || model !== gun) return false;
  for (const path of [...assembly.keys()]) if (path !== gun.path) assembly.delete(path);
  partMounts.clear();
  const problems = [];
  for (const {entry, detail, error} of loaded) {
    const label = attachmentLabel(entry.name);
    if (error) { entry.problem = 'model could not be loaded'; problems.push(`${label}: ${error}`); continue; }
    const mount = bonemergeMount(gun, detail);
    if (!mount) { entry.problem = `no "${detail.bones?.[0]?.name}" bone on this gun`; problems.push(`${label}: ${entry.problem}`); continue; }
    entry.path = detail.path;
    assembly.set(detail.path, detail);
    partMounts.set(detail.path, mount);
  }
  resolvedLoadout = resolved;
  if (!assembly.has(activePartPath)) { activePartPath = gun.path; materialIndex = 0; }
  if (problems.length) toast(`Some attachments could not be shown. ${problems.join(' · ')}`, true);
  return true;
}

function slotChoices(weapon) {
  const choices = new Map();
  for (const name of weapon.compatible) {
    const slot = attachmentInfo(name)?.slot;
    if (!slot) continue;
    if (!choices.has(slot)) choices.set(slot, []);
    choices.get(slot).push(name);
  }
  for (const name of weapon.defaults) {
    const slot = attachmentInfo(name)?.slot;
    if (slot === 'SLOT_MAGAZINE' && !choices.has(slot)) choices.set(slot, []);
  }
  for (const list of choices.values()) list.sort((a, b) => attachmentLabel(a).localeCompare(attachmentLabel(b)));
  return [...choices].sort(([a], [b]) => slotOrder.indexOf(a) - slotOrder.indexOf(b));
}

function choiceText(name) {
  const info = attachmentInfo(name);
  return info.itemId ? `${info.label} · ID ${info.itemId}` : info.label;
}

function slotSelectionValue(slot) {
  const name = slotSelections.get(slot);
  return name === null ? 'none' : name || '';
}

function renderRelated() {
  const list = $('related-list'); list.replaceChildren();
  const section = $('related-models');
  section.classList.toggle('hidden', !model?.path.includes('/weapons/'));
  if (!model) return;
  const weapon = weaponData();
  if (!weapon) {
    $('attachment-guidance').textContent = 'This model is not a PERPHEADS gun in the attachment data, so no slots are offered.';
    return;
  }
  const slots = slotChoices(weapon);
  for (const [slot, names] of slots) {
    const row = document.createElement('div'); row.className = 'part-row';
    const label = document.createElement('span'); label.textContent = `${attachmentData.slots[slot] || slot}${names.length ? ` (${names.length})` : ''}`;
    const select = document.createElement('select'); select.setAttribute('aria-label', `${attachmentData.slots[slot] || slot} attachment`);
    const fallback = weapon.defaults.find(name => attachmentInfo(name)?.slot === slot);
    const none = document.createElement('option'); none.value = '';
    none.textContent = fallback ? `Default: ${attachmentLabel(fallback)}` : 'No part'; select.append(none);
    if (slot === 'SLOT_MAGAZINE' && fallback) {
      const empty = document.createElement('option'); empty.value = 'none'; empty.textContent = 'No part'; select.append(empty);
    }
    for (const name of names) {
      const info = attachmentInfo(name);
      const option = document.createElement('option'); option.value = name;
      option.textContent = choiceText(name);
      option.title = info.model;
      if (!installedModel(info.model)) { option.disabled = true; option.textContent += ' (model not installed)'; }
      select.append(option);
    }
    select.value = slotSelectionValue(slot);
    const entry = resolvedLoadout.get(slot);
    label.title = entry?.path ? `Showing ${entry.path}` : 'No part mounted';
    select.addEventListener('change', () => setSlotAttachment(slot, select.value));
    row.append(label, select); list.append(row);
  }
  for (const [slot, entry] of [...resolvedLoadout].sort(([a], [b]) => slotOrder.indexOf(a) - slotOrder.indexOf(b))) {
    if (entry.reason === 'selected' || slots.some(([choiceSlot]) => choiceSlot === slot)) continue;
    const row = document.createElement('div'); row.className = 'part-row auto-part';
    const label = document.createElement('span'); label.textContent = attachmentData.slots[slot] || slot;
    const value = document.createElement('em');
    value.textContent = `${attachmentLabel(entry.name)} · ${entry.problem || entry.reason}`;
    value.title = attachmentInfo(entry.name).model;
    row.append(label, value); list.append(row);
  }
  const slotCount = slots.length, partCount = weapon.compatible.length;
  $('attachment-guidance').textContent = slotCount
    ? `${weapon.name} (item ${weapon.itemId}): ${partCount} compatible parts across ${slotCount} slots, from PERPHEADS weaponsystem Lua. Required rails and gun defaults are added automatically; choose No part in Magazine to hide the default magazine. Every part is bonemerged onto the gun bone it uses in game. Left-click a part in the 3D view to edit its texture on the right.`
    : `${weapon.name} (item ${weapon.itemId}) has no attachment slots in PERPHEADS Lua.`;
}

async function setSlotAttachment(slot, value) {
  if (slotSelectionValue(slot) === value) return;
  const name = value === 'none' ? null : value;
  if (value) slotSelections.set(slot, name); else slotSelections.delete(slot);
  if (!await applyLoadout()) return;
  const path = name && attachmentInfo(name).model;
  if (path && assembly.has(path)) activePartPath = path;
  materialIndex = 0;
  renderAssembly(); renderRelated(); renderMaterials();
  try { await saveDraft(); } catch (error) { toast(`Draft was not saved: ${error}`, true); }
}
function attachmentName(item) { return item.path.split('/').pop().replace(/\.mdl$/i, '').replace(/_/g, ' '); }
function renderList() {
  renderDraftActions();
  $('import-skin').classList.toggle('hidden', libraryView !== 'skins');
  $('library-title').textContent = libraryView === 'models' ? 'Models' : 'Custom skins';
  $('search').placeholder = libraryView === 'models' ? 'Search model paths' : 'Search custom skins';
  $('skin-count').textContent = (drafts.length + customSkins.filter(skin => !drafts.some(draft => draft.installedIds?.includes(skin.id))).length).toLocaleString();
  $('category-filters').classList.toggle('hidden', libraryView !== 'models');
  $('compatibility-filter').classList.toggle('hidden', libraryView !== 'models');
  if (libraryView === 'skins') { renderCustomSkins(); return; }
  renderCategories();
  const query = $('search').value.trim().toLowerCase();
  const found = visibleModels().filter(item => (category === 'All' || modelCategory(item.path) === category) && (!query || item.path.includes(query)));
  $('model-count').textContent = found.length.toLocaleString();
  const list = $('model-list'); list.replaceChildren();
  for (const item of found.slice(0, 250)) {
    const button = document.createElement('button');
    button.className = `model-item${selectedPath === item.path ? ' active' : ''}`;
    const title = item.path.split('/').pop().replace(/\.mdl$/i, '');
    const icon = document.createElement('span'); icon.className = 'model-thumb';
    if (thumbnailCache.has(item.path)) { const image = document.createElement('img'); image.src = thumbnailCache.get(item.path); image.alt = ''; icon.append(image); }
    else icon.textContent = '⬡';
    const words = document.createElement('span'); words.className = 'model-words';
    const strong = document.createElement('span'); strong.className = 'model-title'; strong.textContent = title;
    const sub = document.createElement('span'); sub.className = 'model-sub'; sub.textContent = item.path;
    words.append(strong, sub); button.append(icon, words); button.dataset.path = item.path;
    button.addEventListener('click', () => selectModel(item)); list.append(button);
    if (!thumbnailCache.has(item.path)) thumbnailObserver.observe(button);
  }
  $('scan-info').textContent = found.length > 250 ? `Showing 250 of ${found.length.toLocaleString()} results. Search to narrow the list.` : `${scanSummary} · ${compatibleOnly ? 'Guide compatible' : 'All models'}`;
}

function visibleModels() { return compatibleOnly ? models.filter(item => item.compatible) : models; }

function modelCategory(path) {
  const normalized = path.toLowerCase(), parts = normalized.split('/');
  if (categoryByModel.has(normalized)) return categoryByModel.get(normalized);
  if (parts.includes('magazines')) return 'Magazine';
  if (parts.includes('components')) return 'Components';
  if (parts.includes('casing')) return 'Ammunition';
  if (parts.includes('attachments')) return 'Attachments';
  if (attachmentData.weapons[normalized] || parts.some(part => /^(weapons?|wrepons|guns?)$/.test(part))) return 'Weapons';
  if (parts.some(part => /^(vehicles?|cars?|trucks?)$/.test(part))) return 'Vehicles';
  if (parts.some(part => /^(players?|characters?|citizens?|peds?)$/.test(part))) return 'Characters';
  if (parts.some(part => part.startsWith('prop') || part.startsWith('paralake') || part === 'perp2')) return 'Props';
  return 'Other';
}

function renderCategories() {
  const container = $('category-filters'); container.replaceChildren();
  const categories = ['All', 'Weapons', 'Sight', 'Stock', 'Muzzle', 'Magazine', 'Underbarrel',
    'Laser', 'Rails / Mounts', 'Iron sights', 'Barrel', 'Attachments', 'Components',
    'Ammunition', 'Props', 'Vehicles', 'Characters', 'Other'];
  const available = visibleModels();
  if (category !== 'All' && !available.some(item => modelCategory(item.path) === category)) category = 'All';
  for (const name of categories) {
    const count = name === 'All' ? available.length : available.filter(item => modelCategory(item.path) === name).length;
    if (!count && name !== 'All') continue;
    const button = document.createElement('button'); button.type = 'button';
    button.className = category === name ? 'active' : '';
    button.textContent = `${name} ${count}`;
    button.addEventListener('click', () => { category = name; renderList(); });
    container.append(button);
  }
}

function renderCustomSkins() {
  const query = $('search').value.trim().toLowerCase();
  const saved = drafts.filter(draft => `${draft.name} ${draft.modelPath}`.toLowerCase().includes(query));
  const found = customSkins.filter(skin => !drafts.some(draft => draft.installedIds?.includes(skin.id))
    && `${skin.name} ${skin.vmtPath || ''} ${skin.vtfPath || ''}`.toLowerCase().includes(query));
  $('model-count').textContent = (found.length + saved.length).toLocaleString();
  $('scan-info').textContent = 'Share with Export skin or Import skin. Uninstall keeps your saved copy; Delete skin removes it and matching installed files.'
    + (installAvailable ? '' : ' Choose your Garry\'s Mod folder to check installed status or uninstall.');
  const list = $('model-list'); list.replaceChildren();
  for (const draft of saved) {
    const row = document.createElement(draft.archived ? 'div' : 'button');
    row.className = `skin-item${draftSession?.id === draft.id ? ' active' : ''}`; row.dataset.draftId = draft.id;
    if (!draft.archived) {
      row.type = 'button'; row.setAttribute('aria-label', `Open draft: ${draft.name}`);
      row.addEventListener('click', () => openDraft(draft));
      const cacheKey = `draft:${draft.id}:${draftThumbnailVersions.get(draft.id) || 0}`;
      row.dataset.thumbnailKey = cacheKey;
      const preview = document.createElement('span'); preview.className = 'model-thumb draft-thumb';
      if (thumbnailCache.has(cacheKey)) {
        const image = document.createElement('img'); image.src = thumbnailCache.get(cacheKey); image.alt = '';
        preview.append(image);
      } else preview.textContent = 'Loading preview';
      row.append(preview);
      if (!thumbnailCache.has(cacheKey)) thumbnailObserver.observe(row);
    }
    const detail = document.createElement('div'); detail.className = 'skin-detail';
    const name = document.createElement('strong'); name.textContent = draft.name;
    const status = document.createElement('small'); status.className = `skin-status ${draft.installedCount ? 'installed' : 'saved'}`;
    const installed = !installAvailable ? 'Install status unavailable' : draft.installedCount === 0 ? 'Not installed'
      : draft.installedCount === draft.installableCount ? 'Installed' : `Partly installed (${draft.installedCount}/${draft.installableCount})`;
    status.textContent = `${draft.archived ? 'Saved skin backup' : `Saved draft · ${draft.textureCount} edited textures`} · ${installed}`;
    const path = document.createElement('small'); path.className = 'skin-path'; path.textContent = draft.modelPath.split('/').pop() || 'VMT / VTF backup'; path.title = draft.modelPath;
    const actions = document.createElement('div'); actions.className = 'skin-actions';
    const tools = document.createElement('div'); tools.className = 'skin-tools';
    const button = (label, action) => { const item = document.createElement('button'); item.type = 'button'; item.className = 'button secondary';
      item.textContent = label; item.addEventListener('click', action); actions.append(item); return item; };
    if (draft.archived) {
      if (draft.installableCount > draft.installedCount) button('Install', () => restoreSavedSkin(draft)).disabled = !installAvailable;
      if (draft.installedCount) button('Uninstall', () => uninstallSavedDraft(draft));
      const share = button('Export skin', () => exportSkin('export_draft', draft.id)); tools.append(share);
      const remove = button('Delete skin', () => deleteSavedDraft(draft)); remove.classList.add('danger'); tools.append(remove);
    }
    detail.append(name, status, path, actions, tools); row.append(detail); list.append(row);
  }
  for (const skin of found) {
    const row = document.createElement('div'); row.className = 'skin-item';
    const detail = document.createElement('div'); detail.className = 'skin-detail';
    const name = document.createElement('strong'); name.textContent = skin.name;
    const status = document.createElement('small'); status.className = `skin-status${skin.vmtPath && skin.hasVtf ? '' : ' incomplete'}`;
    status.textContent = skin.vmtPath && skin.hasVtf ? 'Installed · no local draft' : skin.vmtPath ? 'VMT only · texture missing' : 'VTF only · material missing';
    detail.append(name, status);
    const paths = document.createElement('details'); paths.className = 'skin-files';
    const summary = document.createElement('summary'); summary.textContent = 'Installed files'; paths.append(summary);
    for (const path of [skin.vmtPath, skin.vtfPath].filter(Boolean)) { const label = document.createElement('small'); label.textContent = path; paths.append(label); }
    detail.append(paths);
    const remove = document.createElement('button'); remove.type = 'button'; remove.className = 'delete-skin'; remove.textContent = 'Delete';
    remove.addEventListener('click', () => deleteSkin(skin));
    const actions = document.createElement('div'); actions.className = 'skin-actions';
    const uninstall = document.createElement('button'); uninstall.type = 'button'; uninstall.className = 'button secondary'; uninstall.textContent = 'Uninstall';
    uninstall.addEventListener('click', () => archiveInstalledSkin(skin));
    const share = document.createElement('button'); share.type = 'button'; share.className = 'button secondary'; share.textContent = 'Export skin';
    share.addEventListener('click', () => exportSkin('export_custom_skin', skin.id));
    const tools = document.createElement('div'); tools.className = 'skin-tools';
    tools.append(share, remove); actions.append(uninstall); detail.append(actions, tools);
    row.append(detail); list.append(row);
  }
}

async function refreshSkins() {
  [customSkins, drafts] = await Promise.all([invoke('list_custom_skins'), invoke('list_drafts')]);
  renderList();
  renderDraftActions();
  if (model) renderMaterials();
}

window.addEventListener('focus', () => {
  refreshSkins().catch(error => toast(`Saved skins could not be refreshed: ${error}`, true));
});

async function exportSkin(command, id) {
  try {
    await saveDraft();
    const path = await invoke(command, {id});
    if (path) toast(`Skin exported: ${path}`);
  } catch (error) { toast(`Skin could not be exported: ${error}`, true); }
}

function openWorkshop() {
  const frame = $('workshop-frame');
  if (!frame.getAttribute('src')) frame.src = 'workshop.html';
  else frame.contentWindow.dispatchEvent(new Event('focus'));
  if (!$('workshop-dialog').open) $('workshop-dialog').showModal();
}
$('open-workshop').addEventListener('click', openWorkshop);
$('close-workshop').addEventListener('click', () => $('workshop-dialog').close());
$('workshop-dialog').addEventListener('close', () => {
  refreshSkins().catch(error => toast(`Saved skins could not be refreshed: ${error}`, true));
});
async function stageWorkshopDraft(draft) {
  try {
    const account = await invoke('workshop_account', {poll:false});
    if (!account.steamId) {
      openWorkshop();
      toast('Sign in with Steam in Skin Workshop, then choose Publish to workshop again.');
      return;
    }
    if (!await openDraft(draft)) throw new Error('Draft could not be opened for preview');
    await saveDraft();
    const overlays = [];
    group.traverse(object => { if (object.userData.selectionOverlay) { overlays.push([object, object.visible]); object.visible = false; } });
    const rotation = group.quaternion.clone(), oldDistance = distance;
    let preview;
    try {
      distance = fitDistance; camera.position.set(0, 0, distance); camera.lookAt(0, 0, 0);
      setView('front'); renderer.render(scene, camera);
      const shot = document.createElement('canvas'), scale = Math.min(1, 1024 / Math.max(canvas.width, canvas.height));
      shot.width = Math.max(1, Math.round(canvas.width * scale)); shot.height = Math.max(1, Math.round(canvas.height * scale));
      shot.getContext('2d').drawImage(canvas, 0, 0, shot.width, shot.height);
      preview = shot.toDataURL('image/png');
    } finally {
      group.quaternion.copy(rotation); distance = oldDistance;
      camera.position.set(0, 0, distance); camera.lookAt(0, 0, 0);
      for (const [object, visible] of overlays) object.visible = visible;
      renderer.render(scene, camera);
    }
    await invoke('workshop_stage', {id:draft.id, preview});
    openWorkshop();
  } catch (error) { toast(`Workshop preview could not be prepared: ${error}`, true); }
}

$('import-skin').addEventListener('click', async () => {
  const button = $('import-skin'); button.disabled = true;
  try {
    const name = await invoke('import_skin');
    if (name !== null) {
      $('search').value = '';
      await refreshSkins();
      toast(`Imported "${name}" as a new saved copy. Nothing was installed.`);
    }
  } catch (error) { toast(`Skin could not be imported: ${error}`, true); }
  finally { button.disabled = false; }
});

function renderDraftStatus() {
  renderDraftActions();
  $('draft-tools').classList.toggle('hidden', !model);
  $('save-draft').disabled = !edits.size;
  if (!draftSession) return;
  if (document.activeElement !== $('draft-name')) $('draft-name').value = draftSession.name;
  $('draft-status').textContent = draftSession.status || 'Import a texture to start autosaving.';
  $('draft-status').classList.toggle('error', draftSession.failed || false);
}

function renderDraftActions() {
  const draft = drafts.find(item => item.id === draftSession?.id && !item.archived);
  $('draft-actions').classList.toggle('hidden', !draft);
  if (!draft) return;
  $('install-draft').hidden = draft.installableCount <= draft.installedCount;
  $('install-draft').disabled = !installAvailable;
  $('uninstall-draft').hidden = !draft.installedCount;
}
for (const [id, action] of [
  ['publish-draft', stageWorkshopDraft],
  ['export-draft', draft => exportSkin('export_draft', draft.id)],
  ['install-draft', restoreSavedSkin],
  ['uninstall-draft', uninstallSavedDraft],
  ['delete-draft', deleteSavedDraft],
]) {
  $(id).addEventListener('click', async () => {
    const draft = drafts.find(item => item.id === draftSession?.id && !item.archived);
    if (!draft) { toast('Open a saved draft first.', true); return; }
    $(id).disabled = true;
    try { await action(draft); }
    finally { $(id).disabled = false; renderDraftActions(); }
  });
}

function saveDraft() {
  if (!model || !edits.size || !draftSession) return draftSaveQueue;
  syncSharedTextureEdits();
  const session = draftSession;
  const snapshot = {
    version: 1, name: session.name, modelPath: model.path, slotSelections: Object.fromEntries(slotSelections),
    ...(session.aiOriginals?.length ? {aiOriginals:session.aiOriginals} : {}),
    ...(session.workshopSource ? {workshopSource:session.workshopSource} : {}),
    activePartPath, activeMaterialKey: skinKey(activeMaterial()) || null,
    textures: [...edits].map(([key, edit]) => ({
      key, partPath: edit.partPath, vmtPath: edit.vmtPath || null,
      vtfPath: edit.vtfPath || null, vmtText: edit.vmtText || null, png: edit.png, stripAlpha: edit.stripAlpha || false,
    })),
  };
  session.status = 'Saving draft…'; session.failed = false; renderDraftStatus();
  const job = draftSaveQueue.then(async () => {
    const saved = await invoke('save_draft', {draft: {...snapshot, id: session.id}});
    session.id = saved.id;
    const previewVersion = draftThumbnailVersions.get(saved.id) || 0;
    thumbnailCache.delete(`draft:${saved.id}:${previewVersion}`);
    draftThumbnailVersions.set(saved.id, previewVersion + 1);
    session.status = 'Saved locally · safe to close'; session.failed = false;
    await refreshSkins();
    if (draftSession === session) renderDraftStatus();
  }).catch(error => {
    session.status = 'Save failed · click Save now to retry'; session.failed = true;
    if (draftSession === session) renderDraftStatus();
    throw error;
  });
  // Keep later retries runnable; the caller still receives and reports the failed save.
  draftSaveQueue = job.catch(() => {});
  return job;
}

$('save-draft').addEventListener('click', () => saveDraft().catch(error => toast(`Draft was not saved: ${error}`, true)));
$('draft-name').addEventListener('change', () => {
  if (!draftSession) return;
  const name = $('draft-name').value.trim();
  if (!name) { toast('Draft name cannot be empty.', true); $('draft-name').value = draftSession.name; return; }
  draftSession.name = name;
  saveDraft().catch(error => toast(`Draft was not saved: ${error}`, true));
});

async function openDraft(summary) {
  try {
    await saveDraft();
    const draft = await invoke('load_draft', {id: summary.id});
    if (!contentScanned) throw new Error('Choose your content or Garry\'s Mod folder before opening a draft.');
    const restored = await Promise.all(draft.textures.map(async texture => {
      const image = new Image(); image.src = texture.png; await image.decode();
      return {...texture, image};
    }));
    const serial = ++selectionSerial;
    const detail = await invoke('open_model', {path: draft.modelPath});
    if (serial !== selectionSerial) return;
    await displayModel(detail, serial, Object.entries(draft.slotSelections));
    if (serial !== selectionSerial) return;
    for (const edit of restored) {
      const material = [...assembly.values()].flatMap(part => part.materials).find(material => skinKey(material) === edit.key);
      if (material?.width && (edit.image.width !== material.width || edit.image.height !== material.height)) {
        throw new Error(`Saved texture ${edit.key} no longer matches the content's ${material.width} × ${material.height} dimensions.`);
      }
    }
    draftSession = {id: draft.id, name: draft.name, status: 'Saved locally · restored', failed: false,
      aiOriginals:draft.aiOriginals || [], workshopSource:draft.workshopSource};
    for (const edit of restored) edits.set(edit.key, edit);
    activePartPath = assembly.has(draft.activePartPath) ? draft.activePartPath : detail.path;
    const index = activePart().materials.findIndex(material => skinKey(material) === draft.activeMaterialKey);
    materialIndex = index < 0 ? 0 : index;
    selectedPath = detail.path;
    renderAssembly(); renderMaterials(); renderRelated(); renderList();
    const unavailable = restored.filter(edit => ![...assembly.values()].some(part =>
      part.materials.some(material => skinKey(material) === edit.key)));
    toast(unavailable.length ? `Draft opened. ${unavailable.length} edited textures belong to parts not currently mounted; those edits remain saved.`
      : 'Draft opened with its attachments and edited textures.');
    return true;
  } catch (error) { toast(`Draft could not be opened: ${error}`, true); return false; }
}

async function uninstallSavedDraft(draft) {
  if (!confirm(`Uninstall "${draft.name}" from Garry's Mod?\nThe saved skin and edits will be kept.`)) return;
  try {
    await saveDraft();
    await invoke('uninstall_draft', {id: draft.id});
    await refreshSkins(); toast('Skin uninstalled. Your saved copy is still available.');
  } catch (error) { toast(String(error), true); }
}

async function archiveInstalledSkin(skin) {
  if (!confirm(`Uninstall "${skin.name}"?\nA local backup will be saved first, so you can reinstall it later.`)) return;
  try {
    await invoke('archive_custom_skin', {id: skin.id});
    await refreshSkins(); toast('Skin uninstalled and saved locally.');
  } catch (error) { toast(String(error), true); }
}

async function restoreSavedSkin(draft) {
  await runSkinInstall(async () => {
    await saveDraft();
    const saved = await invoke('load_draft', {id: draft.id});
    const textures = saved.textures.filter(texture => supportedMaterial(texture, texture.partPath));
    const choices = await alphaInstallChoices(textures);
    if (!choices) return;
    await invoke('install_draft', {id: draft.id, alphaChoices: Object.fromEntries(choices)});
    if (draftSession?.id === draft.id) applyAlphaChoices(choices);
    await refreshSkins(); toast('Saved skin installed.');
  });
}

async function deleteSavedDraft(draft) {
  if (!confirm(`Delete "${draft.name}"?\n${installAvailable ? 'The saved copy and all matching installed textures and attachments will be removed. Shared textures still used by other skins and modified files are kept.' : 'Install location is unavailable: only the saved copy can be removed. Installed game files cannot be checked.'}\nUse Uninstall instead to keep a saved copy. This cannot be undone.`)) return;
  try {
    await draftSaveQueue;
    await invoke('delete_draft', {id: draft.id});
    thumbnailCache.delete(`draft:${draft.id}:${draftThumbnailVersions.get(draft.id) || 0}`);
    draftThumbnailVersions.delete(draft.id);
    if (draftSession?.id === draft.id) {
      draftSession = {id: null, name: `${weaponData()?.name || attachmentName(model)} draft`, status: ''};
      edits.clear(); importedPng = null; importedImage = null;
      renderAssembly();
    }
    await refreshSkins(); toast(installAvailable ? 'Skin deleted, including matching installed files. Shared or modified files were kept.' : 'Saved copy deleted. Install location was unavailable; game files were not checked.');
  } catch (error) { toast(String(error), true); }
}

async function deleteSkin(skin) {
  const paths = [skin.vmtPath, skin.hasVtf && skin.vtfPath].filter(Boolean);
  if (!confirm(`Delete this custom skin?\n\n${paths.join('\n')}\n\nThis cannot be undone.`)) return;
  try { await invoke('delete_custom_skin', {id: skin.id}); await refreshSkins(); toast('Custom skin deleted.'); }
  catch (error) { toast(String(error), true); }
}

document.querySelectorAll('[data-library]').forEach(button => button.addEventListener('click', () => {
  libraryView = button.dataset.library;
  document.querySelectorAll('[data-library]').forEach(tab => tab.classList.toggle('active', tab === button));
  $('search').value = '';
  renderList();
}));

async function selectModel(item) {
  try { await saveDraft(); } catch (error) { toast(`Cannot leave this edit until its draft is saved: ${error}`, true); return; }
  const serial = ++selectionSerial;
  selectedPath = item.path; renderList();
  $('model-name').textContent = 'Loading model…';
  try { const detail = await invoke('open_model', {path: item.path}); if (serial === selectionSerial) await displayModel(detail, serial); }
  catch (error) { $('model-name').textContent = 'Could not load model'; toast(String(error), true); }
}

function renderMaterials() {
  const container = $('materials'); container.replaceChildren();
  const groups = textureGroups(), selector = $('texture-select');
  selector.replaceChildren();
  for (const [groupIndex, layers] of groups.entries()) {
    const {part, material, index} = layers[0];
    const edit = textureEdit(material);
    if (edit) for (const layer of layers) {
      const key = skinKey(layer.material);
      if (key && !edits.has(key)) edits.set(key, {...edit, key, partPath: layer.part.path,
        vmtPath: layer.material.vmtPath, vtfPath: layer.material.vtfPath, vmtText: layer.material.vmtText});
    }
    const active = layers.some(layer => layer.part.path === activePartPath && layer.index === materialIndex);
    const option = document.createElement('option'); option.value = String(groupIndex);
    option.textContent = `${(material.vtfPath || material.name).split('/').pop()} · ${[...new Set(layers.map(layer => attachmentName(layer.part)))].join(' + ')}`;
    option.selected = active; selector.append(option);
    if (!active) continue;
    const button = document.createElement('button'); button.className = `material-item${active ? ' active' : ''}`;
    button.dataset.textureKey = textureKey(material) || `${part.path}:${index}`;
    button.title = layers.map(layerLabel).join('\n');
    const image = document.createElement('img'); image.className = 'material-swatch';
    const preview = texturePreviewImage(edit?.png || material.image);
    if (preview?.naturalWidth) image.src = preview.src;
    const label = document.createElement('span'); const title = document.createElement('strong');
    title.textContent = layers.length > 1 ? (material.vtfPath || material.name).split('/').pop() : `${part.path.split('/').pop().replace(/\.mdl$/i, '')} · ${material.name}`;
    const sub = document.createElement('small');
    const installed = customSkins.some(skin => skin.id === `vmt:${material.vmtPath?.replace(/^materials\//, '')}`);
    const status = material.image ? `${material.width} × ${material.height}` : !material.vmtPath ? 'VMT not found' : !material.vtfOrigin ? 'VTF not found' : 'VTF decode failed';
    sub.textContent = `${status}${layers.length > 1 ? ` · ${layers.length} UV overlays` : ''}${installed ? ' · Custom skin installed' : ''}${edits.has(skinKey(material)) ? ' · Edited' : ''}`;
    if (layers.length > 1) {
      const owners = document.createElement('small');
      owners.textContent = [...new Set(layers.map(layer => attachmentName(layer.part)))].join(' + ');
      label.append(owners);
    }
    label.prepend(title); label.append(sub); button.append(image, label); button.addEventListener('click', () => {
      selectTextureLayer({part: activePart(), material: activeMaterial(), index: materialIndex});
    });
    container.append(button);
  }
  updateInstallAll();
  renderDraftStatus();
  renderTexture();
}
$('texture-select').addEventListener('change', event => {
  const layer = textureGroups()[Number(event.target.value)]?.[0];
  if (layer) selectTextureLayer(layer);
});

function activePart() { return assembly.get(activePartPath); }
function activeMaterial() { return activePart()?.materials[materialIndex]; }
function renderUvLayers() {
  const container = $('uv-layers'); container.replaceChildren();
  activeTextureLayers().forEach(layer => {
    const row = document.createElement('div'); row.className = 'uv-layer';
    const label = document.createElement('label');
    const checkbox = document.createElement('input'); checkbox.type = 'checkbox';
    checkbox.checked = !hiddenUvLayers.has(uvLayerKey(layer));
    checkbox.setAttribute('aria-label', `Show UV: ${layerLabel(layer)}`);
    checkbox.addEventListener('change', () => {
      if (checkbox.checked) hiddenUvLayers.delete(uvLayerKey(layer)); else hiddenUvLayers.add(uvLayerKey(layer));
      renderTexture();
    });
    const swatch = document.createElement('span'); swatch.className = 'uv-color'; swatch.style.background = uvColors[uvColorIndex(layer)];
    const text = document.createElement('span'); text.textContent = layerLabel(layer);
    label.append(checkbox, swatch, text);
    const select = document.createElement('button'); select.type = 'button'; select.textContent = 'Highlight';
    select.title = `Highlight ${layerLabel(layer)} in the 3D view`;
    select.addEventListener('click', () => {
      activePartPath = layer.part.path; materialIndex = layer.index; highlightLayers([layer]);
      renderRelated(); renderMaterials();
      saveDraft().catch(error => toast(`Draft was not saved: ${error}`, true));
    });
    row.append(label, select); container.append(row);
  });
}
function drawUv(context, width, height) {
  context.save();
  context.lineWidth = Math.max(1, width / 512); context.lineJoin = 'round';
  context.beginPath(); context.rect(0, 0, width, height); context.clip();
  activeTextureLayers().forEach(layer => {
    if (hiddenUvLayers.has(uvLayerKey(layer))) return;
    const color = uvColors[uvColorIndex(layer)];
    context.strokeStyle = color;
    context.fillStyle = `${color}20`;
    for (const mesh of layer.part.meshes.filter(item => item.materialIndex === layer.index)) {
    for (let i = 0; i < mesh.uvs.length; i += 6) {
      const points = [0, 2, 4].map(offset => [mesh.uvs[i + offset], mesh.uvs[i + offset + 1]]);
      const uMin = Math.floor(Math.min(...points.map(point => point[0])));
      const uMax = Math.floor(Math.max(...points.map(point => point[0])));
      const vMin = Math.floor(Math.min(...points.map(point => point[1])));
      const vMax = Math.floor(Math.max(...points.map(point => point[1])));
      // Source textures repeat. Draw each tile that the triangle crosses into
      // the visible 0–1 image, matching the 3D preview's RepeatWrapping.
      for (let tileU = uMin; tileU <= uMax; tileU++) for (let tileV = vMin; tileV <= vMax; tileV++) {
        context.beginPath();
        points.forEach(([u, v], vertex) => {
          if (vertex === 0) context.moveTo((u - tileU) * width, (v - tileV) * height);
          else context.lineTo((u - tileU) * width, (v - tileV) * height);
        });
        context.closePath(); context.fill(); context.stroke();
      }
      }
    }
  });
  context.restore();
}

function renderTexture() {
  const material = activeMaterial(), tools = $('texture-tools');
  if (!material) { tools.classList.add('hidden'); return; }
  importedPng = currentEdit()?.png || null;
  importedImage = currentEdit()?.image || null;
  tools.classList.remove('hidden');
  const layers = activeTextureLayers();
  $('material-name').textContent = layers.length > 1 ? (material.vtfPath || material.name).split('/').pop() : material.name;
  renderUvLayers();
  $('texture-size').textContent = material.width ? `${material.width} × ${material.height}` : 'No VTF';
  $('vmt-path').textContent = [...new Set(layers.map(layer => layer.material.vmtPath || 'Not found'))].join('\n');
  $('vmt-origin').textContent = [...new Set(layers.map(layer => layer.material.vmtOrigin || 'Not found'))].join('\n');
  $('vtf-path').textContent = material.vtfPath || 'Not found';
  $('vtf-origin').textContent = material.vtfOrigin || 'Not found';
  $('vmt-text').textContent = [...new Map(layers.map(layer =>
    [skinKey(layer.material) || uvLayerKey(layer), layer.material])).values()]
    .map(info => `${info.vmtPath || info.name}\n${info.vmtText || 'VMT not found'}`).join('\n\n');
  const skinSupported = supportedMaterial(material);
  $('export-vtf').disabled = !importedPng || !skinSupported;
  $('copy-png').disabled = !(importedPng || material.image);
  $('import-note').textContent = !skinSupported
    ? material.vmtPath?.includes('reticles')
      ? 'Reticles are preview-only: PERPHEADS explicitly skips custom reticle materials when loading skins.'
      : 'This material can be previewed and mapped. Installation supports finished weapons and attachments using v_models or Jarheads weapon materials.'
    : importedPng
      ? 'Edited texture is previewed on every visible part sharing this VTF. Each colored UV overlay identifies one part.'
      : 'Import a PNG or DDS with the same dimensions to preview it. You can also export the original texture for editing.';
  const triangles = [];
  for (const layer of layers) {
    if (hiddenUvLayers.has(uvLayerKey(layer))) continue;
    triangles.push(layerLabel(layer));
    let triangleIndex = 0;
    for (const mesh of layer.part.meshes.filter(item => item.materialIndex === layer.index)) {
    for (let i = 0; i < mesh.uvs.length; i += 6) {
      const points = [0, 2, 4].map(offset => {
        const u = mesh.uvs[i + offset], v = mesh.uvs[i + offset + 1];
        const wrappedU = ((u % 1) + 1) % 1, wrappedV = ((v % 1) + 1) % 1;
        return `(${u.toFixed(6)}, ${v.toFixed(6)}) [tile ${(wrappedU * (material.width || 512)).toFixed(1)}, ${(wrappedV * (material.height || 512)).toFixed(1)} px]`;
      });
      triangles.push(`${String(++triangleIndex).padStart(3, '0')}: ${points.join(' | ')}`);
    }
    }
  }
  $('uv-coordinates').textContent = triangles.join('\n') || 'No triangles use this material.';
  const canvas = $('texture-canvas'), width = material.width || 512, height = material.height || 512;
  canvas.width = width; canvas.height = height;
  const context = canvas.getContext('2d'); context.clearRect(0, 0, width, height);
  const img = texturePreviewImage(importedPng || material.image, $('show-alpha').checked);
  if (img?.complete && img.naturalWidth) context.drawImage(img, 0, 0, width, height);
  if ($('show-uv').checked) drawUv(context, width, height);
}

$('show-uv').addEventListener('change', renderTexture);
$('show-alpha').addEventListener('change', renderTexture);
async function savePng(name, dataUrl) {
  try { const saved = await invoke('save_png', {name, pngBase64: dataUrl}); if (saved) toast(`Saved PNG: ${saved}`); }
  catch (error) { toast(String(error), true); }
}
$('export-image').addEventListener('click', async () => {
  const material = activeMaterial(); if (!material) return;
  const image = importedPng || material.image;
  if (!image) { toast('This material has no decoded texture to export.', true); return; }
  const dialog = $('export-dialog');
  dialog.returnValue = 'cancel';
  const choice = await new Promise(resolve => {
    dialog.addEventListener('close', () => resolve(dialog.returnValue), {once: true});
    dialog.showModal();
  });
  if (choice !== 'export') return;
  const format = $('export-format').value;
  try { const saved = await invoke(format === 'dds' ? 'save_dds' : 'save_png', {name: material.name.split('/').pop(), pngBase64: image}); if (saved) toast(`Saved ${format.toUpperCase()}: ${saved}`); }
  catch (error) { toast(String(error), true); }
});
function uvOverlayPng(material) {
  const canvas = document.createElement('canvas'); canvas.width = material.width || 512; canvas.height = material.height || 512;
  drawUv(canvas.getContext('2d'), canvas.width, canvas.height);
  return canvas.toDataURL('image/png');
}
$('download-overlay').addEventListener('click', () => {
  const material = activeMaterial(); if (!material) return;
  savePng(`${material.name.split('/').pop()}_uv_overlay`, uvOverlayPng(material));
});
async function copyImage(png, label, stripAlpha) {
  try { await invoke('copy_png', {pngBase64: png, stripAlpha}); toast(`${label} copied to clipboard.`); }
  catch (error) { toast(String(error), true); }
}
$('copy-png').addEventListener('click', () => {
  const material = activeMaterial(); if (!material) return;
  const png = currentEdit()?.png || material.image;
  if (!png) { toast('This material has no decoded texture to copy.', true); return; }
  copyImage(png, 'Texture image (RGB, no transparency)', true);
});
$('copy-overlay').addEventListener('click', () => {
  const material = activeMaterial(); if (!material) return;
  copyImage(uvOverlayPng(material), 'UV overlay', false);
});
async function importTexture(loadImage) {
  const material = activeMaterial();
  if (!material) { toast('Select a texture before importing an image.', true); return; }
  const partPath = activePartPath, selectedKey = skinKey(material), serial = selectionSerial, loadout = loadoutSerial, layers = activeTextureLayers();
  try {
    const dataUrl = await loadImage();
    const img = new Image(); img.src = dataUrl; await img.decode();
    if (serial !== selectionSerial || loadout !== loadoutSerial) throw new Error('The model changed during import. Import the texture again on the intended model.');
    if (material.width && (img.width !== material.width || img.height !== material.height)) throw new Error(`Image must be ${material.width} × ${material.height} to match this VTF.`);
    for (const layer of layers) {
      const key = skinKey(layer.material);
      edits.set(key, {png: dataUrl, image: img, key, partPath: layer.part.path,
        vmtPath: layer.material.vmtPath, vtfPath: layer.material.vtfPath, vmtText: layer.material.vmtText});
    }
    if (!draftSession) draftSession = {id: null, name: `${weaponData()?.name || attachmentName(model)} draft`, status: ''};
    for (const object of group.children.filter(item =>
      textureKey(assembly.get(item.userData.partPath).materials[item.userData.materialIndex]) === textureKey(material))) {
      object.material.map?.dispose();
      const texture = configureSourceTexture(new THREE.TextureLoader().load(dataUrl));
      object.material.map = texture; object.material.needsUpdate = true;
    }
    if (activePartPath === partPath && skinKey(activeMaterial()) === selectedKey) { importedPng = dataUrl; importedImage = img; }
    renderMaterials();
    await saveDraft();
  } catch (error) { toast(String(error), true); }
}
$('png-input').addEventListener('change', async event => {
  const input = event.target, file = input.files[0]; if (!file) return;
  await importTexture(async () => {
    const raw = await new Promise((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(reader.result); reader.onerror = () => reject(reader.error || new Error('Could not read the image file.')); reader.readAsDataURL(file); });
    return file.name.toLowerCase().endsWith('.dds') ? invoke('import_image', {dataBase64: raw}) : raw;
  });
  input.value = '';
});
$('import-clipboard').addEventListener('click', () => {
  importTexture(() => invoke('import_clipboard_image'));
});
let skinInstallPending = false;
async function runSkinInstall(action) {
  if (skinInstallPending) return;
  skinInstallPending = true;
  try { await action(); }
  catch (error) { if (String(error) !== 'Export cancelled') toast(String(error), true); }
  finally { skinInstallPending = false; }
}
function askInstallAlpha(name, stripAlpha) {
  const dialog = $('alpha-dialog');
  $('alpha-texture-name').textContent = name;
  return new Promise(resolve => {
    dialog.returnValue = 'cancel';
    dialog.addEventListener('close', () => resolve(dialog.returnValue), {once: true});
    dialog.showModal();
    $(stripAlpha ? 'alpha-remove' : 'alpha-keep').focus();
  });
}
async function alphaInstallChoices(textures) {
  const choices = new Map();
  for (const texture of textures) {
    if (choices.has(texture.vtfPath)) continue;
    let strip = texture.stripAlpha || false;
    if (await invoke('texture_has_alpha', {pngBase64: texture.png})) {
      const choice = await askInstallAlpha(texture.vtfPath, strip);
      if (choice === 'cancel') return null;
      strip = choice === 'remove';
    }
    choices.set(texture.vtfPath, strip);
  }
  return choices;
}
function applyAlphaChoices(choices) {
  for (const edit of edits.values()) {
    if (choices.has(edit.vtfPath)) edit.stripAlpha = choices.get(edit.vtfPath);
  }
}
async function installEditedTextures(textures) {
  if (!textures.length) throw new Error('No supported edited textures to install.');
  const choices = await alphaInstallChoices(textures);
  if (!choices) return;
  await saveDraft();
  const count = await invoke('export_textures', {skins: textures.map(texture => ({
    materialPath: texture.vtfPath, vmtPath: texture.vmtPath, vmtText: texture.vmtText,
    pngBase64: texture.png, stripAlpha: choices.get(texture.vtfPath),
  }))});
  applyAlphaChoices(choices);
  await saveDraft();
  installAvailable = true; await refreshSkins(); toast(`${count} custom materials installed.`);
}
$('export-vtf').addEventListener('click', () => runSkinInstall(async () => {
  const skins = new Map();
  for (const {part, material} of activeTextureLayers()) {
    if (supportedMaterial(material, part.path)) skins.set(skinKey(material), {
      ...currentEdit(), vtfPath: material.vtfPath, vmtPath: material.vmtPath, vmtText: material.vmtText,
    });
  }
  await installEditedTextures([...skins.values()]);
}));
$('install-all').addEventListener('click', () => runSkinInstall(async () => {
  const byMaterial = new Map();
  for (const part of assembly.values()) for (const material of part.materials) {
    const edit = textureEdit(material);
    if (edit && supportedMaterial(material, part.path)) byMaterial.set(skinKey(material), {
      ...edit, vtfPath: material.vtfPath, vmtPath: material.vmtPath, vmtText: material.vmtText,
    });
  }
  await installEditedTextures([...byMaterial.values()]);
}));
$('copy-path').addEventListener('click', async () => { if (model) { await navigator.clipboard.writeText(model.origin); toast('Source path copied.'); } });
$('search').addEventListener('input', renderList);
document.querySelectorAll('[data-compatibility]').forEach(button => button.addEventListener('click', () => {
  compatibleOnly = button.dataset.compatibility === 'compatible';
  document.querySelectorAll('[data-compatibility]').forEach(item => {
    const active = item === button;
    item.classList.toggle('active', active);
    item.setAttribute('aria-pressed', String(active));
  });
  renderList();
}));
$('choose-folder').addEventListener('click', async () => { const folder = await invoke('choose_content_folder'); if (folder) scan(folder); });
let scanning = false;
$('refresh-models').addEventListener('click', () => {
  const folder = localStorage.getItem('contentFolder');
  if (folder) scan(folder);
});
async function scan(folder) {
  if (scanning) return;
  scanning = true;
  $('refresh-models').disabled = true; $('choose-folder').disabled = true;
  $('scan-info').textContent = 'Indexing addons…'; $('source-label').textContent = folder;
  try {
    await saveDraft();
    const result = await invoke('scan_content', {root: folder});
    partCache.clear(); thumbnailCache.clear();
    models = result.models; customSkins = result.customSkins; contentScanned = true; installAvailable = result.hasInstallRoot; localStorage.setItem('contentFolder', folder); $('source-label').textContent = result.root;
    drafts = await invoke('list_drafts');
    document.querySelector('.source-dot').classList.add('ready');
    scanSummary = `${result.archiveCount} GMAs indexed · ${result.modelCount.toLocaleString()} PERPHEADS models${result.hasInstallRoot ? '' : ' · choose GMod on export'}`;
    renderList();
  } catch (error) { $('scan-info').textContent = String(error); toast(String(error), true); }
  finally {
    scanning = false; $('choose-folder').disabled = false;
    $('refresh-models').disabled = !localStorage.getItem('contentFolder');
  }
}
const previousFolder = localStorage.getItem('contentFolder'); if (previousFolder) scan(previousFolder);
else refreshSkins().catch(error => toast(`Saved skins could not be loaded: ${error}`, true));
startUpdates({
  invoke, toast, canUpdate: () => !skinInstallPending,
  saveBeforeUpdate: async () => {
    if (draftSession) {
      const name = $('draft-name').value.trim();
      if (!name) throw new Error('Enter a draft name before updating.');
      draftSession.name = name;
    }
    await saveDraft();
  },
});
