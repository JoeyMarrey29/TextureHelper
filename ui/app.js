import * as THREE from './vendor/three.module.js';
import {attachmentData} from './attachment-data.js';
import {startUpdates} from './updates.js';
import {startMcp} from './mcp.js';

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
const componentEdits = new Map();
let componentMode = false, componentSelectionSerial = 0;
let textureSelected = false, overviewKey = '';
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
let textureVariants = [], textureWarnings = [], variantBusy = false;
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
      const materialIndex = detail.materials.indexOf(material);
      const component = draft.componentTextures?.find(texture => texture.partPath === detail.path && texture.materialIndex === materialIndex);
      if (component) return {...material, image:component.png};
      const edit = draft.textures.find(texture => texture.partPath === detail.path && texture.vmtPath === material.vmtPath)
        || draft.textures.find(texture => texture.vtfPath && texture.vtfPath === material.vtfPath);
      return edit ? {...material, image:edit.png,
        vmtText:edit.vmtPath === material.vmtPath ? edit.vmtText ?? material.vmtText : material.vmtText} : material;
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

async function makeThumbnail(detail, {identifyComponent = false, size: imageSize = 80} = {}) {
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
    const materialInfo = identifyComponent ? null : detail.materials[mesh.materialIndex];
    const material = identifyComponent
      ? new THREE.MeshStandardMaterial({color:mesh.componentColor || '#718090', roughness:.8, side:THREE.DoubleSide})
      : createSourceMaterial(materialInfo, 0xb8c2cb);
    if (!identifyComponent && materialInfo.image) {
      try {
        const texture = configureSourceTexture(await new THREE.TextureLoader().loadAsync(materialInfo.image));
        material.color.setHex(0xffffff); material.map = texture;
      } catch { /* Keep the neutral material. */ }
    }
    pivot.add(new THREE.Mesh(geometry, material));
  }
  const center = bounds.getCenter(new THREE.Vector3());
  for (const object of pivot.children) object.position.copy(center).multiplyScalar(-1);
  pivot.quaternion.setFromRotationMatrix(new THREE.Matrix4().makeBasis(
    ...viewBases.front.map(axis => new THREE.Vector3(...axis))));
  pivot.quaternion.premultiply(new THREE.Quaternion().setFromEuler(new THREE.Euler(.1, .16, 0, 'YXZ')));
  const size = Math.max(bounds.getSize(new THREE.Vector3()).length(), .01);
  const half = new THREE.Vector3(), corner = new THREE.Vector3();
  for (const x of [bounds.min.x, bounds.max.x]) for (const y of [bounds.min.y, bounds.max.y]) {
    for (const z of [bounds.min.z, bounds.max.z]) {
      corner.set(x, y, z).sub(center).applyQuaternion(pivot.quaternion);
      half.x = Math.max(half.x, Math.abs(corner.x));
      half.y = Math.max(half.y, Math.abs(corner.y));
    }
  }
  const extent = Math.max(half.x, half.y, size * .025) * 1.14;
  const thumbCamera = new THREE.OrthographicCamera(-extent, extent, extent, -extent, .001, size * 100);
  thumbCamera.position.set(0, 0, size * 3); thumbCamera.lookAt(0, 0, 0);
  thumbnailRenderer.setSize(imageSize, imageSize, false);
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
  requestRender();
}
new ResizeObserver(resize).observe(canvas);
let renderRequest = 0;
function requestRender() {
  if (renderRequest) return;
  renderRequest = requestAnimationFrame(() => {
    renderRequest = 0;
    camera.position.set(0, 0, distance);
    camera.lookAt(0, 0, 0);
    renderer.render(scene, camera);
  });
}
requestRender();
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
  requestRender();
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
function selectTextureLayer(layer, component = false) {
  textureSelected = true;
  componentMode = component;
  componentSelectionSerial++;
  activePartPath = layer.part.path; materialIndex = layer.index;
  highlightLayers(editingTextureLayers());
  renderRelated(); renderMaterials();
  saveDraft().catch(error => toast(`Draft was not saved: ${error}`, true));
}
function pickTexture(event, component = false) {
  const rect = canvas.getBoundingClientRect();
  camera.position.set(0, 0, distance); camera.lookAt(0, 0, 0);
  camera.updateMatrixWorld(); group.updateMatrixWorld(true);
  partRaycaster.setFromCamera(new THREE.Vector2(
    (event.clientX - rect.left) / rect.width * 2 - 1,
    1 - (event.clientY - rect.top) / rect.height * 2), camera);
  const hit = partRaycaster.intersectObjects(group.children, false)[0];
  if (!hit) { showComponentOverview(); return; }
  const part = assembly.get(hit.object.userData.partPath);
  const index = hit.object.userData.materialIndex;
  if (!part?.materials[index]) { toast('The selected mesh has no material to inspect.', true); return; }
  selectTextureLayer({part, material: part.materials[index], index}, component);
}
canvas.addEventListener('contextmenu', event => event.preventDefault());
canvas.addEventListener('dblclick', event => { if (event.button === 0) pickTexture(event, true); });
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
  requestRender();
});
canvas.addEventListener('wheel', event => {
  event.preventDefault();
  distance = Math.max(.05, distance * (event.deltaY > 0 ? 1.12 : .89));
  requestRender();
}, {passive: false});

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
  requestRender();
}

function skinKey(material) { return material?.vmtPath || material?.vtfPath; }
function textureEdit(material) {
  return edits.get(skinKey(material)) || (material?.vtfPath
    ? [...edits.values()].find(edit => edit.vtfPath?.toLowerCase() === material.vtfPath.toLowerCase()) : null);
}
function componentEdit(part, index) { return componentEdits.get(`${part.path}:${index}`); }
function currentEdit() { return (componentMode && activePart() ? componentEdit(activePart(), materialIndex) : null) || textureEdit(activeMaterial()); }
function editingTextureLayers() {
  return componentMode ? activeTextureLayers().filter(layer => layer.part.path === activePartPath && layer.index === materialIndex) : activeTextureLayers();
}
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
  requestRender();
  const bounds = new THREE.Box3();
  for (const detail of assembly.values()) for (const mesh of detail.meshes) {
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute('position', new THREE.Float32BufferAttribute(mesh.positions, 3));
    geometry.setAttribute('normal', new THREE.Float32BufferAttribute(mesh.normals, 3));
    geometry.setAttribute('uv', new THREE.Float32BufferAttribute(mesh.uvs, 2));
    geometry.computeBoundingBox();
    const info = detail.materials[mesh.materialIndex];
    const ownEdit = edits.get(skinKey(info));
    const object = new THREE.Mesh(geometry, makeMaterial({...info, vmtText:ownEdit?.vmtText ?? info.vmtText}));
    object.userData.materialIndex = mesh.materialIndex;
    object.userData.partPath = detail.path;
    object.userData.skinKey = skinKey(info);
    const mount = partMounts.get(detail.path);
    if (mount) { geometry.applyMatrix4(mount.matrix); geometry.computeBoundingBox(); }
    bounds.union(geometry.boundingBox);
    const edit = componentEdit(detail, mesh.materialIndex) || textureEdit(info);
    if (edit) { object.material.map?.dispose(); object.material.map = configureSourceTexture(new THREE.TextureLoader().load(edit.png, requestRender, undefined,
      error => toast(`Could not load the edited model texture: ${error}`, true))); object.material.needsUpdate = true; }
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
    const texture = configureSourceTexture(new THREE.TextureLoader().load(info.image, requestRender, undefined,
      error => toast(`Could not load the model texture: ${error}`, true)));
    material.map = texture;
    material.needsUpdate = true;
  } else material.color.setHex(0x9eaab7);
  return material;
}

async function displayModel(detail, serial, selections = []) {
  highlightedMaterial = null;
  hiddenUvLayers.clear();
  assembly.clear(); edits.clear(); componentEdits.clear(); componentMode = false; textureSelected = false; overviewKey = ''; componentSelectionSerial++; partMounts.clear(); slotSelections.clear(); partCache.clear(); resolvedLoadout = new Map(); assembly.set(detail.path, detail);
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
  $('library-title').textContent = libraryView === 'models' ? 'Models' : libraryView === 'textures' ? 'Textures' : 'Custom skins';
  $('search').placeholder = libraryView === 'models' ? 'Search model paths' : libraryView === 'textures' ? 'Search textures / materials' : 'Search custom skins';
  $('texture-count').textContent = textureVariants.length.toLocaleString();
  $('library-import-texture').classList.toggle('hidden', libraryView !== 'textures');
  $('texture-library-warnings').classList.toggle('hidden', libraryView !== 'textures' || !textureWarnings.length);
  $('skin-count').textContent = (drafts.length + customSkins.filter(skin => !drafts.some(draft => draft.installedIds?.includes(skin.id))).length).toLocaleString();
  $('category-filters').classList.toggle('hidden', libraryView !== 'models');
  $('compatibility-filter').classList.toggle('hidden', libraryView !== 'models');
  if (libraryView === 'skins') { renderCustomSkins(); return; }
  if (libraryView === 'textures') { renderTextureLibrary(); return; }
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

function replacementDimensionsMatch(width, height, material) {
  const validSide = side => Number.isInteger(side) && side > 0 && side <= 4096 && (side & (side - 1)) === 0;
  return validSide(width) && validSide(height) && (!material?.width || !material?.height
    || width * material.height === height * material.width);
}

function activeTextureDimensions() {
  const image = currentEdit()?.image, material = activeMaterial();
  return {width:image?.width || material?.width || 512, height:image?.height || material?.height || 512};
}

function variantMatches(variant, material = activeMaterial()) {
  return !!material && variant.vmtPath === material.vmtPath && variant.vtfPath === material.vtfPath
    && replacementDimensionsMatch(variant.width, variant.height, material);
}

function renderTextureVariants() {
  const material = activeMaterial();
  $('texture-variants').classList.toggle('hidden', !material?.vmtPath || !material?.vtfPath);
  const select = $('variant-select'); select.replaceChildren();
  const original = new Option('Original / default', 'original'); select.append(original);
  const edit = currentEdit(), matches = textureVariants.filter(variant => variantMatches(variant));
  for (const variant of matches) select.append(new Option(variantLabel(variant), variant.id));
  if (edit?.png && edit.png !== material?.image) {
    if (matches.some(variant => variant.id === edit.variantId)) select.value = edit.variantId;
    else { select.append(new Option('Current draft edit', 'current')); select.value = 'current'; }
  } else select.value = 'original';
  select.disabled = variantBusy;
  $('save-texture-variant').disabled = variantBusy || !(edit?.png || material?.image);
  renderComponentScope();
}

function sharedComponentName(part) {
  const weapon = weaponData(part);
  if (weapon) return `Weapon body — ${weapon.name}`;
  const attachment = Object.values(attachmentData.attachments).find(info => info.model === part.path);
  if (attachment) return `${attachmentData.slots[attachment.slot] || 'Attachment'} — ${attachment.label}`;
  return attachmentName(part);
}

async function sharedComponentThumbnail(layers) {
  const selected = new Map(layers.map(layer => [uvLayerKey(layer), uvColors[uvColorIndex(layer) % uvColors.length]]));
  const meshes = [];
  for (const part of assembly.values()) {
    const matrix = partMounts.get(part.path)?.matrix || new THREE.Matrix4();
    for (const mesh of part.meshes) {
      const positions = new THREE.Float32BufferAttribute(mesh.positions, 3);
      const normals = new THREE.Float32BufferAttribute(mesh.normals, 3);
      positions.applyMatrix4(matrix);
      normals.applyNormalMatrix(new THREE.Matrix3().getNormalMatrix(matrix));
      meshes.push({...mesh, positions:positions.array, normals:normals.array,
        componentColor:selected.get(`${part.path}:${mesh.materialIndex}`) || null});
    }
  }
  return makeThumbnail({meshes}, {identifyComponent:true, size:220});
}

let componentDiagramKey = '';
function renderComponentScope() {
  const layers = componentMode ? editingTextureLayers() : activeTextureLayers(), box = $('component-scope');
  box.classList.toggle('hidden', !textureSelected || !layers.length);
  if (!textureSelected || !layers.length) { componentDiagramKey = ''; return; }
  $('component-name').textContent = componentMode
    ? `${sharedComponentName(layers[0].part)} · ${layers[0].material.name}`
    : `Shared texture · ${[...new Set(layers.map(layer => sharedComponentName(layer.part)))].join(' + ')}`;
  $('component-note').textContent = componentMode
    ? 'Editing only the highlighted component. Other components keep their own images; their UV regions combine automatically for install, export and publishing.'
    : 'This texture is shared by the highlighted components. Select a component card to edit only that part, or keep editing the shared texture.';
  const key = `${selectionSerial}:${loadoutSerial}:${uvLayerKey(layers[0])}:${componentMode}:${layers.map(uvLayerKey).join('|')}`;
  if (key !== componentDiagramKey) {
    componentDiagramKey = key;
    $('component-preview').removeAttribute('src');
    sharedComponentThumbnail(layers).then(png => {
      if (componentDiagramKey === key) $('component-preview').src = png;
    }).catch(error => { if (componentDiagramKey === key) toast(`Component preview failed: ${error}`, true); });
  }
}
function showComponentOverview() {
  textureSelected = false; componentMode = false; componentSelectionSerial++;
  highlightedMaterial = null; updateMaterialHighlight();
  renderMaterials();
}
$('show-component-overview').addEventListener('click', showComponentOverview);

function renderComponentOverview() {
  $('component-overview').classList.toggle('hidden', !model || textureSelected);
  $('texture-editor').classList.toggle('hidden', !textureSelected);
  $('inspector-title').textContent = textureSelected ? componentMode ? 'Component texture' : 'Shared texture' : 'Components';
  if (!model || textureSelected) return;
  const selections = textureGroups().flat().map(layer => {
    const assigned = componentEdit(layer.part, layer.index);
    const edit = assigned || textureEdit(layer.material);
    const options = textureVariants.filter(variant => variantMatches(variant, layer.material));
    const selection = !edit || edit.original || edit.png === layer.material.image ? 'original'
      : edit.variantId && options.some(variant => variant.id === edit.variantId) ? edit.variantId : 'current';
    return `${uvLayerKey(layer)}:${selection}`;
  }).join('|');
  const key = `${selectionSerial}:${loadoutSerial}:${textureVariants.map(variant => `${variant.id}:${variant.name}`).join('|')}:${selections}`;
  if (key === overviewKey) return;
  overviewKey = key;
  const sharedContainer = $('shared-texture-cards'); sharedContainer.replaceChildren();
  for (const group of textureGroups()) {
    const layers = group.filter(layer => layer.material.image
      && layer.part.meshes.some(mesh => mesh.materialIndex === layer.index));
    if (new Set(layers.map(layer => layer.part.path)).size < 2) continue;
    const row = document.createElement('section'); row.className = 'shared-texture-card';
    const open = document.createElement('button'); open.type = 'button'; open.className = 'component-card-open';
    open.dataset.textureKey = textureKey(layers[0].material);
    const image = document.createElement('img'); image.alt = 'Components using this shared texture';
    const name = document.createElement('strong');
    name.textContent = (layers[0].material.vtfPath || layers[0].material.name).split('/').pop();
    const owners = document.createElement('small');
    owners.textContent = [...new Set(layers.map(layer => sharedComponentName(layer.part)))].join(' + ');
    const applied = document.createElement('small');
    const edit = textureEdit(layers[0].material);
    const appliedEdit = componentEdit(layers[0].part, layers[0].index) || edit;
    const variant = textureVariants.find(variant => variant.id === edit?.variantId)
      || textureVariants.find(variant => variant.id === appliedEdit?.variantId);
    const sources = new Set(layers.map(layer => (componentEdit(layer.part, layer.index) || textureEdit(layer.material))?.png || layer.material.image));
    applied.textContent = sources.size > 1 ? `Applied: Combined component textures${variant && edit?.variantId === variant.id ? ` · ${variantLabel(variant)}` : ''}`
      : variant ? `Applied: ${variantLabel(variant)}`
      : !appliedEdit || appliedEdit.png === layers[0].material.image ? 'Applied: Original / default' : 'Applied: Current draft texture';
    open.append(image, name, owners, applied);
    open.addEventListener('click', () => selectTextureLayer(layers[0]));
    row.append(open); sharedContainer.append(row);
    sharedComponentThumbnail(layers).then(png => {
      if (row.isConnected) image.src = png;
    }).catch(error => {
      if (row.isConnected) {
        const warning = document.createElement('small'); warning.textContent = `Preview unavailable: ${error}`; row.append(warning);
      }
    });
  }
  $('shared-texture-overview').classList.toggle('hidden', !sharedContainer.childElementCount);
  const container = $('component-cards'); container.replaceChildren();
  for (const layers of textureGroups()) for (const layer of layers) {
    if (!layer.material.image || !layer.part.meshes.some(mesh => mesh.materialIndex === layer.index)) continue;
    const row = document.createElement('section'); row.className = 'component-card';
    const open = document.createElement('button'); open.type = 'button'; open.className = 'component-card-open';
    open.dataset.layerKey = uvLayerKey(layer);
    const image = document.createElement('img'); image.alt = `Edit ${sharedComponentName(layer.part)} · ${layer.material.name}`;
    const name = document.createElement('strong'); name.textContent = sharedComponentName(layer.part);
    name.style.color = uvColors[uvColorIndex(layer)];
    const region = document.createElement('small'); region.textContent = `${layer.material.name}${layers.length > 1 ? ` · shared by ${layers.length} components` : ''}`;
    open.append(image, name, region);
    open.addEventListener('click', () => selectTextureLayer(layer, true));
    const select = document.createElement('select'); select.setAttribute('aria-label', `Skin for ${sharedComponentName(layer.part)} · ${layer.material.name}`);
    select.append(new Option('Current component image', 'current'), new Option('Original / default', 'original'));
    for (const variant of textureVariants.filter(variant => variantMatches(variant, layer.material))) select.append(new Option(variantLabel(variant), variant.id));
    const assigned = componentEdit(layer.part, layer.index), edit = assigned || textureEdit(layer.material);
    const selectedVariant = edit?.variantId && [...select.options].some(option => option.value === edit.variantId)
      ? edit.variantId : null;
    select.value = !edit || edit.original || edit.png === layer.material.image
      ? 'original' : selectedVariant || 'current';
    select.addEventListener('change', async () => {
      if (select.value !== 'current') await applyComponentVariant(select.value, layer);
    });
    row.append(open, select); container.append(row);
    sharedComponentThumbnail([layer]).then(png => {
      if (row.isConnected) image.src = png;
    }).catch(error => {
      if (row.isConnected) { const warning = document.createElement('small'); warning.textContent = `Preview unavailable: ${error}`; row.append(warning); }
    });
  }
}

function variantLabel(variant) {
  const sameName = textureVariants.filter(other => other.name === variant.name
    && other.vmtPath === variant.vmtPath && other.vtfPath === variant.vtfPath);
  if (sameName.length < 2) return variant.name;
  const sameAlpha = sameName.filter(other => other.hasAlpha === variant.hasAlpha).sort((a, b) => a.id.localeCompare(b.id));
  const suffix = sameAlpha.length > 1 ? ` ${sameAlpha.findIndex(other => other.id === variant.id) + 1}` : '';
  return `${variant.name} · ${variant.hasAlpha ? 'Alpha mask' : 'Opaque'}${suffix}`;
}

function renderTextureLibrary() {
  const query = $('search').value.trim().toLowerCase();
  const found = textureVariants.filter(variant => `${variant.name} ${variant.vmtPath} ${variant.vtfPath}`.toLowerCase().includes(query));
  $('model-count').textContent = found.length.toLocaleString();
  $('scan-info').textContent = 'Reusable VMT + VTF pairs. Export changed draft textures from the draft actions; installed skins remain discoverable. Deleting here keeps drafts and installed files.';
  $('texture-warning-summary').textContent = `${textureWarnings.length} textures could not be paired`;
  $('texture-warning-detail').textContent = textureWarnings.join('\n\n');
  const list = $('model-list'); list.replaceChildren();
  if (!found.length) { const empty = document.createElement('p'); empty.className = 'quiet'; empty.textContent = 'No saved textures. Import an image into a draft, then export its changed textures here, or use Save current variant.'; list.append(empty); }
  for (const variant of found) {
    const row = document.createElement('div'); row.className = 'skin-item texture-library-card'; row.dataset.variantId = variant.id;
    const detail = document.createElement('div'); detail.className = 'skin-detail';
    const open = document.createElement('button'); open.type = 'button'; open.className = 'texture-card-open';
    const thumb = document.createElement('span'); thumb.className = 'model-thumb';
    const image = document.createElement('img'); image.src = variant.preview; image.alt = ''; image.loading = 'lazy'; thumb.append(image);
    const name = document.createElement('strong'); name.textContent = variantLabel(variant);
    const info = document.createElement('small'); info.textContent = `${variant.width} × ${variant.height} · VMT + VTF`;
    const path = document.createElement('small'); path.className = 'skin-path'; path.textContent = variant.vmtPath; path.title = variant.vmtPath;
    open.append(thumb, name, info, path); open.addEventListener('click', () => viewTextureVariant(variant.id)); detail.append(open);
    const actions = document.createElement('div'); actions.className = 'skin-tools';
    const apply = document.createElement('button'); apply.type = 'button'; apply.className = 'button secondary'; apply.textContent = 'Use on selected texture';
    apply.disabled = !textureSelected || !variantMatches(variant) || variantBusy; apply.addEventListener('click', () => applyTextureVariant(variant.id));
    const rename = document.createElement('button'); rename.type = 'button'; rename.className = 'button secondary'; rename.textContent = 'Rename';
    rename.addEventListener('click', () => renameTextureVariant(variant));
    const remove = document.createElement('button'); remove.type = 'button'; remove.className = 'button danger'; remove.textContent = 'Delete';
    remove.addEventListener('click', async () => {
      if (!confirm(`Delete saved texture "${variant.name}"?\nDrafts and installed files will be kept.`)) return;
      remove.disabled = true;
      try { await invoke('delete_texture_variant', {id:variant.id}); await refreshTextureVariants(); toast('Texture removed from the library. Drafts and installed files were kept.'); }
      catch (error) { toast(`Texture could not be deleted: ${error}`, true); remove.disabled = false; }
    });
    actions.append(apply, rename, remove); detail.append(actions); row.append(detail); list.append(row);
  }
}

async function refreshTextureVariants() {
  const library = await invoke('list_texture_variants');
  textureVariants = library.variants;
  textureWarnings = library.warnings;
  renderTextureVariants();
  if (libraryView === 'textures') renderList();
}

async function renameTextureVariant(variant) {
  const dialog = $('rename-variant-dialog');
  if (dialog.open) return;
  $('rename-variant-name').value = variant.name;
  dialog.returnValue = '';
  const rename = await new Promise(resolve => {
    dialog.addEventListener('close', () => resolve(dialog.returnValue === 'rename'), {once:true});
    dialog.showModal();
  });
  if (!rename) return;
  try {
    await invoke('rename_texture_variant', {id:variant.id, name:$('rename-variant-name').value.trim()});
    await refreshTextureVariants();
    toast('Texture renamed. Drafts and installed files were kept unchanged.');
  } catch (error) { toast(`Texture could not be renamed: ${error}`, true); }
}

async function viewTextureVariant(id) {
  try {
    const variant = await invoke('load_texture_variant', {id});
    const dialog = $('texture-variant-dialog');
    if (dialog.open) return;
    $('texture-variant-title').textContent = variantLabel(variant);
    $('texture-variant-paths').textContent = `${variant.width} × ${variant.height}\n${variant.vmtPath}\n${variant.vtfPath}`;
    $('texture-variant-preview').src = variant.preview; $('texture-variant-vmt').textContent = variant.vmtText;
    $('apply-texture-variant').disabled = !textureSelected || !variantMatches(variant) || variantBusy;
    dialog.returnValue = '';
    const apply = await new Promise(resolve => {
      dialog.addEventListener('close', () => resolve(dialog.returnValue === 'apply'), {once:true}); dialog.showModal();
    });
    if (apply) await applyTextureVariant(id);
  } catch (error) { toast(`Texture could not be opened: ${error}`, true); }
}

async function applyTextureVariant(id) {
  if (variantBusy) return;
  const material = activeMaterial(), part = activePartPath, serial = selectionSerial, loadout = loadoutSerial, scope = componentSelectionSerial;
  variantBusy = true; renderTextureVariants();
  try {
    const variant = id === 'original' ? null : await invoke('load_texture_variant', {id});
    if (!textureSelected || !material?.image || (variant && !variantMatches(variant, material))) throw new Error('Choose a texture with matching VMT / VTF paths, aspect ratio and supported dimensions first.');
    if (serial !== selectionSerial || loadout !== loadoutSerial || scope !== componentSelectionSerial || part !== activePartPath || skinKey(material) !== skinKey(activeMaterial())) {
      throw new Error('The selected part changed. Choose the variant again for the intended texture.');
    }
    await importTexture(async () => ({png:variant?.png || material.image, vmtText:componentMode ? null : variant?.vmtText || material.vmtText,
      variantId:variant?.id, original:!variant}), {remember:false});
  } catch (error) { toast(`Variant could not be applied: ${error}`, true); }
  finally { variantBusy = false; renderTextureVariants(); if (libraryView === 'textures') renderList(); }
}

$('variant-select').addEventListener('change', event => {
  if (event.target.value !== 'current') applyTextureVariant(event.target.value);
});

async function applyComponentVariant(id, layer) {
  if (variantBusy) return;
  const partPath = layer.part.path, materialIndex = layer.index, material = layer.material;
  const serial = selectionSerial, loadout = loadoutSerial, scope = componentSelectionSerial;
  variantBusy = true;
  try {
    const variant = id === 'original' ? null : await invoke('load_texture_variant', {id});
    if (!material?.image || (variant && !variantMatches(variant, material))) {
      throw new Error('Choose a texture with matching VMT / VTF paths, aspect ratio and supported dimensions first.');
    }
    if (serial !== selectionSerial || loadout !== loadoutSerial || scope !== componentSelectionSerial
      || assembly.get(partPath)?.materials[materialIndex] !== material) {
      throw new Error('The model or component changed. Choose the variant again for the intended part.');
    }
    const png = variant?.png || material.image;
    const image = new Image(); image.src = png; await image.decode();
    const key = skinKey(material);
    if (!edits.has(key)) edits.set(key, {...textureEdit(material), key, partPath,
      vmtPath:material.vmtPath, vtfPath:material.vtfPath, vmtText:material.vmtText,
      png:textureEdit(material)?.png || material.image, image:textureEdit(material)?.image || image});
    const identity = uvLayerKey(layer), current = componentEdits.get(identity) || edits.get(key);
    componentEdits.set(identity, {...current, partPath, materialIndex, vmtPath:material.vmtPath,
      vtfPath:material.vtfPath, png, image, variantId:variant?.id});
    renderAssembly(); renderMaterials();
    await saveDraft();
  } catch (error) { toast(`Component texture could not be changed: ${error}`, true); }
  finally { variantBusy = false; renderTextureVariants(); renderComponentOverview(); }
}
$('library-import-texture').addEventListener('click', () => {
  if (!textureSelected || !activeMaterial()) { toast('Select a model texture before importing a variant.', true); return; }
  $('png-input').click();
});
$('save-texture-variant').addEventListener('click', async () => {
  const material = activeMaterial(), edit = currentEdit(), dialog = $('save-variant-dialog');
  if (!material || variantBusy || dialog.open) return;
  const input = {name:'', vmtPath:material.vmtPath, vtfPath:material.vtfPath,
    vmtText:edits.get(skinKey(material))?.vmtText ?? material.vmtText, png:edit?.png || material.image};
  $('variant-name').value = `${draftSession?.name || attachmentName(activePart())} - ${material.name}`.slice(0, 100);
  dialog.returnValue = '';
  const save = await new Promise(resolve => {
    dialog.addEventListener('close', () => resolve(dialog.returnValue === 'save'), {once:true}); dialog.showModal();
  });
  if (!save) return;
  try {
    input.name = $('variant-name').value.trim();
    const saved = await invoke('save_texture_variant', {texture:input});
    if (edit?.png === input.png && currentEdit() === edit) edit.variantId = saved.id;
    await refreshTextureVariants(); toast('VMT / VTF variant saved to Textures.');
  } catch (error) { toast(`Variant could not be saved: ${error}`, true); }
});

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
  const list = $('model-list');
  const previews = new Map([...list.querySelectorAll('[data-draft-id][data-thumbnail-key]')]
    .map(row => [row.dataset.thumbnailKey, row.querySelector('.draft-thumb')]));
  list.replaceChildren();
  for (const draft of saved) {
    const row = document.createElement(draft.archived ? 'div' : 'button');
    row.className = `skin-item${draftSession?.id === draft.id ? ' active' : ''}`; row.dataset.draftId = draft.id;
    if (!draft.archived) {
      row.type = 'button'; row.setAttribute('aria-label', `Open draft: ${draft.name}`);
      row.addEventListener('click', () => openDraft(draft));
      const cacheKey = `draft:${draft.id}:${draftThumbnailVersions.get(draft.id) || 0}`;
      row.dataset.thumbnailKey = cacheKey;
      const retained = previews.get(cacheKey);
      const preview = retained || document.createElement('span'); preview.className = 'model-thumb draft-thumb';
      if (!retained && thumbnailCache.has(cacheKey)) {
        const image = document.createElement('img'); image.src = thumbnailCache.get(cacheKey); image.alt = '';
        preview.append(image);
      } else if (!retained) preview.textContent = 'Loading preview';
      row.append(preview);
      if (!thumbnailCache.has(cacheKey)) thumbnailObserver.observe(row);
    }
    const detail = document.createElement('div'); detail.className = 'skin-detail';
    const name = document.createElement('strong'); name.textContent = draft.name;
    const status = document.createElement('small'); status.className = `skin-status ${draft.installedCount ? 'installed' : 'saved'}`;
    const installed = !installAvailable ? 'Install status unavailable' : draft.installedCount === 0 ? 'Not installed'
      : draft.installedCount === draft.installableCount ? 'Installed' : `Partly installed (${draft.installedCount}/${draft.installableCount})`;
    status.textContent = `${draft.archived ? 'Saved skin backup' : `Saved draft · ${draft.textureCount} edited textures`} · ${installed}`;
    if (draft.componentError) { status.textContent += ` · Output blocked: ${draft.componentError}`; status.classList.add('error'); }
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
    const edit = document.createElement('button'); edit.type = 'button'; edit.className = 'button secondary';
    edit.textContent = 'Open as editable draft';
    edit.disabled = !contentScanned || !skin.vmtPath || !skin.hasVtf;
    edit.title = !skin.vmtPath || !skin.hasVtf ? 'Both an installed material and texture are required' : 'Find matching models and confirm before creating an editable copy';
    edit.addEventListener('click', async () => {
      edit.disabled = true;
      try { await editInstalledSkin(skin); }
      catch (error) { toast(`Installed skin could not be opened: ${error}`, true); }
      finally { edit.disabled = false; }
    });
    actions.append(edit);
    const uninstall = document.createElement('button'); uninstall.type = 'button'; uninstall.className = 'button secondary'; uninstall.textContent = 'Uninstall';
    uninstall.addEventListener('click', () => archiveInstalledSkin(skin));
    const share = document.createElement('button'); share.type = 'button'; share.className = 'button secondary'; share.textContent = 'Export skin';
    share.addEventListener('click', () => exportSkin('export_custom_skin', skin.id));
    const tools = document.createElement('div'); tools.className = 'skin-tools';
    tools.append(share, remove); actions.append(uninstall); detail.append(actions, tools);
    row.append(detail); list.append(row);
  }
}

async function editInstalledSkin(skin) {
  toast('Finding models that use this installed material…');
  const paths = await invoke('installed_skin_models', {id:skin.id});
  if (!paths.length) throw new Error('No compatible model uses this installed material. Refresh models or export a backup; no files were changed.');
  const dialog = $('installed-draft-dialog'), select = $('installed-draft-model');
  if (dialog.open) throw new Error('Finish the current model selection first.');
  select.replaceChildren();
  for (const path of paths) {
    const option = document.createElement('option'); option.value = path; option.textContent = path; select.append(option);
  }
  dialog.returnValue = '';
  const confirmed = await new Promise(resolve => {
    dialog.addEventListener('close', () => resolve(dialog.returnValue === 'open'), {once:true});
    dialog.showModal();
  });
  if (!confirmed) return;
  await saveDraft();
  const draft = await invoke('edit_installed_skin', {id:skin.id, modelPath:select.value});
  await refreshSkins();
  await openDraft(draft);
}

async function refreshSkins() {
  [customSkins, drafts] = await Promise.all([invoke('list_custom_skins'), invoke('list_drafts')]);
  try { await refreshTextureVariants(); }
  catch (error) { toast(`Texture library could not be refreshed: ${error}`, true); }
  renderTextureVariants();
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
  $('draft-status').textContent = draftSession.componentError ? `Saved locally · output blocked: ${draftSession.componentError}` : draftSession.status || 'Import a texture to start autosaving.';
  $('draft-status').classList.toggle('error', !!draftSession.componentError || draftSession.failed || false);
}

function renderDraftActions() {
  const draft = drafts.find(item => item.id === draftSession?.id && !item.archived);
  $('draft-actions').classList.toggle('hidden', !draft);
  if (!draft) return;
  $('export-draft-textures').title = 'Copy changed draft textures to the library only when you choose.';
  $('install-draft').hidden = draft.installableCount <= draft.installedCount;
  $('install-draft').disabled = !installAvailable;
  $('uninstall-draft').hidden = !draft.installedCount;
}
for (const [id, action] of [
  ['publish-draft', stageWorkshopDraft],
  ['export-draft', draft => exportSkin('export_draft', draft.id)],
  ['export-draft-textures', exportDraftTexturesToLibrary],
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

async function exportDraftTexturesToLibrary(draft) {
  let output;
  try {
    await saveDraft();
    output = await invoke('prepare_draft_output', {id:draftSession?.id || draft.id});
  } catch (error) {
    toast(`Draft textures could not be prepared for the library: ${error}`, true);
    return;
  }
  const draftName = output.name;
  const inputs = [];
  const identity = texture => JSON.stringify([texture.vmtPath.toLowerCase(), texture.vtfPath.toLowerCase(), texture.vmtText, texture.png]);
  const known = new Set(textureVariants.map(identity));
  const add = (name, material, edit) => {
    if (!edit?.png || !material?.vmtPath || !material?.vtfPath || !material?.vmtText) return;
    if (edit.png === material.image) return;
    const texture = {name:name.slice(0, 200), vmtPath:material.vmtPath, vtfPath:material.vtfPath,
      vmtText:edit.vmtText || material.vmtText, png:edit.png};
    const key = identity(texture);
    if (known.has(key)) return;
    known.add(key);
    inputs.push(texture);
  };
  for (const texture of output.textures) {
    const material = [...assembly.values()].flatMap(part => part.materials).find(info => skinKey(info) === texture.key);
    add(`${draftName} - ${material?.name || 'Texture'}`, material, texture);
  }
  if (!inputs.length) { toast('There are no new changed draft textures to add. Existing library variants are already saved.'); return; }
  let saved = 0;
  try {
    for (const texture of inputs) { await invoke('save_texture_variant', {texture}); saved++; }
    await refreshTextureVariants();
    toast(`Saved ${saved} draft texture${saved === 1 ? '' : 's'} to the library. Your draft was kept unchanged.`);
  } catch (error) {
    try { await refreshTextureVariants(); }
    catch (refreshError) { toast(`Saved ${saved} of ${inputs.length} draft textures; library refresh failed: ${refreshError}`, true); return; }
    toast(`Saved ${saved} of ${inputs.length} draft textures before export failed: ${error}`, true);
  }
}

function sameDraftPreview(previous, current) {
  if (!previous || previous.modelPath !== current.modelPath) return false;
  const slots = value => JSON.stringify(Object.entries(value).sort(([a], [b]) => a.localeCompare(b)));
  if (slots(previous.slotSelections) !== slots(current.slotSelections)) return false;
  const sameRecords = (before, after, key, fields) => {
    if (before.length !== after.length) return false;
    const records = new Map(before.map(record => [key(record), record]));
    return after.every(record => {
      const old = records.get(key(record));
      return old && fields.every(field => (old[field] ?? '') === (record[field] ?? ''));
    });
  };
  return sameRecords(previous.textures, current.textures, texture => texture.key, ['png', 'vmtText'])
    && sameRecords(previous.componentTextures || [], current.componentTextures || [],
      texture => `${texture.partPath}:${texture.materialIndex}`, ['png']);
}

function mcpDraftFingerprint() {
  const record = ([key, edit]) => [key, edit.png, edit.vmtText, edit.stripAlpha, edit.variantId];
  return JSON.stringify({
    model: model?.path, selectionSerial, loadoutSerial,
    slots:[...slotSelections], edits:[...edits].map(record), components:[...componentEdits].map(record),
    originalIds:(draftSession?.aiOriginals || []).map(original => original.id),
  });
}

async function snapshotMcpDraft({sendOriginal = false} = {}) {
  if (!model || draftSession?.deletionPromise) throw new Error('Open an editable model or draft first.');
  const selectedModel = model, serial = selectionSerial, loadout = loadoutSerial;
  await saveDraft();
  if (model !== selectedModel || selectionSerial !== serial || loadoutSerial !== loadout) {
    throw new Error('The model or attachments changed while preparing the request. Try again.');
  }
  const fingerprint = mcpDraftFingerprint(), session = draftSession;
  const targets = [], coveredKeys = new Set(), coveredComponents = new Set();
  let skipped = 0;
  for (const group of textureGroups()) {
    const layers = group.filter(layer => layer.part.meshes.some(mesh => mesh.materialIndex === layer.index));
    const editable = layers.filter(layer => supportedMaterial(layer.material, layer.part.path));
    skipped += layers.length - editable.length;
    if (!editable.length) continue;
    for (const layer of editable) coveredKeys.add(skinKey(layer.material));
    if (editable.some(layer => componentEdits.has(uvLayerKey(layer)))) {
      for (const layer of editable) {
        const source = componentEdit(layer.part, layer.index) || textureEdit(layer.material);
        coveredComponents.add(uvLayerKey(layer));
        targets.push({label:layerLabel(layer), png:source?.png || layer.material.image, component:true, layers:[layer], keys:[]});
      }
    } else {
      const source = textureEdit(editable[0].material);
      targets.push({label:editable.map(layerLabel).join(' / '), png:source?.png || editable[0].material.image,
        component:false, layers:editable, keys:editable.map(layer => skinKey(layer.material))});
    }
  }
  // Saved edits can belong to attachments that are not currently mounted.
  for (const [key, edit] of edits) {
    if (coveredKeys.has(key)) continue;
    if (!supportedMaterial(edit, edit.partPath)) { skipped++; continue; }
    const existing = targets.find(target => !target.component && target.png === edit.png
      && (target.layers[0]?.material.vtfPath || edits.get(target.keys[0])?.vtfPath)?.toLowerCase() === edit.vtfPath?.toLowerCase());
    if (existing) existing.keys.push(key);
    else targets.push({label:`Saved texture: ${edit.vmtPath}`, png:edit.png, component:false, layers:[], keys:[key]});
  }
  for (const [key, edit] of componentEdits) {
    if (coveredComponents.has(key)) continue;
    if (!supportedMaterial(edit, edit.partPath)) { skipped++; continue; }
    targets.push({label:`Saved component: ${edit.partPath} / ${edit.materialIndex}`, png:edit.png,
      component:true, layers:[], keys:[], componentKey:key});
  }
  if (!targets.length) throw new Error('This draft has no editable game textures.');
  if (targets.length > 128) throw new Error('MCP previews support at most 128 texture sources per request.');
  let pixels = 0;
  for (const target of targets) {
    if (!target.png) throw new Error(`${target.label} has no decoded image; no textures were sent.`);
    const image = new Image(); image.src = target.png; await image.decode();
    target.width = image.width; target.height = image.height;
    if (!replacementDimensionsMatch(image.width, image.height, {width:image.width, height:image.height})) {
      throw new Error(`${target.label} needs power-of-two dimensions up to 4096 pixels per side.`);
    }
    pixels += image.width * image.height;
    if (pixels > 64 * 1024 * 1024) throw new Error('This draft exceeds the 64-million-pixel MCP preview working limit.');
    if (!sendOriginal) {
      let layers = target.layers;
      if (!layers.length) {
        const sources = target.componentKey ? [componentEdits.get(target.componentKey)] : target.keys.map(key => edits.get(key));
        layers = (await Promise.all(sources.map(async edit => {
          if (!edit?.partPath) throw new Error(`${target.label}: cannot locate UV geometry. No textures were sent.`);
          const part = await loadPart(edit.partPath);
          return part.materials.flatMap((material, index) => {
            const matches = target.component ? index === edit.materialIndex
              : (!!material.vtfPath && material.vtfPath.toLowerCase() === edit.vtfPath?.toLowerCase())
                || (!!material.vmtPath && material.vmtPath.toLowerCase() === edit.vmtPath?.toLowerCase());
            return matches ? [{part, material, index}] : [];
          });
        }))).flat();
      }
      if (!layers.some(layer => layer.part.meshes.some(mesh => mesh.materialIndex === layer.index && mesh.uvs.length >= 6))) {
        throw new Error(`${target.label}: UV geometry is unavailable. No textures were sent.`);
      }
      const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
      const context = canvas.getContext('2d');
      context.fillStyle = '#ffffff'; context.fillRect(0, 0, canvas.width, canvas.height);
      drawUv(context, canvas.width, canvas.height, layers, false);
      target.referencePng = canvas.toDataURL('image/png');
    }
  }
  if (mcpDraftFingerprint() !== fingerprint || draftSession !== session) throw new Error('The draft changed while preparing textures. Try again.');
  return {targets, fingerprint, session, originalCount:session?.aiOriginals?.length || 0,
    note:skipped ? `${skipped} non-editable material slots are excluded.` : 'All editable draft textures are included.'};
}

async function applyMcpDraft(captured, results, failedTargets = []) {
  if (draftSession !== captured.session || mcpDraftFingerprint() !== captured.fingerprint || draftSession?.deletionPromise) {
    throw new Error('The draft or attachments changed after generation. No edits applied; generate a new preview.');
  }
  const accountedTargets = [...results.map(result => result.target), ...failedTargets];
  if (!results.length || accountedTargets.length !== captured.targets.length
      || new Set(accountedTargets).size !== accountedTargets.length
      || accountedTargets.some(target => !captured.targets.includes(target))) {
    throw new Error('The MCP preview is incomplete. No edits applied.');
  }
  const nextEdits = new Map(edits), nextComponents = new Map(componentEdits);
  const originals = results.filter(result => result.originalPng).map(({target, originalPng, originalProvider}) => ({
    id:`ai-${Array.from(crypto.getRandomValues(new Uint8Array(16)), byte => byte.toString(16).padStart(2, '0')).join('')}`,
    label:`${originalProvider} original: ${target.label.slice(0, 200)}`, png:originalPng,
  }));
  if ((draftSession?.aiOriginals?.length || 0) + originals.length > 24) {
    throw new Error('The draft cannot retain all generated originals (limit 24). No edits applied.');
  }
  for (const {target, png, image} of results) {
    for (const layer of target.layers) {
      const key = skinKey(layer.material), base = textureEdit(layer.material);
      if (target.component) {
        if (!nextEdits.has(key)) nextEdits.set(key, {...base, key, png:base?.png || layer.material.image,
          partPath:layer.part.path, vmtPath:layer.material.vmtPath, vtfPath:layer.material.vtfPath,
          vmtText:base?.vmtText ?? layer.material.vmtText});
        nextComponents.set(uvLayerKey(layer), {...base, ...componentEdits.get(uvLayerKey(layer)), png, image,
          partPath:layer.part.path, materialIndex:layer.index, vmtPath:layer.material.vmtPath,
          vtfPath:layer.material.vtfPath, variantId:undefined});
      } else {
        nextEdits.set(key, {...base, key, png, image, partPath:layer.part.path, vmtPath:layer.material.vmtPath,
          vtfPath:layer.material.vtfPath, vmtText:base?.vmtText ?? layer.material.vmtText, variantId:undefined});
      }
    }
    for (const key of target.keys) {
      const existing = nextEdits.get(key);
      if (existing) nextEdits.set(key, {...existing, png, image, variantId:undefined});
    }
    if (target.componentKey) nextComponents.set(target.componentKey, {
      ...nextComponents.get(target.componentKey), png, image, variantId:undefined,
    });
  }
  edits.clear(); for (const [key, edit] of nextEdits) edits.set(key, edit);
  componentEdits.clear(); for (const [key, edit] of nextComponents) componentEdits.set(key, edit);
  if (!draftSession) draftSession = {id:null, name:`${weaponData()?.name || attachmentName(model)} draft`, status:''};
  if (originals.length) draftSession.aiOriginals = [...(draftSession.aiOriginals || []), ...originals];
  importedPng = currentEdit()?.png || null; importedImage = currentEdit()?.image || null;
  overviewKey = '';
  renderAssembly(); renderMaterials();
  try { await saveDraft(); }
  catch (error) { throw new Error(`Edits are applied locally but saving failed: ${error}. Keep the app open and use Save now to retry.`); }
  return draftSession.componentError || null;
}

startMcp({invoke, snapshot:snapshotMcpDraft, apply:applyMcpDraft, toast});

function saveDraft() {
  if (draftSession?.deletionPromise) return draftSession.deletionPromise;
  if (!model || !edits.size || !draftSession) return draftSaveQueue;
  syncSharedTextureEdits();
  for (const layers of textureGroups()) {
    if (!layers.some(layer => componentEdits.has(uvLayerKey(layer)))) continue;
    for (const layer of layers) {
      if (componentEdits.has(uvLayerKey(layer)) || !layer.part.meshes.some(mesh => mesh.materialIndex === layer.index)) continue;
      const base = textureEdit(layer.material);
      componentEdits.set(uvLayerKey(layer), {...base, partPath:layer.part.path, materialIndex:layer.index,
        vtfPath:layer.material.vtfPath, vmtPath:layer.material.vmtPath, png:base?.png || layer.material.image});
    }
  }
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
    componentTextures:[...componentEdits.values()].map(edit => ({
      partPath:edit.partPath, materialIndex:edit.materialIndex, png:edit.png,
    })),
  };
  session.status = 'Saving draft…'; session.failed = false; renderDraftStatus();
  const job = draftSaveQueue.then(async () => {
    const saved = await invoke('save_draft', {draft: {...snapshot, id: session.id}});
    session.id = saved.id;
    session.componentError = saved.componentError;
    if (draftSession === session && snapshot.componentTextures.length && !saved.componentError) {
      for (const texture of saved.textures) {
        const edit = edits.get(texture.key);
        if (edit?.png !== snapshot.textures.find(item => item.key === texture.key)?.png) continue;
        const image = new Image(); image.src = texture.png; await image.decode();
        edits.set(texture.key, {...edit, ...texture, image});
      }
    }
    const previewChanged = !sameDraftPreview(session.thumbnailSnapshot, saved);
    session.thumbnailSnapshot = saved;
    if (previewChanged) {
      const previewVersion = draftThumbnailVersions.get(saved.id) || 0;
      thumbnailCache.delete(`draft:${saved.id}:${previewVersion}`);
      draftThumbnailVersions.set(saved.id, previewVersion + 1);
    }
    session.status = 'Saved locally · safe to close'; session.failed = false;
    drafts = await invoke('list_drafts');
    renderList();
    renderDraftActions();
    if (draftSession === session) renderDraftStatus();
    if (draftSession === session && snapshot.componentTextures.length) renderMaterials();
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
      return {...texture, image, variantId:draft.variantIds?.[texture.key]};
    }));
    const restoredComponents = await Promise.all((draft.componentTextures || []).map(async texture => {
      const image = new Image(); image.src = texture.png; await image.decode();
      return {...texture, image, variantId:draft.componentVariantIds?.[`${texture.partPath}:${texture.materialIndex}`]};
    }));
    const serial = ++selectionSerial;
    const detail = await invoke('open_model', {path: draft.modelPath});
    if (serial !== selectionSerial) return;
    await displayModel(detail, serial, Object.entries(draft.slotSelections));
    if (serial !== selectionSerial) return;
    for (const edit of restored) {
      const material = [...assembly.values()].flatMap(part => part.materials).find(material => skinKey(material) === edit.key);
      if (!replacementDimensionsMatch(edit.image.width, edit.image.height, material)) {
        throw new Error(`Saved texture ${edit.key} has unsupported dimensions or no longer matches the content's aspect ratio.`);
      }
    }
    draftSession = {id: draft.id, name: draft.name, status: 'Saved locally · restored', failed: false,
      aiOriginals:draft.aiOriginals || [], workshopSource:draft.workshopSource, componentError:draft.componentError,
      thumbnailSnapshot:draft};
    for (const edit of restored) edits.set(edit.key, edit);
    for (const edit of restoredComponents) {
      const part = assembly.get(edit.partPath), material = part?.materials[edit.materialIndex];
      if (!replacementDimensionsMatch(edit.image.width, edit.image.height, material)) {
        throw new Error('Saved component dimensions are unsupported or no longer match the original aspect ratio.');
      }
      componentEdits.set(`${edit.partPath}:${edit.materialIndex}`, {...edit, vtfPath:material?.vtfPath, vmtPath:material?.vmtPath});
    }
    activePartPath = assembly.has(draft.activePartPath) ? draft.activePartPath : detail.path;
    const index = activePart().materials.findIndex(material => skinKey(material) === draft.activeMaterialKey);
    materialIndex = index < 0 ? 0 : index;
    selectedPath = detail.path;
    overviewKey = '';
    renderAssembly(); renderMaterials(); renderRelated(); renderList();
    const unavailable = restored.filter(edit => ![...assembly.values()].some(part =>
      part.materials.some(material => skinKey(material) === edit.key)));
    toast(draft.variantWarning || (unavailable.length ? `Draft opened. ${unavailable.length} edited textures belong to parts not currently mounted; those edits remain saved.`
      : 'Draft opened with its attachments and edited textures.'), !!draft.variantWarning);
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
    const saved = await invoke('prepare_draft_output', {id: draft.id});
    const textures = saved.textures.filter(texture => supportedMaterial(texture, texture.partPath));
    const choices = await alphaInstallChoices(textures);
    if (!choices) return;
    const request = {id: draft.id, alphaChoices: Object.fromEntries(choices)};
    const source = await invoke('load_draft', {id:draft.id});
    const mergeRequest = source.componentTextures?.length ? {id:draft.id,
      vtfPaths:[...new Set(textures.map(texture => texture.vtfPath))], componentKeys:null, alphaChoices:request.alphaChoices} : null;
    const confirmation = await confirmSkinInstall(request, mergeRequest);
    if (confirmation === null) return;
    if (confirmation.mergeRequest) {
      const name = await invoke('install_component_merge', {request:confirmation.mergeRequest, overwrite:confirmation.overwrite});
      await refreshSkins(); toast(`Installed "${name}". Source drafts were kept unchanged.`);
    } else {
      await invoke('install_draft', {...request, overwrite:confirmation.overwrite});
      if (draftSession?.id === draft.id) applyAlphaChoices(choices);
      await refreshSkins(); toast('Saved skin installed.');
    }
  });
}

async function deleteSavedDraft(draft) {
  if (!confirm(`Delete "${draft.name}"?\n${installAvailable ? 'The saved copy and all matching installed textures and attachments will be removed. Shared textures still used by other skins and modified files are kept.' : 'Install location is unavailable: only the saved copy can be removed. Installed game files cannot be checked.'}\nUse Uninstall instead to keep a saved copy. This cannot be undone.`)) return;
  const session = draftSession?.id === draft.id ? draftSession : null;
  const deletion = (async () => {
    await draftSaveQueue;
    await invoke('delete_draft', {id: draft.id});
    thumbnailCache.delete(`draft:${draft.id}:${draftThumbnailVersions.get(draft.id) || 0}`);
    draftThumbnailVersions.delete(draft.id);
    if (session && draftSession === session) {
      draftSession = {id: null, name: `${weaponData()?.name || attachmentName(model)} draft`, status: ''};
      edits.clear(); componentEdits.clear(); importedPng = null; importedImage = null;
      overviewKey = '';
      renderAssembly();
      showComponentOverview();
    }
  })();
  if (session) session.deletionPromise = deletion;
  try {
    await deletion;
    await refreshSkins(); toast(installAvailable ? 'Skin deleted, including matching installed files. Shared or modified files were kept.' : 'Saved copy deleted. Install location was unavailable; game files were not checked.');
  } catch (error) { toast(String(error), true); }
  finally { if (session) delete session.deletionPromise; }
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
  syncSharedTextureEdits();
  updateInstallAll();
  renderDraftStatus();
  renderTexture();
  renderComponentOverview();
}
function activePart() { return assembly.get(activePartPath); }
function activeMaterial() { return activePart()?.materials[materialIndex]; }
function renderUvLayers() {
  const container = $('uv-layers'); container.replaceChildren();
  const layers = activeTextureLayers().filter(layer => layer.part.meshes.some(mesh => mesh.materialIndex === layer.index));
  const hidden = componentMode || layers.length <= 1;
  container.classList.toggle('hidden', hidden);
  if (hidden) return;
  layers.forEach(layer => {
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
    const text = document.createElement('span'); text.textContent = `${sharedComponentName(layer.part)} · ${layer.material.name}`;
    label.append(checkbox, swatch, text);
    const select = document.createElement('button'); select.type = 'button'; select.textContent = 'Highlight';
    select.title = `Highlight ${layerLabel(layer)} in the 3D view`;
    select.addEventListener('click', () => {
      textureSelected = true; componentSelectionSerial++;
      activePartPath = layer.part.path; materialIndex = layer.index; highlightLayers([layer]);
      renderRelated(); renderMaterials();
      saveDraft().catch(error => toast(`Draft was not saved: ${error}`, true));
    });
    const edit = document.createElement('button'); edit.type = 'button'; edit.textContent = 'Edit component';
    edit.addEventListener('click', () => selectTextureLayer(layer, true));
    row.append(label, select, edit); container.append(row);
  });
}
function drawUv(context, width, height, layers = editingTextureLayers(), respectHidden = true) {
  context.save();
  context.lineWidth = Math.max(1, width / 512); context.lineJoin = 'round';
  context.beginPath(); context.rect(0, 0, width, height); context.clip();
  layers.forEach((layer, index) => {
    if (respectHidden && hiddenUvLayers.has(uvLayerKey(layer))) return;
    const color = uvColors[respectHidden ? uvColorIndex(layer) : index % uvColors.length];
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
  renderTextureVariants();
  if (!material) { tools.classList.add('hidden'); return; }
  importedPng = currentEdit()?.png || null;
  importedImage = currentEdit()?.image || null;
  tools.classList.remove('hidden');
  const layers = editingTextureLayers();
  $('material-name').textContent = layers.length > 1 ? (material.vtfPath || material.name).split('/').pop() : material.name;
  renderUvLayers();
  const {width, height} = activeTextureDimensions();
  $('texture-size').textContent = material.width ? `${width} × ${height}` : 'No VTF';
  $('vmt-path').textContent = [...new Set(layers.map(layer => layer.material.vmtPath || 'Not found'))].join('\n');
  $('vmt-origin').textContent = [...new Set(layers.map(layer => layer.material.vmtOrigin || 'Not found'))].join('\n');
  $('vtf-path').textContent = material.vtfPath || 'Not found';
  $('vtf-origin').textContent = material.vtfOrigin || 'Not found';
  $('vmt-text').textContent = [...new Map(layers.map(layer =>
    [skinKey(layer.material) || uvLayerKey(layer), layer.material])).values()]
    .map(info => `${info.vmtPath || info.name}\n${edits.get(skinKey(info))?.vmtText ?? info.vmtText ?? 'VMT not found'}`).join('\n\n');
  const skinSupported = supportedMaterial(material);
  $('export-vtf').disabled = !importedPng || !skinSupported;
  $('copy-png').disabled = !(importedPng || material.image);
  $('import-note').textContent = componentMode
    ? 'Imports and variants affect only this component. Other components stay unchanged. Their UV regions combine automatically for install, export and publishing; incompatible overlaps block output. VMT shader settings stay shared.'
    : !skinSupported
    ? material.vmtPath?.includes('reticles')
      ? 'Reticles are preview-only: PERPHEADS explicitly skips custom reticle materials when loading skins.'
      : 'This material can be previewed and mapped. Installation supports finished weapons and attachments using v_models or Jarheads weapon materials.'
    : importedPng
      ? 'Shared atlas preview. Component edits keep their own images in 3D. Importing here replaces this shared texture and clears its component overrides.'
      : 'Import a PNG, DDS or VTF with the original aspect ratio and power-of-two dimensions up to 4096 × 4096, optionally selecting its VMT + VTF together. Resolution can differ from the original. Changes are saved only in the draft.';
  renderUvCoordinates();
  const canvas = $('texture-canvas');
  canvas.width = width; canvas.height = height;
  const context = canvas.getContext('2d'); context.clearRect(0, 0, width, height);
  const img = texturePreviewImage(importedPng || material.image, $('show-alpha').checked);
  if (img?.complete && img.naturalWidth) context.drawImage(img, 0, 0, width, height);
  if ($('show-uv').checked) drawUv(context, width, height);
}

function renderUvCoordinates() {
  const output = $('uv-coordinates');
  if (!$('uv-coordinate-details').open) { output.textContent = ''; return; }
  const material = activeMaterial();
  if (!material) { output.textContent = ''; return; }
  const {width, height} = activeTextureDimensions();
  const triangles = [], layers = editingTextureLayers();
  for (const layer of layers) {
    if (hiddenUvLayers.has(uvLayerKey(layer))) continue;
    triangles.push(layerLabel(layer));
    let triangleIndex = 0;
    for (const mesh of layer.part.meshes.filter(item => item.materialIndex === layer.index)) {
    for (let i = 0; i < mesh.uvs.length; i += 6) {
      const points = [0, 2, 4].map(offset => {
        const u = mesh.uvs[i + offset], v = mesh.uvs[i + offset + 1];
        const wrappedU = ((u % 1) + 1) % 1, wrappedV = ((v % 1) + 1) % 1;
        return `(${u.toFixed(6)}, ${v.toFixed(6)}) [tile ${(wrappedU * width).toFixed(1)}, ${(wrappedV * height).toFixed(1)} px]`;
      });
      triangles.push(`${String(++triangleIndex).padStart(3, '0')}: ${points.join(' | ')}`);
    }
    }
  }
  output.textContent = triangles.join('\n') || 'No triangles use this material.';
}

$('uv-coordinate-details').addEventListener('toggle', renderUvCoordinates);
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
  try {
    let png = image;
    if (componentEdits.size) {
      await saveDraft();
      const output = await invoke('prepare_draft_output', {id:draftSession.id});
      const texture = output.textures.find(texture => texture.vtfPath === material.vtfPath);
      if (!texture) throw new Error('The selected shared texture is missing from this draft output.');
      png = texture.png;
    }
    const saved = await invoke(format === 'dds' ? 'save_dds' : 'save_png', {name: material.name.split('/').pop(), pngBase64:png});
    if (saved) toast(`Saved ${format.toUpperCase()}: ${saved}`);
  }
  catch (error) { toast(String(error), true); }
});
function uvOverlayPng(material) {
  const {width, height} = activeTextureDimensions();
  const canvas = document.createElement('canvas'); canvas.width = width; canvas.height = height;
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
  const partPath = activePartPath, selectedKey = skinKey(material), serial = selectionSerial, loadout = loadoutSerial,
    scope = componentSelectionSerial, component = componentMode, layers = editingTextureLayers();
  try {
    const loaded = await loadImage();
    let dataUrl = typeof loaded === 'string' ? loaded : loaded.png;
    const vmtText = typeof loaded === 'string' ? null : loaded.vmtText;
    const img = new Image(); img.src = dataUrl; await img.decode();
    if (serial !== selectionSerial || loadout !== loadoutSerial || scope !== componentSelectionSerial) throw new Error('The model or component changed during import. Import the texture again on the intended model.');
    if (!replacementDimensionsMatch(img.width, img.height, material)) {
      if (!replacementDimensionsMatch(material.width, material.height, material)) {
        throw new Error('This material has no supported target size. Use power-of-two dimensions up to 4096 × 4096.');
      }
      const dialog = $('resize-image-dialog');
      if (dialog.open) throw new Error('Finish the current resize confirmation before importing another image.');
      $('resize-image-message').textContent = `This image is ${img.width} × ${img.height}. Textures must keep the original aspect ratio and use power-of-two dimensions up to 4096 × 4096. Resize to ${material.width} × ${material.height}?`;
      dialog.returnValue = 'cancel';
      const resize = await new Promise(resolve => {
        dialog.addEventListener('close', () => resolve(dialog.returnValue === 'resize'), {once:true});
        dialog.showModal();
      });
      if (!resize) { toast('Image import cancelled. The texture was not changed.'); return false; }
      const resized = document.createElement('canvas');
      resized.width = material.width; resized.height = material.height;
      const context = resized.getContext('2d');
      context.imageSmoothingEnabled = true; context.imageSmoothingQuality = 'high';
      context.drawImage(img, 0, 0, resized.width, resized.height);
      dataUrl = resized.toDataURL('image/png');
      img.src = dataUrl; await img.decode();
    }
    if (serial !== selectionSerial || loadout !== loadoutSerial || scope !== componentSelectionSerial) throw new Error('The model or component changed during import. The texture was not applied.');
    if (component) {
      const prepared = await Promise.all(activeTextureLayers()
        .filter(layer => layer.part.meshes.some(mesh => mesh.materialIndex === layer.index)).map(async layer => {
        const key = skinKey(layer.material), base = textureEdit(layer.material);
        const png = base?.png || layer.material.image;
        if (!png) throw new Error(`${sharedComponentName(layer.part)} has no decoded image to preserve.`);
        const image = new Image(); image.src = png; await image.decode();
        return {layer, key, base:{...base, key, png, image, partPath:layer.part.path,
          vtfPath:layer.material.vtfPath, vmtPath:layer.material.vmtPath, vmtText:base?.vmtText ?? layer.material.vmtText}};
      }));
      if (scope !== componentSelectionSerial || serial !== selectionSerial || loadout !== loadoutSerial) throw new Error('Component changed while preparing the edit. Import again.');
      for (const {layer, key, base} of prepared) {
        if (!edits.has(key)) edits.set(key, base);
        if (!componentEdits.has(uvLayerKey(layer))) componentEdits.set(uvLayerKey(layer), {
          ...edits.get(key), partPath:layer.part.path, materialIndex:layer.index,
        });
      }
      const layer = layers[0];
      componentEdits.set(uvLayerKey(layer), {...componentEdits.get(uvLayerKey(layer)), png:dataUrl, image:img,
        variantId:typeof loaded !== 'string' ? loaded.variantId : undefined});
    } else {
      for (const [key, edit] of componentEdits) if (edit.vtfPath === material.vtfPath) componentEdits.delete(key);
      for (const layer of layers) {
      const key = skinKey(layer.material);
      edits.set(key, {png: dataUrl, image: img, key, partPath: layer.part.path,
        vmtPath: layer.material.vmtPath, vtfPath: layer.material.vtfPath,
        vmtText: typeof loaded !== 'string' && loaded.original ? layer.material.vmtText
          : key === selectedKey && vmtText ? vmtText : edits.get(key)?.vmtText ?? layer.material.vmtText,
        variantId: key === selectedKey && typeof loaded !== 'string' ? loaded.variantId : undefined});
      }
    }
    if (!draftSession) draftSession = {id: null, name: `${weaponData()?.name || attachmentName(model)} draft`, status: ''};
    renderAssembly();
    if (activePartPath === partPath && skinKey(activeMaterial()) === selectedKey) { importedPng = dataUrl; importedImage = img; }
    overviewKey = '';
    renderMaterials();
    await saveDraft();
    return true;
  } catch (error) { toast(String(error), true); return false; }
}
$('png-input').addEventListener('change', async event => {
  const input = event.target, files = [...input.files]; if (!files.length) return;
  await importTexture(async () => {
    const materials = files.filter(file => /\.vmt$/i.test(file.name)), images = files.filter(file => /\.(png|dds|vtf)$/i.test(file.name));
    if (materials.length > 1 || images.length !== 1 || files.length !== materials.length + images.length) {
      throw new Error('Select one PNG, DDS or VTF, optionally with its VMT. To import a VMT, select its matching VTF as well.');
    }
    const file = images[0];
    if (files.some(file => file.size > 96 * 1024 * 1024)) throw new Error('Texture files must be at most 96 MiB.');
    const vmtText = materials.length ? await materials[0].text() : null;
    if (vmtText) {
      if (materials[0].size > 128 * 1024) throw new Error('VMT files must be at most 128 KiB.');
      const reference = vmtValue(vmtText, 'basetexture')?.replaceAll('\\', '/').split('/').pop()?.replace(/\.vtf$/i, '').toLowerCase();
      if (!/\.vtf$/i.test(file.name) || reference !== file.name.replace(/\.vtf$/i, '').toLowerCase()) {
        throw new Error('Select the VTF named by the VMT $basetexture so the pair can be verified.');
      }
    }
    const raw = await new Promise((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(reader.result); reader.onerror = () => reject(reader.error || new Error('Could not read the image file.')); reader.readAsDataURL(file); });
    const png = /\.(dds|vtf)$/i.test(file.name) ? await invoke('import_image', {dataBase64: raw}) : raw;
    return {png, vmtText};
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
async function confirmSkinInstall(request, mergeRequest = null, merged = false) {
  toast('Checking which models will change...');
  const {conflicts, affectedModels, warnings} = merged ? await invoke('preview_component_merge', {request:mergeRequest})
    : await invoke('preview_skin_install', request);
  if (!conflicts.length && !affectedModels.length && !warnings.length) return {overwrite:{}, mergeRequest:merged ? mergeRequest : null};
  const dialog = $('overwrite-skin-dialog'), list = $('overwrite-skin-files');
  if (dialog.open) throw new Error('Finish the current skin replacement confirmation first.');
  $('overwrite-skin-title').textContent = merged ? 'Review merged component texture'
    : conflicts.length ? 'Replace installed skin?' : 'Review skin installation';
  $('skin-overwrite-warning').hidden = !conflicts.length;
  $('merge-component-install').hidden = !mergeRequest || merged;
  $('confirm-skin-overwrite').textContent = merged ? 'Merge and install' : conflicts.length ? 'Replace entire texture' : 'Install';
  $('install-affected-title').textContent = `Models that will change (${affectedModels.length})`;
  const affected = $('install-affected-models');
  affected.replaceChildren();
  if (affectedModels.length) {
    const models = document.createElement('ul');
    for (const path of affectedModels) {
      const item = document.createElement('li');
      item.textContent = sharedComponentName({path}); item.title = path; models.append(item);
    }
    affected.append(models);
  } else {
    affected.textContent = warnings.length ? 'No model changes could be confirmed. See the warnings below.'
      : 'No indexed weapon or attachment changes its UV-covered pixels or material settings.';
  }
  const warningList = $('install-impact-warnings');
  warningList.replaceChildren();
  for (const warning of warnings) {
    const message = document.createElement('p');
    message.className = 'note'; message.textContent = `Warning: ${warning}`; warningList.append(message);
  }
  $('merge-install-note').textContent = merged
    ? 'The right side now shows the merged result. Installed pixels outside the selected component UVs and existing VMT settings are preserved. Conflicting UV overlaps block merging. The result is saved as a separate installed skin for status, export and uninstall; source drafts stay unchanged.'
    : 'Replace installs the entire new atlas. Merge component textures, when available, preserves other installed UV regions and material settings, then shows the merged result for confirmation. A merged result becomes a separate saved skin; source drafts stay unchanged. Flattened textures and backups cannot identify individual components.';
  list.replaceChildren();
  for (const conflict of conflicts) {
    const card = document.createElement('section'), path = document.createElement('p');
    card.className = 'install-conflict';
    path.className = 'install-conflict-path'; path.textContent = conflict.path; card.append(path);
    const comparison = document.createElement('div'); comparison.className = 'install-comparison';
    for (const [label, png, text] of [
      ['Installed — will be replaced', conflict.installedPng, conflict.installedText],
      [merged ? 'Merged — will be installed' : 'New — will be installed', conflict.replacementPng, conflict.replacementText],
    ]) {
      const side = document.createElement('div'), title = document.createElement('h3');
      title.textContent = label; side.append(title);
      if (png) {
        const image = document.createElement('img'); image.src = png; image.alt = label; side.append(image);
      } else {
        const content = document.createElement(text ? 'pre' : 'p');
        content.textContent = text || 'Preview unavailable. This file will still be replaced if you proceed.';
        side.append(content);
      }
      comparison.append(side);
    }
    card.append(comparison);
    if (conflict.warning) {
      const warning = document.createElement('p'); warning.className = 'note'; warning.textContent = conflict.warning; card.append(warning);
    }
    list.append(card);
  }
  dialog.returnValue = 'cancel';
  const choice = await new Promise(resolve => {
    dialog.addEventListener('close', () => resolve(dialog.returnValue), {once:true});
    dialog.showModal(); $('cancel-skin-overwrite').focus();
  });
  list.replaceChildren();
  affected.replaceChildren();
  warningList.replaceChildren();
  if (choice === 'merge' && mergeRequest && !merged) return confirmSkinInstall(request, mergeRequest, true);
  return choice === 'overwrite' ? {overwrite:Object.fromEntries(conflicts.map(conflict => [conflict.path, conflict.fingerprint])),
    mergeRequest:merged ? mergeRequest : null} : null;
}
async function installEditedTextures(textures, componentOnly = false) {
  if (!textures.length) throw new Error('No supported edited textures to install.');
  if (componentEdits.size) {
    const paths = new Set(textures.map(texture => texture.vtfPath));
    await saveDraft();
    const output = await invoke('prepare_draft_output', {id:draftSession.id});
    textures = output.textures.filter(texture => paths.has(texture.vtfPath) && supportedMaterial(texture, texture.partPath));
  }
  const choices = await alphaInstallChoices(textures);
  if (!choices) return;
  await saveDraft();
  const request = {skins: textures.map(texture => ({
    materialPath: texture.vtfPath, vmtPath: texture.vmtPath, vmtText: texture.vmtText,
    pngBase64: texture.png, stripAlpha: choices.get(texture.vtfPath),
  }))};
  const vtfPaths = [...new Set(textures.map(texture => texture.vtfPath))];
  const mergeRequest = componentEdits.size ? {id:draftSession.id, vtfPaths,
    componentKeys:componentOnly && componentMode ? [`${activePartPath}:${materialIndex}`] : null,
    alphaChoices:Object.fromEntries(choices)} : null;
  const confirmation = await confirmSkinInstall(request, mergeRequest);
  if (confirmation === null) return;
  if (confirmation.mergeRequest) {
    const name = await invoke('install_component_merge', {request:confirmation.mergeRequest, overwrite:confirmation.overwrite});
    installAvailable = true; await refreshSkins(); toast(`Installed "${name}". Source drafts were kept unchanged.`);
    return;
  }
  const count = await invoke('export_textures', {...request, overwrite:confirmation.overwrite});
  applyAlphaChoices(choices);
  await saveDraft();
  installAvailable = true; await refreshSkins(); toast(`${count} custom materials installed.`);
}
$('export-vtf').addEventListener('click', () => runSkinInstall(async () => {
  const skins = new Map();
  for (const {part, material} of activeTextureLayers()) {
    if (supportedMaterial(material, part.path)) skins.set(skinKey(material), {
      ...currentEdit(), vtfPath: material.vtfPath, vmtPath: material.vmtPath, vmtText: edits.get(skinKey(material))?.vmtText ?? material.vmtText,
    });
  }
  await installEditedTextures([...skins.values()], true);
}));
$('install-all').addEventListener('click', () => runSkinInstall(async () => {
  const byMaterial = new Map();
  for (const part of assembly.values()) for (const material of part.materials) {
    const edit = textureEdit(material);
    if (edit && supportedMaterial(material, part.path)) byMaterial.set(skinKey(material), {
      ...edit, vtfPath: material.vtfPath, vmtPath: material.vmtPath, vmtText: edits.get(skinKey(material))?.vmtText ?? material.vmtText,
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
    try { await refreshTextureVariants(); }
    catch (error) { toast(`Texture library could not be refreshed: ${error}`, true); }
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
