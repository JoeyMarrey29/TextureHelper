"""Headless preview regressions. Requires Playwright and installed Microsoft Edge.

Run: python tools\\test_preview.py
Optionally pass --fixtures <folder> with open_model JSON responses for visual checks.
"""

import argparse
import base64
import json
import mimetypes
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import sync_playwright


ROOT = Path(__file__).resolve().parents[1]
ORIGIN = "http://preview.test/"
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--fixtures", type=Path)
parser.add_argument("--screenshot", type=Path)
args = parser.parse_args()

data_text = (ROOT / "ui" / "attachment-data.js").read_text(encoding="utf-8")
data = json.loads(data_text[data_text.index("{"):data_text.rindex("}") + 1])
identity = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]


def model_detail(path, sequence=None):
    if args.fixtures:
        suffix = "" if sequence is None else f"__seq_{sequence}"
        fixture = args.fixtures / (path.replace("/", "__") + suffix + ".json")
        return json.loads(fixture.read_text(encoding="utf-8"))
    return {
        "path": path, "origin": "test fixture", "triangleCount": 1,
        "bones": [{"name": "root", "transform": identity}],
        "attachments": [], "relatedModels": [],
        "sequence": "idle_down" if sequence == 1 else "idle",
        "materials": [{
            "name": path, "vmtPath": "materials/" + path + ".vmt",
            "vmtText": 'VertexLitGeneric\n{\n}', "image": None,
        }],
        "meshes": [{
            "materialIndex": 0, "positions": [-1, -1, 0, 1, -1, 0, 0, 1, 0],
            "normals": [0, 0, 1] * 3, "uvs": [0, 0, 1, 0, 0.5, 1],
        }],
    }


HOOKS = """
window.previewTest = {
  THREE, createSourceMaterial, makeMaterial, makeThumbnail, resolveLoadout, slotChoices,
  supportedMaterial,
  setSlotAttachment, setView, modelCategory, renderList, openDraft, saveDraft, stageWorkshopDraft,
  applyTextureVariant, applyComponentVariant, refreshSkins, restoreSavedSkin,
  previewRenderCount:()=>renderer.info.render.frame,
  holdTextureRedraws() {
    const original=THREE.TextureLoader.prototype.load;
    const callbacks=[];
    THREE.TextureLoader.prototype.load=function(url,onLoad,...rest) {
      return original.call(this,url,texture=>callbacks.push(()=>onLoad?.(texture)),...rest);
    };
    window.releaseTextureRedraws=()=>{
      THREE.TextureLoader.prototype.load=original;
      callbacks.forEach(callback=>callback());
    };
    window.pendingTextureRedraws=()=>callbacks.length;
  },
  sameDraftPreview,
  draftPreviewVersion:()=>draftThumbnailVersions.get(draftSession?.id),
  setupSharedComposition() {
    const gun=assembly.get(model.path), stock=assembly.get('models/perpheads/weapons/attachments/stock_ak101.mdl');
    const png=this.compositionPng('#666666','#666666');
    const material={name:'Shared wood',vmtPath:'materials/models/weapons/v_models/test/shared.vmt',
      vtfPath:'materials/models/weapons/v_models/test/shared.vtf',width:16,height:8,image:png,
      vmtText:'VertexLitGeneric\\n{\\n$basetexture "models/weapons/v_models/test/shared"\\n$phong 1\\n}'};
    gun.materials[0]={...material};stock.materials[0]={...material};
    window.draftMock.componentFixture={paths:[gun.path,stock.path],material};
    activePartPath=gun.path;materialIndex=0;
    renderAssembly();renderMaterials();
    return {gun:gun.path,stock:stock.path,material};
  },
  compositionPng(left,right) {
    const canvas=document.createElement('canvas');canvas.width=16;canvas.height=8;
    const context=canvas.getContext('2d');context.fillStyle=left;context.fillRect(0,0,8,8);
    context.fillStyle=right;context.fillRect(8,0,8,8);return canvas.toDataURL();
  },
  variantPng(color, width = 4, height = width) {
    const canvas = document.createElement('canvas'); canvas.width = width; canvas.height = height;
    const context = canvas.getContext('2d'); context.fillStyle = color; context.fillRect(0,0,width,height);
    return canvas.toDataURL();
  },
  setupVariantFixture() {
    for (const part of assembly.values()) for (const [index, material] of part.materials.entries()) {
      const path = 'materials/models/weapons/v_models/variants/' + part.path.split('/').pop().replace('.mdl','') + index;
      Object.assign(material,{vmtPath:path+'.vmt',vtfPath:path+'.vtf',width:4,height:4,
        vmtText:'VertexLitGeneric\\n{\\n$basetexture "'+path.replace('materials/','')+'"\\n}',
        image:this.variantPng('#666666')});
    }
    renderAssembly(); renderMaterials();
  },
  setupResolutionFixture() {
    this.setupVariantFixture();this.setupMaskedTexture(512);
    window.draftMock.componentFixture={paths:[model.path],material:{...activeMaterial()}};
  },
  variantEdits() { return [...edits.values()].map(({key,png,vmtText}) => ({key,png,vmtText})); },
  draftState() { return {id: draftSession?.id, name: draftSession?.name, status: draftSession?.status,
    selections: Object.fromEntries(slotSelections), edits: [...edits].map(([key, edit]) => ({key, png: edit.png, partPath: edit.partPath}))}; },
  async selectPart(path) {
    activePartPath = path; materialIndex = 0; textureSelected=true;componentMode=false;componentSelectionSerial++;renderMaterials();
  },
  selectTexture(key) {
    const layer = textureGroups().find(layers => textureKey(layers[0].material) === key)?.[0];
    if (!layer) throw Error('Texture not found: ' + key);
    selectTextureLayer(layer);
  },
  selectActiveShared() {
    selectTextureLayer({part:activePart(), material:activeMaterial(), index:materialIndex});
  },
  rotation() { return group.quaternion.toArray(); },
  surfacePoint(path) {
    camera.position.set(0, 0, distance); camera.lookAt(0,0,0);
    camera.updateMatrixWorld(); group.updateMatrixWorld(true);
    const rect = canvas.getBoundingClientRect();
    for (const object of group.children.filter(object => object.userData.partPath === path)) {
      const positions = object.geometry.attributes.position;
      for (let i = 0; i < positions.count; i += 3) {
        const point = new THREE.Vector3();
        for (let j = 0; j < 3; j++) point.add(new THREE.Vector3().fromBufferAttribute(positions, i + j));
        point.divideScalar(3).applyMatrix4(object.matrixWorld).project(camera);
        if (Math.abs(point.x) > .95 || Math.abs(point.y) > .95 || Math.abs(point.z) > 1) continue;
        partRaycaster.setFromCamera(new THREE.Vector2(point.x, point.y), camera);
        const hit = partRaycaster.intersectObjects(group.children, false)[0];
        if (hit?.object === object) return {
          x: rect.left + (point.x + 1) * rect.width / 2, y: rect.top + (1 - point.y) * rect.height / 2,
          index: object.userData.materialIndex
        };
      }
    }
    throw Error('No visible surface found for ' + path);
  },
  setupPicking() {
    group.quaternion.identity(); distance = 10;
    group.children.forEach(object => {
      object.geometry.dispose(); object.geometry = new THREE.PlaneGeometry(1, 1);
      const x = object.userData.partPath === model.path ? -1
        : object.userData.partPath.includes('stock_ak101') ? 1 : 0;
      object.position.set(x, 0, x === -1 ? 0 : -1);
    });
    group.updateMatrixWorld(true);
  },
  overlapPicking() {
    group.children.forEach(object => object.position.set(0, 0,
      object.userData.partPath === model.path ? 1 : 0));
  },
  meshPoint(path) {
    camera.position.set(0, 0, distance); camera.lookAt(0,0,0);
    camera.updateMatrixWorld(); group.updateMatrixWorld(true);
    const object = group.children.find(object => object.userData.partPath === path);
    const point = object.getWorldPosition(new THREE.Vector3()).project(camera);
    const rect = canvas.getBoundingClientRect();
    return {x: rect.left + (point.x + 1) * rect.width / 2, y: rect.top + (1 - point.y) * rect.height / 2};
  },
  uploadPng() {
    const material = activeMaterial(), canvas = document.createElement('canvas');
    canvas.width = material.width || 4; canvas.height = material.height || 4;
    const context = canvas.getContext('2d'); context.fillStyle = '#e12a43';
    context.fillRect(0, 0, canvas.width, canvas.height); return canvas.toDataURL();
  },
  uploadAlphaPng() {
    const material = activeMaterial(), canvas = document.createElement('canvas');
    canvas.width = material.width || 4; canvas.height = material.height || 4;
    const context = canvas.getContext('2d'); context.fillStyle = 'rgba(200,100,50,0.125)';
    context.fillRect(0, 0, canvas.width, canvas.height); return canvas.toDataURL();
  },
  setupJarheads() {
    const material = activeMaterial();
    material.vmtPath = 'materials/models/jarheads/weapons/attachments/rifle_sights/flip/sights.vmt';
    material.vtfPath = 'materials/models/jarheads/weapons/attachments/rifle_sights/flip/sights.vtf';
    material.vmtText = 'VertexLitGeneric\\n{\\n$basetexture "models/jarheads/weapons/attachments/rifle_sights/flip/sights"\\n}';
    renderMaterials();
  },
  async showSkins() {
    libraryView = 'skins'; await refreshSkins();
  },
  enableInstall() { installAvailable = true; renderList(); },
  draftThumbnailModel,
  async open(path) {
    models = Object.keys(attachmentData.weapons).concat(
      Object.values(attachmentData.attachments).map(info => info.model)
    ).map(path => ({path, compatible: true}));
    contentScanned = true;
    await selectModel({path});
    textureSelected=true;renderMaterials();
  },
  showComponentOverview,
  componentState:()=>({mode:componentMode,selected:textureSelected,edits:[...componentEdits.values()].map(({partPath,materialIndex,png})=>({partPath,materialIndex,png}))}),
  editComponent(path,index=0) {
    const part=assembly.get(path);selectTextureLayer({part,index,material:part.materials[index]},true);
  },
  hasPart(path) { return assembly.has(path); },
  hasMagazine() { return [...assembly.keys()].some(path => path.includes('/magazines/')); },
  activePath() { return activePartPath; },
  partSequence(path) { return assembly.get(path)?.sequence; },
  bounds(path) {
    const part = assembly.get(path), matrix = partMounts.get(path)?.matrix || new THREE.Matrix4();
    const box = new THREE.Box3();
    for (const mesh of part.meshes) for (let i = 0; i < mesh.positions.length; i += 3) {
      box.expandByPoint(new THREE.Vector3().fromArray(mesh.positions, i).applyMatrix4(matrix));
    }
    return {min: box.min.toArray(), max: box.max.toArray()};
  },
  mountedMaterials() { return group.children.map(object => object.material); },
  highlightedMeshes() {
    return group.children.filter(object => object.children.some(child => child.userData.selectionOverlay))
      .map(object => ({path: object.userData.partPath, index: object.userData.materialIndex,
        color: object.children.find(child => child.userData.selectionOverlay).material.color.getHex()}));
  },
  rebuild() { renderAssembly(); },
  setupSharedTexture() {
    const gun = assembly.get(model.path), stock = assembly.get('models/perpheads/weapons/attachments/stock_ak101.mdl');
    stock.materials[0] = {...gun.materials[0], name: 'Stock wood', vmtPath: 'materials/models/weapons/v_models/test/stock.vmt'};
    gun.materials[0].vmtPath = 'materials/models/weapons/v_models/test/body.vmt';
    gun.materials[0].vtfPath = 'materials/models/weapons/v_models/test/shared.vtf';
    stock.materials[0].vtfPath = gun.materials[0].vtfPath;
    activePartPath = gun.path; materialIndex = 0;
    renderAssembly(); renderMaterials();
  },
  removeSharedOwnerEdit() {
    const stock = assembly.get('models/perpheads/weapons/attachments/stock_ak101.mdl');
    const key = skinKey(stock.materials[0]);
    edits.delete(key);
    renderAssembly(); renderMaterials();
    return key;
  },
  uvLayers() { return activeTextureLayers().map(layer => ({
    path: layer.part.path, index: layer.index, key: uvLayerKey(layer),
    triangles: layer.part.meshes.filter(mesh => mesh.materialIndex === layer.index).reduce((n, mesh) => n + mesh.uvs.length / 6, 0)
  })); },
  uvStrokes() {
    const colors = [];
    drawUv({save(){},restore(){},beginPath(){},rect(){},clip(){},moveTo(){},lineTo(){},closePath(){},fill(){},
      stroke(){ colors.push(this.strokeStyle); }},512,512);
    return colors;
  },
  textureEdits() { return group.children.map(object => ({path: object.userData.partPath,
    key: object.userData.skinKey, source: object.material.map?.image?.src})); },
  resetEdits() { draftSession = null; edits.clear(); partCache.clear(); },
  setupMaskedTexture(width = 4, height = width) {
    const canvas = document.createElement('canvas'); canvas.width = width; canvas.height = height;
    const context = canvas.getContext('2d'); context.fillStyle = 'rgba(200,100,50,0.125)';
    context.fillRect(0, 0, width, height);
    const material = activeMaterial(); material.image = canvas.toDataURL();
    material.width = width; material.height = height;
    renderAssembly(); renderMaterials(); return material.image;
  }
};
"""

CHECK_MATERIALS = r"""
async () => {
  const {THREE, createSourceMaterial, makeMaterial, makeThumbnail} = previewTest;
  const check = (condition, message) => { if (!condition) throw Error(message); };
  const info = {vmtText: '"UnlitGeneric"\n{\n"$additive" "1"\n}'};
  const reticle = createSourceMaterial(info);
  check(reticle.isMeshBasicMaterial, 'Unlit reticles must not depend on lighting');
  check(reticle.transparent && !reticle.depthWrite && reticle.depthTest, 'Reticle depth flags');
  check(reticle.blending === THREE.CustomBlending && reticle.blendSrc === THREE.SrcAlphaFactor
    && reticle.blendDst === THREE.OneFactor && reticle.blendSrcAlpha === THREE.ZeroFactor
    && reticle.blendDstAlpha === THREE.OneFactor, 'Reticle additive blending must preserve destination alpha');
  const lit = createSourceMaterial({vmtText: 'VertexLitGeneric\n{\n"$additive" "0"\n}'});
  check(lit.isMeshStandardMaterial && !lit.transparent && lit.depthWrite, 'Opaque materials unchanged');
  const alpha = createSourceMaterial({vmtText: '"VertexLitGeneric"\n{\n"$alphatest" "1"\n"$alphatestreference" "0.3"\n"$translucent" "1"\n}'});
  check(alpha.alphaTest === 0.3 && alpha.transparent && !alpha.depthWrite, 'Existing transparency flags');
  const unquoted = createSourceMaterial({vmtText: 'unlitgeneric\n{\n$additive 1\n}'});
  check(unquoted.isMeshBasicMaterial && unquoted.transparent, 'Unquoted case-insensitive VMT');

  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = 8;
  const context = canvas.getContext('2d');
  context.fillStyle = 'black'; context.fillRect(0, 0, 8, 8);
  context.fillStyle = 'red'; context.fillRect(4, 0, 4, 8);
  info.image = canvas.toDataURL();
  const material = makeMaterial(info);
  const deadline = performance.now() + 5000;
  while (!material.map.image && performance.now() < deadline) await new Promise(resolve => setTimeout(resolve, 10));
  check(material.map.image, 'Preview texture did not load');
  await material.map.image.decode();
  material.map.minFilter = material.map.magFilter = THREE.NearestFilter;
  const renderer = new THREE.WebGLRenderer({preserveDrawingBuffer: true});
  renderer.setSize(32, 32);
  const scene = new THREE.Scene(); scene.background = new THREE.Color(0x204060);
  const camera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0.1, 10); camera.position.z = 2;
  const mesh = new THREE.Mesh(new THREE.PlaneGeometry(2, 2), material);
  const gl = renderer.getContext();
  const pixel = x => { const p = new Uint8Array(4); gl.readPixels(x, 16, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, p); return [...p]; };
  renderer.render(scene, camera); const background = pixel(8);
  scene.add(mesh); renderer.render(scene, camera);
  const black = pixel(8), red = pixel(24);
  check(black.every((value, i) => Math.abs(value - background[i]) <= 1), 'Black reticle pixels must leave the background unchanged');
  check(red[0] > background[0] + 100 && red[1] >= background[1] - 1, 'Red reticle must remain visible without lights');

  // An opaque object in front must still occlude the additive reticle.
  const occluder = new THREE.Mesh(mesh.geometry, new THREE.MeshBasicMaterial({color: 0x00ff00}));
  occluder.position.z = 0.5; scene.add(occluder); renderer.render(scene, camera);
  check(pixel(24)[0] < 2 && pixel(24)[1] > 250, 'Reticle must not show through the sight housing');
  scene.remove(occluder); occluder.material.dispose();

  // Texture imports replace the map without replacing the material.
  material.map.dispose(); material.map = new THREE.CanvasTexture(canvas);
  material.map.colorSpace = THREE.SRGBColorSpace; material.needsUpdate = true;
  renderer.render(scene, camera);
  check(pixel(8).every((value, i) => Math.abs(value - background[i]) <= 1), 'Imported textures retain additive blending');

  const thumbnail = await makeThumbnail({
    materials: [info],
    meshes: [{materialIndex: 0, positions: [-1,-1,0, 1,-1,0, 1,1,0, -1,-1,0, 1,1,0, -1,1,0],
      normals: [0,0,1, 0,0,1, 0,0,1, 0,0,1, 0,0,1, 0,0,1],
      uvs: [0,0, 1,0, 1,1, 0,0, 1,1, 0,1]}]
  });
  const image = new Image(); image.src = thumbnail; await image.decode();
  canvas.width = canvas.height = 80; context.drawImage(image, 0, 0);
  const pixels = context.getImageData(0, 0, 80, 80).data;
  let visibleRed = false;
  for (let i = 0; i < pixels.length; i += 4) {
    check(pixels[i] >= pixels[0] - 2 && pixels[i+1] >= pixels[1] - 2 && pixels[i+2] >= pixels[2] - 2, 'Thumbnail has a dark reticle rectangle');
    visibleRed ||= pixels[i] > pixels[0] + 30;
  }
  check(visibleRed, 'Thumbnail lost the reticle');
  mesh.geometry.dispose(); material.map.dispose(); material.dispose();
  [reticle, lit, alpha, unquoted].forEach(m => m.dispose()); renderer.dispose();
}
"""

CHECK_TRANSPARENT_CANVAS = """
() => {
  const T = previewTest.THREE;
  const canvas = document.createElement('canvas'); canvas.id = 'compositing-check';
  canvas.style.cssText = 'position:fixed;left:0;top:0;z-index:10000;background:rgb(32,64,96)';
  document.body.append(canvas);
  const renderer = new T.WebGLRenderer({canvas, alpha: true, preserveDrawingBuffer: true});
  renderer.setSize(64, 64);
  const scene = new T.Scene();
  const camera = new T.OrthographicCamera(-1,1,1,-1,.1,10); camera.position.z = 2;
  const texture = new T.DataTexture(new Uint8Array([0,0,0,255,255,0,0,255]), 2,1);
  texture.colorSpace = T.SRGBColorSpace; texture.needsUpdate = true;
  const material = previewTest.createSourceMaterial({vmtText:'UnlitGeneric\\n{\\n$additive 1\\n}'});
  material.map = texture;
  const geometry = new T.PlaneGeometry(2,2);
  scene.add(new T.Mesh(geometry, material));
  renderer.render(scene, camera);
  window.cleanupCompositeTest = () => {
    geometry.dispose(); texture.dispose(); material.dispose(); renderer.dispose(); canvas.remove();
    delete window.cleanupCompositeTest;
  };
}
"""

CHECK_COMPOSITE_PIXELS = """
async png => {
  const image = new Image(); image.src = 'data:image/png;base64,' + png; await image.decode();
  const canvas = document.createElement('canvas'); canvas.width = canvas.height = 64;
  const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
  const black = context.getImageData(16, 32, 1, 1).data;
  const red = context.getImageData(48, 32, 1, 1).data;
  if (![32,64,96,255].every((value, i) => Math.abs(black[i] - value) <= 1))
    throw Error('Black reticle pixels obscure the CSS background: ' + black);
  if (red[0] < 250 || red[1] < 63 || red[2] < 95)
    throw Error('Reticle is not added to the CSS background: ' + red);
}
"""


def serve(route):
    url = urlsplit(route.request.url)
    path = url.path.lstrip("/")
    if path.startswith("model/"):
        sequence = parse_qs(url.query).get("sequence", [None])[0]
        route.fulfill(json=model_detail(path.removeprefix("model/"), None if sequence is None else int(sequence)))
        return
    file = ROOT / "ui" / (path or "index.html")
    if not file.is_file():
        raise FileNotFoundError(file)
    body = file.read_bytes()
    if path == "app.js":
        body += HOOKS.encode("utf-8")
    content_type = "text/javascript" if file.suffix == ".js" else mimetypes.guess_type(file)[0]
    route.fulfill(body=body, content_type=content_type or "application/octet-stream")


with sync_playwright() as playwright:
    browser = playwright.chromium.launch(channel="msedge", headless=True, args=[
        "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
    ])
    try:
        page = browser.new_page(viewport={"width": 1500, "height": 950})
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.route(ORIGIN + "**", serve)
        page.add_init_script("""
          window.draftMock = {
            failSave: false, installed: new Set(), calls: [], exports: [], copies: [], failCopy: false,
            clipboardImage: null, deferClipboard: false, imageExports: [], cancelImageSave: false,
            updateChecks: 0, updateAvailable: null, failUpdateCheck: false,
            saved: new Map(Object.entries(JSON.parse(localStorage.getItem('mockDrafts') || '{}'))),
            variants: new Map(Object.entries(JSON.parse(localStorage.getItem('mockVariants') || '{}')))
          };
          window.__TAURI__ = {core: {invoke: async (command, args) => {
            const mock = window.draftMock; mock.calls.push(command);
            if (command === 'combine_shared_texture') {
              mock.combineArgs=args;
              if (mock.failCombine) throw Error('Simulated UV inspection failure');
              if (mock.deferCombine) return new Promise(resolve=>{mock.resolveCombine=resolve;});
              return structuredClone(mock.combineResult);
            }
            if (command === 'list_texture_variants') {
              if (mock.failVariantList) throw Error('Simulated texture-library read failure');
              return {variants:[...mock.variants.values()],warnings:mock.variantWarnings || []};
            }
            if (command === 'load_texture_variant') {
              const variant = mock.variants.get(args.id);
              if (!variant) throw Error('Texture variant no longer exists');
              return structuredClone(variant);
            }
            if (command === 'rename_texture_variant') {
              if (mock.failVariantRename) throw Error('Simulated texture rename failure');
              const variant=mock.variants.get(args.id), name=args.name.trim();
              if(!variant) throw Error('Texture variant no longer exists');
              if(!name || name.length>200) throw Error('Texture name must contain 1-200 printable characters');
              variant.name=name;
              localStorage.setItem('mockVariants',JSON.stringify(Object.fromEntries(mock.variants)));
              return structuredClone(variant);
            }
            if (command === 'save_texture_variant') {
              if (mock.failVariantSave) throw Error('Simulated texture-library disk failure');
              const texture = args.texture;
              const existing = [...mock.variants.values()].find(item => item.png === texture.png
                && item.vmtPath === texture.vmtPath && item.vtfPath === texture.vtfPath && item.vmtText === texture.vmtText);
              if (existing) {
                existing.name = texture.name;
                localStorage.setItem('mockVariants', JSON.stringify(Object.fromEntries(mock.variants)));
                return existing;
              }
              const image = new Image(); image.src = texture.png; await image.decode();
              const canvas = document.createElement('canvas'); canvas.width=image.width; canvas.height=image.height;
              const context=canvas.getContext('2d'); context.drawImage(image,0,0);
              const pixels=context.getImageData(0,0,image.width,image.height);
              let hasAlpha=false;
              for(let i=3;i<pixels.data.length;i+=4){if(pixels.data[i]<255)hasAlpha=true;pixels.data[i]=255;}
              context.putImageData(pixels,0,0);
              const next = Math.max(0, ...[...mock.variants.keys()].map(id => Number(id.split('-').at(-1)))) + 1;
              const variant = {...texture,id:'variant-' + next,
                width:image.width,height:image.height,preview:canvas.toDataURL(),hasAlpha};
              mock.variants.set(variant.id,variant);
              localStorage.setItem('mockVariants', JSON.stringify(Object.fromEntries(mock.variants)));
              return structuredClone(variant);
            }
            if (command === 'delete_texture_variant') {
              mock.variants.delete(args.id);
              localStorage.setItem('mockVariants', JSON.stringify(Object.fromEntries(mock.variants)));
              return;
            }
            if (command === 'open_workshop') return;
            if (command === 'workshop_stage') { mock.stagedWorkshop = args; return; }
            if (command === 'workshop_list') return {skins:[], next:null};
            if (command === 'workshop_pending') return null;
            if (command === 'workshop_account') return mock.workshopConnected ? {steamId:'76561198000000000'} : {connected:false};
            if (command === 'choose_content_folder') return 'C:\\\\test-content';
            if (command === 'scan_content') return {root:args.root, models:[], customSkins:[],
              archiveCount:1, modelCount:0, hasInstallRoot:true};
            if (command === 'copy_png') {
              if (mock.failCopy) throw Error('Clipboard busy');
              mock.copies.push(args.pngBase64); (mock.copyAlphaChoices ||= []).push(args.stripAlpha); return;
            }
            if (command === 'import_clipboard_image') {
              if (mock.deferClipboard) return new Promise(resolve => { mock.resolveClipboard = resolve; });
              if (!mock.clipboardImage) throw Error('Clipboard contains no image pixels');
              return mock.clipboardImage;
            }
            if (command === 'import_image') {
              if (!mock.decodedImage) throw Error('Unsupported or damaged texture');
              return mock.decodedImage;
            }
            if (command === 'save_png' || command === 'save_dds') {
              mock.imageExports.push({command, ...args});
              return mock.cancelImageSave ? null : 'texture.' + (command === 'save_dds' ? 'dds' : 'png');
            }
            if (command === 'check_update') {
              mock.updateChecks++;
              if (mock.failUpdateCheck) throw Error('Simulated offline update check');
              return {currentVersion: '0.2.0', availableVersion: mock.updateAvailable, notes: 'Latest release notes'};
            }
            if (command === 'update_log') return [{tag_name:'v0.2.0',name:'Texture Helper 0.2.0',body:'Release notes <script>not executable</script>'}];
            if (command === 'install_update') throw Error('Simulated installer failure');
            if (command === 'list_custom_skins') return mock.customInventory || [];
            if (command === 'installed_skin_models') return mock.matchingModels || [];
            if (command === 'edit_installed_skin') {
              if (mock.failInstalledImport) throw Error('Installed texture could not be decoded');
              mock.importedModel = args.modelPath;
              const existing = mock.installedImport;
              const draft = {...existing,id:'installed-edit-copy',modelPath:args.modelPath,archivedFiles:[]};
              mock.saved.set(draft.id,draft);
              mock.installed.add(draft.id);
              return draft;
            }
            if (command === 'list_drafts') return [...mock.saved.values()].map(draft => ({
              id: draft.id, name: draft.name, modelPath: draft.modelPath, textureCount: draft.textures.length,
              installedCount: mock.installed.has(draft.id) ? (draft.archivedFiles?.length ? 1 : draft.textures.length) : 0,
              installableCount: draft.archivedFiles?.length ? 1 : draft.textures.length,
              archived: !!draft.archivedFiles?.length, installedIds: []
            }));
            if (command === 'export_draft' || command === 'export_custom_skin') {
              if (mock.failPackageExport) throw Error('Simulated package export failure');
              mock.packageExports ||= [];
              mock.packageExports.push({command, id:args.id, draft:structuredClone(mock.saved.get(args.id))});
              return mock.cancelPackageExport ? null : 'shared.phskin';
            }
            if (command === 'import_skin') {
              if (mock.failPackageImport) throw Error('Invalid skin package');
              if (!mock.packageImport) return null;
              const draft = structuredClone(mock.packageImport);
              draft.id = 'imported-' + (mock.saved.size + 1);
              mock.saved.set(draft.id, draft);
              localStorage.setItem('mockDrafts', JSON.stringify(Object.fromEntries(mock.saved)));
              return draft.name;
            }
            if (command === 'save_draft') {
              if (mock.failSave) throw Error('Simulated disk failure');
              const draft = structuredClone(args.draft);
              if (draft.componentTextures?.length) {
                draft.componentError = mock.componentConflict ? '7 UV pixels have conflicting component colours or alpha' : null;
                if (!draft.componentError && mock.combinedPng) for (const texture of draft.textures) texture.png=mock.combinedPng;
              }
              draft.id ||= 'draft-' + (mock.saved.size + 1);
              mock.saved.set(draft.id, draft);
              localStorage.setItem('mockDrafts', JSON.stringify(Object.fromEntries(mock.saved)));
              return draft;
            }
            if (command === 'prepare_draft_output') {
              const draft=structuredClone(mock.saved.get(args.id));
              if(mock.componentConflict) throw Error('7 UV pixels have conflicting component colours or alpha');
              draft.componentTextures=[];delete draft.componentError;
              return draft;
            }
            if (command === 'load_draft') {
              const draft=structuredClone(mock.saved.get(args.id));
              draft.variantIds=Object.fromEntries(draft.textures.flatMap(texture=>{
                const match=[...mock.variants.values()].find(variant=>variant.png===texture.png
                  &&variant.vmtPath===texture.vmtPath&&variant.vtfPath===texture.vtfPath&&variant.vmtText===texture.vmtText);
                return match?[[texture.key,match.id]]:[];
              }));
              draft.componentVariantIds=Object.fromEntries((draft.componentTextures||[]).flatMap(texture=>{
                const match=[...mock.variants.values()].find(variant=>variant.png===texture.png);
                return match?[[`${texture.partPath}:${texture.materialIndex}`,match.id]]:[];
              }));
              return draft;
            }
            if (command === 'delete_draft') {
              if (mock.deferDelete) await new Promise(resolve=>{mock.resolveDelete=resolve;});
              mock.saved.delete(args.id);
              mock.installed.delete(args.id);
              localStorage.setItem('mockDrafts', JSON.stringify(Object.fromEntries(mock.saved)));
              return;
            }
            if (command === 'install_draft') {
              if (mock.failOverwrite) throw Error('Skin files changed since confirmation. Review again.');
              mock.lastInstallOverwrite = args.overwrite;
              const draft = mock.saved.get(args.id);
              for (const texture of draft.textures) {
                if (args.alphaChoices?.[texture.vtfPath] !== undefined) texture.stripAlpha = args.alphaChoices[texture.vtfPath];
              }
              localStorage.setItem('mockDrafts', JSON.stringify(Object.fromEntries(mock.saved)));
              mock.installed.add(args.id); return;
            }
            if (command === 'preview_skin_install') {
              if (mock.failInstallPreview) throw Error('Could not read the installed skin');
              mock.lastInstallPreview = structuredClone(args);
              return structuredClone({conflicts:mock.installConflicts || [],affectedModels:mock.installAffectedModels || [],
                warnings:mock.installImpactWarnings || []});
            }
            if (command === 'preview_component_merge') {
              mock.lastMergePreview = structuredClone(args.request);
              if (mock.failMerge) throw Error('Merge blocked: overlapping UV pixels conflict with another component');
              return structuredClone({conflicts:mock.mergeConflicts || [],affectedModels:mock.mergeAffectedModels || [],
                warnings:mock.mergeImpactWarnings || []});
            }
            if (command === 'install_component_merge') {
              if (mock.failOverwrite) throw Error('Skin files changed since confirmation. Review again.');
              mock.lastMergeInstall = structuredClone(args);
              mock.mergeInstalls = (mock.mergeInstalls || 0) + 1;
              const source=mock.saved.get(args.request.id);
              const merged={...structuredClone(source),id:'merged-'+mock.mergeInstalls,
                name:source.name+' - merged installation',textures:[],componentTextures:[],
                archivedFiles:[{root:'vtf',relative:'merged_custom.vtf',bytes:'mock'}]};
              mock.saved.set(merged.id,merged);mock.installed.add(merged.id);
              return merged.name;
            }
            if (command === 'export_textures') {
              if (mock.failOverwrite) throw Error('Skin files changed since confirmation. Review again.');
              mock.lastInstallOverwrite = args.overwrite;
              mock.exports.push(args.skins); return args.skins.length;
            }
            if (command === 'texture_has_alpha') {
              const image = new Image(); image.src = args.pngBase64; await image.decode();
              const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
              const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
              const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
              for (let i = 3; i < pixels.length; i += 4) if (pixels[i] < 255) return true;
              return false;
            }
            if (command === 'uninstall_draft') { mock.installed.delete(args.id); return 1; }
            if (command === 'texture_rgb_preview') {
              const image = new Image(); image.src = args.pngBase64; await image.decode();
              const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
              const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
              const pixels = context.getImageData(0, 0, canvas.width, canvas.height);
              for (let i = 3; i < pixels.data.length; i += 4) pixels.data[i] = 255;
              context.putImageData(pixels, 0, 0); return canvas.toDataURL();
            }
            if (command !== 'open_model') throw Error('Unexpected command: ' + command);
            const sequence = args.sequence === undefined ? '' : '?sequence=' + args.sequence;
            const detail=await (await fetch('/model/' + args.path + sequence)).json();
            if(mock.componentFixture?.paths.includes(args.path)) detail.materials[0]={...mock.componentFixture.material};
            return detail;
          }}};
        """)
        page.goto(ORIGIN)
        page.wait_for_function("window.previewTest !== undefined")
        page.wait_for_function("draftMock.updateChecks === 1")
        page.evaluate("previewTest.refreshSkins()")
        page.evaluate("window.dispatchEvent(new Event('focus'))")
        page.wait_for_timeout(100)
        assert not page.locator("#toast").inner_text().startswith("Saved skins could not"), "Refresh failed without a selected model"
        assert not page.locator("#toast").inner_text().startswith("Saved skins could not be loaded"), "Startup refresh failed"
        page.evaluate("draftMock.variantWarnings=['Installed test: VTF missing'];")
        page.evaluate("previewTest.refreshSkins()")
        page.locator('[data-library="textures"]').click()
        assert page.locator("#texture-library-warnings").is_visible()
        assert "VTF missing" in page.locator("#texture-warning-detail").text_content()
        page.evaluate("draftMock.failVariantList=true")
        page.evaluate("previewTest.refreshSkins()")
        assert "Texture library could not be refreshed" in page.locator("#toast").inner_text()
        page.evaluate("draftMock.failVariantList=false;draftMock.variantWarnings=[]")
        page.evaluate("previewTest.refreshSkins()")
        page.locator('[data-library="models"]').click()
        print("PASS: startup/focus refresh without a selected model, visible pairing warnings and independent draft refresh")
        assert page.locator("#update-status").inner_text() == "Up to date"
        assert page.locator("#app-version").inner_text() == "v0.2.0"
        logo = page.locator(".brand").bounding_box()
        updates = page.locator("#updates-button").bounding_box()
        assert updates["x"] >= logo["x"] + logo["width"]
        page.locator("#updates-button").click()
        assert page.locator("#update-log").inner_text().endswith("<script>not executable</script>")
        assert page.locator("#update-log script").count() == 0
        page.locator("#close-updates").click()
        page.evaluate(CHECK_MATERIALS)
        page.evaluate(CHECK_TRANSPARENT_CANVAS)
        composite = page.locator("#compositing-check").screenshot()
        page.evaluate(CHECK_COMPOSITE_PIXELS, base64.b64encode(composite).decode("ascii"))
        page.evaluate("cleanupCompositeTest()")
        print("PASS: additive pixels, unlit reticles, depth occlusion, imports, thumbnails, existing flags")
        print("PASS: transparent canvas preserves the CSS background under black reticle pixels")

        page.evaluate("""weapons => {
          for (const weapon of Object.values(weapons)) {
            const defaults = previewTest.resolveLoadout(weapon, new Map());
            const empty = previewTest.resolveLoadout(weapon, new Map([['SLOT_MAGAZINE', null]]));
            if (empty.has('SLOT_MAGAZINE')) throw Error(weapon.key + ': magazine was re-added');
            for (const [slot, entry] of defaults) {
              if (slot !== 'SLOT_MAGAZINE' && empty.get(slot)?.name !== entry.name)
                throw Error(weapon.key + ': unrelated default changed');
            }
            if (defaults.has('SLOT_MAGAZINE') && !previewTest.slotChoices(weapon).some(([slot]) => slot === 'SLOT_MAGAZINE'))
              throw Error(weapon.key + ': default magazine has no selector');
          }
        }""", data["weapons"])
        print(f"PASS: magazine override and other defaults across all {len(data['weapons'])} guns")

        page.evaluate("""data => {
          const expected = {
            SLOT_SIGHT: 'Sight', SLOT_STOCK: 'Stock', SLOT_SILENCER: 'Muzzle',
            SLOT_MAGAZINE: 'Magazine', SLOT_UNDERBARREL: 'Underbarrel', SLOT_LEFT_RAIL: 'Laser',
            SLOT_IRON_FRONT: 'Iron sights', SLOT_IRON_REAR: 'Iron sights',
            SLOT_TOP_RAIL: 'Rails / Mounts', SLOT_SCOPE_RINGS: 'Rails / Mounts'
          };
          for (const part of Object.values(data.attachments)) {
            if (previewTest.modelCategory(part.model) !== expected[part.slot])
              throw Error('Wrong category: ' + part.model);
          }
          for (const path of Object.keys(data.weapons))
            if (previewTest.modelCategory(path) !== 'Weapons') throw Error('Wrong gun category: ' + path);
        }""", data)
        print("PASS: weapons and every Lua-defined attachment have separate library categories")

        page.evaluate("previewTest.open('models/perpheads/weapons/c_ak101.mdl')")
        page.wait_for_timeout(100)
        idle_frames = page.evaluate("previewTest.previewRenderCount()")
        page.wait_for_timeout(100)
        assert page.evaluate("previewTest.previewRenderCount()") == idle_frames, "The model preview rendered continuously while idle"
        page.locator("#viewport").evaluate("canvas=>canvas.dispatchEvent(new WheelEvent('wheel',{deltaY:-1,cancelable:true}))")
        page.wait_for_function("previewTest.previewRenderCount() > " + str(idle_frames))
        print("PASS: model preview renders on changes, not continuously while idle")
        page.locator("#show-uv").uncheck()
        original = page.evaluate("previewTest.setupMaskedTexture()")
        page.evaluate("previewTest.holdTextureRedraws();previewTest.rebuild()")
        page.wait_for_function("window.pendingTextureRedraws()>0")
        page.wait_for_timeout(100)
        before_texture_ready = page.evaluate("previewTest.previewRenderCount()")
        page.evaluate("window.releaseTextureRedraws()")
        page.wait_for_function("count=>previewTest.previewRenderCount()>count",arg=before_texture_ready)
        print("PASS: delayed model textures trigger redraw without moving the model")
        page.wait_for_function("""() => {
          const p = document.getElementById('texture-canvas').getContext('2d').getImageData(1,1,1,1).data;
          return p[0] > 190 && p[1] > 90 && p[3] === 255;
        }""")
        assert page.locator("#texture-size").inner_text() == "4 × 4"
        assert page.locator("#materials, #component-edit-controls").count() == 0
        assert page.locator("#texture-variants").locator("button").count() == 1
        assert page.locator("#texture-variants small").count() == 0
        assert page.locator("#uv-layers").is_hidden()
        assert page.locator("#uv-layers input").count() == 0
        assert page.locator("#show-uv").is_visible()
        page.evaluate("previewTest.showComponentOverview()")
        assert page.locator("#shared-texture-overview").is_hidden(), "Single-owner textures appeared as shared cards"
        page.evaluate("previewTest.selectPart(previewTest.activePath())")
        page.locator("#show-alpha").check()
        page.wait_for_function("""() => {
          const p = document.getElementById('texture-canvas').getContext('2d').getImageData(1,1,1,1).data;
          return p[3] > 25 && p[3] < 40;
        }""")
        assert original in page.evaluate("previewTest.textureEdits()")[0]["source"]
        page.locator("#copy-png").click()
        page.wait_for_function("draftMock.copies.length === 1")
        assert page.evaluate("draftMock.copies[0]") == original
        assert page.evaluate("draftMock.copyAlphaChoices[0]") is True
        page.locator("#copy-overlay").click()
        page.wait_for_function("draftMock.copies.length === 2")
        assert page.evaluate("draftMock.copyAlphaChoices[1]") is False
        overlay = page.evaluate("""async () => {
          const image = new Image(); image.src = draftMock.copies[1]; await image.decode();
          const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
          const context = canvas.getContext('2d'); context.drawImage(image,0,0);
          return {width:image.width,height:image.height,pixels:[...context.getImageData(0,0,image.width,image.height).data]};
        }""")
        assert overlay["width"] == overlay["height"] == 4
        assert any(alpha == 0 for alpha in overlay["pixels"][3::4])
        page.evaluate("draftMock.failCopy = true")
        page.locator("#copy-png").click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('Clipboard busy')")
        assert page.locator("#toast").get_attribute("class") == "toast error"
        page.evaluate("draftMock.failCopy = false")
        assert page.locator("#copy-png").inner_text() == "Copy PNG to clipboard"
        assert page.locator("#copy-overlay").inner_text() == "Copy UV overlay to clipboard"
        assert page.locator("#download-overlay").inner_text() == "Export UV overlay"
        assert page.locator("#export-png, #export-dds").count() == 0
        for format_name, command in (("png", "save_png"), ("dds", "save_dds")):
            page.locator("#export-image").click()
            page.locator("#export-format").select_option(format_name)
            page.locator("#export-dialog button[value=export]").click()
            page.wait_for_function("command => draftMock.imageExports.at(-1)?.command === command", arg=command)
            assert page.evaluate("draftMock.imageExports.at(-1).pngBase64") == original
        page.locator("#export-image").click()
        page.locator("#export-dialog button[value=cancel]").click()
        assert page.evaluate("draftMock.imageExports.length") == 2
        page.evaluate("draftMock.cancelImageSave = true")
        page.locator("#export-image").click()
        page.locator("#export-dialog button[value=export]").click()
        page.wait_for_function("draftMock.imageExports.length === 3")
        page.evaluate("draftMock.cancelImageSave = false")
        page.locator("#download-overlay").click()
        page.wait_for_function("draftMock.imageExports.length === 4")
        assert page.evaluate("draftMock.imageExports.at(-1).command") == "save_png"
        assert page.evaluate("draftMock.imageExports.at(-1).name.endsWith('_uv_overlay')")
        page.locator("#import-clipboard").click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('no image pixels')")
        assert page.evaluate("draftMock.saved.size") == 0
        page.evaluate("""() => {
          const canvas = document.createElement('canvas'); canvas.width = 2; canvas.height = 1;
          draftMock.clipboardImage = canvas.toDataURL();
        }""")
        page.locator("#import-clipboard").click()
        page.locator("#resize-image-dialog").wait_for(state="visible")
        assert "2 × 1" in page.locator("#resize-image-message").inner_text()
        assert "4 × 4" in page.locator("#resize-image-message").inner_text()
        page.locator('#resize-image-dialog button[value="cancel"]').click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('import cancelled')")
        assert page.evaluate("previewTest.draftState().edits.length") == 0
        page.locator("#show-alpha").uncheck()
        page.locator("#show-uv").check()
        print("PASS: RGB panel/swatches reveal masked color; alpha toggle and 3D source retain original alpha")
        assert page.locator(".workspace #draft-name").is_visible()
        page.get_by_label("Draft name", exact=True).fill("Named before importing")
        page.get_by_label("Draft name", exact=True).dispatch_event("change")
        assert page.evaluate("previewTest.draftState().name") == "Named before importing"
        assert page.evaluate("draftMock.saved.size") == 0
        assert page.evaluate("previewTest.highlightedMeshes().length") == 0
        before = page.evaluate("previewTest.mountedMaterials().map(material => material.uuid)")
        page.evaluate("previewTest.selectActiveShared()")
        highlighted = page.evaluate("previewTest.highlightedMeshes()")
        assert highlighted and all(item == {"path": "models/perpheads/weapons/c_ak101.mdl", "index": 0, "color": 0xff765c}
                                   for item in highlighted)
        assert page.evaluate("previewTest.mountedMaterials().map(material => material.uuid)") == before
        page.evaluate("previewTest.rebuild()")
        assert page.evaluate("previewTest.highlightedMeshes()") == highlighted
        page.locator("#viewport").click(position={"x": 5, "y": 5})
        assert page.evaluate("previewTest.highlightedMeshes().length") == 0
        other_path = page.evaluate("previewTest.uvLayers()[0].path")
        other_path = next(part["model"] for name, part in data["attachments"].items()
                          if name in data["weapons"]["models/perpheads/weapons/c_ak101.mdl"]["defaults"]
                          and part["model"] != other_path)
        page.evaluate("path => previewTest.selectPart(path)", other_path)
        page.evaluate("previewTest.selectActiveShared()")
        highlighted = page.evaluate("previewTest.highlightedMeshes()")
        assert highlighted and all(item["path"] != "models/perpheads/weapons/c_ak101.mdl" for item in highlighted)
        page.locator("#viewport").click(button="right")
        assert page.evaluate("previewTest.highlightedMeshes()") == highlighted
        print("PASS: scoped highlights preserve textures, survive rebuild and right-click, clear on empty left-click")

        page.evaluate("previewTest.setSlotAttachment('SLOT_STOCK', 'stock_ak101')")
        if args.fixtures:
            page.evaluate("previewTest.selectTexture('materials/models/weapons/v_models/ak101/wood.vtf')")
            assert len(page.evaluate("previewTest.uvLayers()")) == 2
            assert page.locator("#material-name").inner_text() == "wood.vtf"
            point = page.evaluate("previewTest.surfacePoint('models/perpheads/weapons/attachments/stock_ak101.mdl')")
            page.mouse.click(point["x"], point["y"])
            assert page.evaluate("previewTest.activePath()") == "models/perpheads/weapons/attachments/stock_ak101.mdl"
            assert page.evaluate("previewTest.uvLayers().length") == 2
            assert {item["path"] for item in page.evaluate("previewTest.highlightedMeshes()")} == {
                "models/perpheads/weapons/c_ak101.mdl", "models/perpheads/weapons/attachments/stock_ak101.mdl"
            }
            print("PASS: actual mounted AK101 stock click opens shared wood VTF and highlights stock/body")
        page.evaluate("previewTest.setupSharedTexture()")
        page.evaluate("previewTest.selectTexture('materials/models/weapons/v_models/test/shared.vtf')")
        layers = page.evaluate("previewTest.uvLayers()")
        assert len(layers) == 2 and all(layer["triangles"] > 0 for layer in layers)
        assert page.locator("#uv-coordinates").text_content() == "", "Closed coordinate panel formatted triangle text"
        page.locator("#uv-coordinate-details summary").click()
        page.wait_for_function("document.getElementById('uv-coordinates').textContent.includes('stock ak101')")
        assert page.locator("#uv-layers input").count() == 2
        for checkbox in page.locator("#uv-layers input").all():
            checkbox.uncheck()
        page.locator("#copy-overlay").click()
        page.wait_for_function("draftMock.copies.length === 3")
        assert page.evaluate("""async () => {
          const image = new Image(); image.src = draftMock.copies[2]; await image.decode();
          const canvas = document.createElement('canvas'); canvas.width=image.width;canvas.height=image.height;
          const context=canvas.getContext('2d');context.drawImage(image,0,0);
          return [...context.getImageData(0,0,image.width,image.height).data].every(value=>value===0);
        }""")
        for checkbox in page.locator("#uv-layers input").all():
            checkbox.check()
        assert set(page.evaluate("previewTest.uvStrokes()")) == {"#ff765c", "#62c4ff"}
        assert len({item["path"] for item in page.evaluate("previewTest.highlightedMeshes()")}) == 2
        assert {item["color"] for item in page.evaluate("previewTest.highlightedMeshes()")} == {0xff765c, 0x62c4ff}
        assert page.locator("#material-name").inner_text() == "shared.vtf"
        assert page.locator("#texture-select").count() == 0
        page.evaluate("previewTest.setupPicking()")
        assert page.locator("#viewport").evaluate("element => getComputedStyle(element).cursor") == "default"
        stock_path = "models/perpheads/weapons/attachments/stock_ak101.mdl"
        point = page.evaluate("path => previewTest.meshPoint(path)", stock_path)
        rotation = page.evaluate("previewTest.rotation()")
        page.mouse.click(point["x"], point["y"])
        assert page.evaluate("previewTest.activePath()") == stock_path
        assert len({item["path"] for item in page.evaluate("previewTest.highlightedMeshes()")}) == 2
        assert page.locator("#material-name").inner_text() == "shared.vtf"
        assert page.evaluate("previewTest.rotation()") == rotation
        highlighted = page.evaluate("previewTest.highlightedMeshes()")
        page.mouse.move(point["x"], point["y"])
        page.mouse.down(button="left")
        page.mouse.move(point["x"] + 45, point["y"] + 30)
        page.mouse.up(button="left")
        assert page.evaluate("previewTest.rotation()") == rotation
        assert page.evaluate("previewTest.highlightedMeshes()") == highlighted
        page.mouse.move(point["x"], point["y"])
        page.mouse.down(button="right")
        page.mouse.move(point["x"] + 45, point["y"] + 30)
        assert page.locator("#viewport").evaluate("element => getComputedStyle(element).cursor") == "grabbing"
        page.mouse.up(button="right")
        assert page.locator("#viewport").evaluate("element => getComputedStyle(element).cursor") == "default"
        assert page.evaluate("previewTest.rotation()") != rotation
        assert page.evaluate("previewTest.highlightedMeshes()") == highlighted
        rotation = page.evaluate("previewTest.rotation()")
        page.keyboard.down("Shift")
        page.mouse.move(point["x"], point["y"])
        page.mouse.down(button="right")
        page.mouse.move(point["x"] + 30, point["y"])
        page.mouse.up(button="right")
        page.keyboard.up("Shift")
        assert page.evaluate("previewTest.rotation()") != rotation
        assert page.evaluate("previewTest.highlightedMeshes()") == highlighted
        page.locator("#viewport").click(position={"x": 5, "y": 5})
        assert page.evaluate("previewTest.highlightedMeshes()") == []
        assert page.evaluate("previewTest.activePath()") == stock_path
        page.evaluate("previewTest.setupPicking(); previewTest.overlapPicking()")
        point = page.evaluate("previewTest.meshPoint('models/perpheads/weapons/c_ak101.mdl')")
        page.mouse.click(point["x"], point["y"])
        assert page.evaluate("previewTest.activePath()") == "models/perpheads/weapons/c_ak101.mdl"
        assert len({item["path"] for item in page.evaluate("previewTest.highlightedMeshes()")}) == 2
        assert page.locator("#material-name").inner_text() == "shared.vtf"
        page.evaluate("previewTest.selectTexture('materials/models/weapons/v_models/test/shared.vtf')")
        page.evaluate("previewTest.rebuild()")
        page.evaluate("previewTest.selectTexture('materials/models/weapons/v_models/test/shared.vtf')")
        print("PASS: nearest-mesh picking, shared VTF selection, compact fallback, inert left-drag, right rotation and Shift roll")
        page.locator("#uv-layers input").last.uncheck()
        assert set(page.evaluate("previewTest.uvStrokes()")) == {"#ff765c"}
        assert "stock ak101" not in page.locator("#uv-coordinates").inner_text()
        page.locator("#uv-coordinate-details summary").click()
        page.wait_for_function("document.getElementById('uv-coordinates').textContent === ''")
        page.locator("#uv-layers input").last.check()
        page.locator("#uv-layers").get_by_role("button", name="Highlight", exact=True).last.click()
        assert all(item["path"].endswith("stock_ak101.mdl") for item in page.evaluate("previewTest.highlightedMeshes()"))
        assert {item["color"] for item in page.evaluate("previewTest.highlightedMeshes()")} == {0x62c4ff}
        page.evaluate("previewTest.rebuild()")
        assert {item["color"] for item in page.evaluate("previewTest.highlightedMeshes()")} == {0x62c4ff}
        png = page.evaluate("previewTest.uploadPng()")
        page.locator("#png-input").set_input_files({"name": "shared.png", "mimeType": "image/png",
            "buffer": base64.b64decode(png.split(",")[1])})
        page.wait_for_function("draftMock.saved.values().next().value?.textures.length === 2")
        state = page.evaluate("previewTest.draftState()")
        assert len(state["edits"]) == 2 and len({edit["png"] for edit in state["edits"]}) == 1
        assert page.locator("#install-all").inner_text() == "Install all edited textures (1)"
        page.wait_for_function("""() => previewTest.textureEdits().filter(item =>
          previewTest.draftState().edits.some(edit => edit.key === item.key)).every(item => item.source)""")
        assert {item["source"] for item in page.evaluate("previewTest.textureEdits()")
                if item["key"] in {edit["key"] for edit in state["edits"]}} == {png}
        clipboard_png = page.evaluate("draftMock.clipboardImage = previewTest.uploadAlphaPng()")
        page.locator("#import-clipboard").click()
        page.wait_for_function("png => [...draftMock.saved.values()][0]?.textures.every(texture => texture.png === png)", arg=clipboard_png)
        state = page.evaluate("previewTest.draftState()")
        assert len(state["edits"]) == 2 and all(edit["png"] == clipboard_png for edit in state["edits"])
        page.wait_for_function("""png => previewTest.textureEdits().filter(item =>
          previewTest.draftState().edits.some(edit => edit.key === item.key)).every(item => item.source === png)""", arg=clipboard_png)
        assert page.evaluate("draftMock.exports.length") == 0
        page.set_viewport_size({"width": 1000, "height": 950})
        assert page.locator(".inspector").evaluate("element => element.scrollWidth <= element.clientWidth")
        for selector in ("#copy-png", "#copy-overlay", "#import-clipboard"):
            assert page.locator(selector).evaluate("element => element.scrollWidth <= element.clientWidth")
        page.set_viewport_size({"width": 1500, "height": 950})
        if args.screenshot:
            page.evaluate("previewTest.setView('front')")
            page.wait_for_timeout(500)
            page.screenshot(path=str(args.screenshot.with_stem(args.screenshot.stem + "-shared")))
        # Isolate the subsequent draft lifecycle checks from this edited synthetic fixture.
        page.evaluate("draftMock.saved.clear(); localStorage.removeItem('mockDrafts')")
        page.evaluate("previewTest.resetEdits()")
        print("PASS: one shared VTF entry, two independent colored UV layers, scoped highlights, and shared import/draft pixels")

        page.evaluate("draftMock.deferClipboard = true")
        page.locator("#import-clipboard").click()
        page.wait_for_function("typeof draftMock.resolveClipboard === 'function'")
        page.evaluate("previewTest.open('models/perpheads/weapons/c_m4a1.mdl')")
        page.evaluate("png => { draftMock.deferClipboard = false; draftMock.resolveClipboard(png); }", clipboard_png)
        page.wait_for_function("document.getElementById('toast').textContent.includes('model or component changed during import')")
        assert page.evaluate("previewTest.draftState().edits.length") == 0
        assert page.evaluate("draftMock.saved.size") == 0
        print("PASS: export format popup/cancellation, clipboard import alpha/shared drafts, dimension/no-image/race errors and narrow layout")
        flip_paths = [data["attachments"][name]["model"] for name in ("flip_front", "flip_rear")]
        for sight, expected in (("", "idle"), ("eotech", "idle_down"), ("reddot", "idle_down"), ("", "idle")):
            page.evaluate("name => previewTest.setSlotAttachment('SLOT_SIGHT', name)", sight)
            for path in flip_paths:
                assert page.evaluate("path => previewTest.partSequence(path)", path) == expected
        print("PASS: M4A1 iron sights raise without an optic, fold with one, and restore from cache")

        page.evaluate("previewTest.open('models/perpheads/weapons/c_g36.mdl')")
        if args.fixtures:
            gun_path = "models/perpheads/weapons/c_g36.mdl"
            mag_path = data["attachments"]["mag_556_45_30_2"]["model"]
            gun = page.evaluate("path => previewTest.bounds(path)", gun_path)
            mag = page.evaluate("path => previewTest.bounds(path)", mag_path)
            assert gun["min"][2] < mag["max"][2] < gun["max"][2], (gun, mag)
            assert gun["min"][1] < mag["min"][1] < mag["max"][1] < gun["max"][1], (gun, mag)
            print("PASS: actual G36 magazine seats inside receiver bounds in the UI assembly")

            page.evaluate("previewTest.renderList()")
            for category in ("Weapons", "Sight", "Stock", "Muzzle", "Magazine", "Underbarrel", "Laser", "Rails / Mounts", "Iron sights"):
                label = page.locator("#category-filters button").evaluate_all(
                    "(buttons, category) => buttons.find(button => button.textContent.startsWith(category + ' ')).textContent", category)
                page.get_by_role("button", name=label, exact=True).click()
                paths = page.locator(".model-item").evaluate_all("items => items.map(item => item.dataset.path)")
                assert paths, category
                assert page.evaluate("""([paths, category]) =>
                    paths.every(path => previewTest.modelCategory(path) === category)""", [paths, category])
            label = page.locator("#category-filters button").evaluate_all(
                "buttons => buttons.find(button => button.textContent.startsWith('All ')).textContent")
            page.get_by_role("button", name=label, exact=True).click()
            print("PASS: category buttons filter the rendered library to the correct model types")

        for gun in ("ak101", "makarov", "m4a1"):
            path = f"models/perpheads/weapons/c_{gun}.mdl"
            page.evaluate("path => previewTest.open(path)", path)
            select = page.get_by_label("Magazine attachment", exact=True)
            default_magazine = any(
                data["attachments"][name]["slot"] == "SLOT_MAGAZINE"
                for name in data["weapons"][path]["defaults"]
            )
            assert page.evaluate("previewTest.hasMagazine()") == default_magazine
            empty_value = "none" if default_magazine else ""
            select.select_option(empty_value)
            page.wait_for_function("!previewTest.hasMagazine()")
            other_slot, other_name = ("SLOT_SIGHT", "reddot") if "reddot" in data["weapons"][path]["compatible"] else ("SLOT_SILENCER", "suppressor_918")
            page.evaluate("([slot, name]) => previewTest.setSlotAttachment(slot, name)", [other_slot, other_name])
            assert not page.evaluate("previewTest.hasMagazine()"), "Sight change re-added a magazine"
            assert select.input_value() == empty_value
            assert "/magazines/" not in page.evaluate("previewTest.activePath()")
            if default_magazine:
                select.select_option("")
                page.wait_for_function("previewTest.hasMagazine()")
            magazine = next((name for name in reversed(data["weapons"][path]["compatible"])
                             if data["attachments"][name]["slot"] == "SLOT_MAGAZINE"), None)
            if magazine:
                select.select_option(magazine)
                page.wait_for_function("previewTest.hasMagazine()")
                select.select_option(empty_value)
                page.wait_for_function("!previewTest.hasMagazine()")
            print(f"PASS: {gun} magazine removal, default restore, and unrelated slot changes")

        page.evaluate("previewTest.open('models/perpheads/weapons/c_ak101.mdl')")
        page.evaluate("previewTest.setSlotAttachment('SLOT_SIGHT', 'eotech')")
        page.evaluate("previewTest.setSlotAttachment('SLOT_MAGAZINE', 'none')")
        page.get_by_label("Draft name", exact=True).fill("Name before first import")
        page.get_by_label("Draft name", exact=True).dispatch_event("change")
        for count, part in enumerate(("models/perpheads/weapons/c_ak101.mdl", data["attachments"]["eotech"]["model"]), 1):
            page.evaluate("path => previewTest.selectPart(path)", part)
            png = page.evaluate("previewTest.uploadPng()")
            page.locator("#png-input").set_input_files({
                "name": "edited.png", "mimeType": "image/png",
                "buffer": base64.b64decode(png.split(",")[1]),
            })
            page.wait_for_function("""count => previewTest.draftState().status.includes('Saved locally')
                && draftMock.saved.values().next().value?.textures.length === count""", arg=count)
        state = page.evaluate("previewTest.draftState()")
        draft_id = state["id"]
        assert len(state["edits"]) == 2
        assert page.evaluate("draftMock.saved.size") == 1, "Autosave created duplicate drafts"
        assert page.evaluate("draftMock.saved.values().next().value.name") == "Name before first import"
        assert not page.evaluate("draftMock.calls.some(command => command.startsWith('export_texture') || command === 'install_draft')")
        page.get_by_label("Draft name", exact=True).fill("My saved AK101")
        page.get_by_label("Draft name", exact=True).dispatch_event("change")
        page.wait_for_function("draftMock.saved.values().next().value.name === 'My saved AK101'")
        page.evaluate("previewTest.setSlotAttachment('SLOT_STOCK', 'stock_ak101')")
        page.evaluate("previewTest.open('models/perpheads/weapons/c_m4a1.mdl')")
        legacy_originals = page.evaluate("""() => {
          const draft = draftMock.saved.values().next().value;
          draft.aiOriginals = [{id:'legacy-texture', label:'Saved original', png:draft.textures[0].png}];
          return draft.aiOriginals;
        }""")
        page.evaluate("previewTest.showSkins()")
        row = page.locator(f'.skin-item[data-draft-id="{draft_id}"]')
        assert "My saved AK101" in row.inner_text()
        row.locator(".draft-thumb img").wait_for()
        assert row.locator(".draft-thumb img").evaluate("image => image.complete && image.naturalWidth > 0")
        framing = row.locator(".draft-thumb img").evaluate("""image => {
          const canvas=document.createElement('canvas');canvas.width=image.naturalWidth;canvas.height=image.naturalHeight;
          const context=canvas.getContext('2d');context.drawImage(image,0,0);
          const pixels=context.getImageData(0,0,canvas.width,canvas.height).data;
          const background=[42,49,57];let minX=canvas.width,minY=canvas.height,maxX=-1,maxY=-1;
          for(let y=0;y<canvas.height;y++)for(let x=0;x<canvas.width;x++){
            const i=(y*canvas.width+x)*4;
            if(Math.abs(pixels[i]-background[0])+Math.abs(pixels[i+1]-background[1])+Math.abs(pixels[i+2]-background[2])>36){
              minX=Math.min(minX,x);maxX=Math.max(maxX,x);minY=Math.min(minY,y);maxY=Math.max(maxY,y);
            }
          }
          return {width:maxX-minX+1,height:maxY-minY+1};
        }""")
        assert framing["width"] >= 40 and framing["height"] >= 3, f"Draft model thumbnail is too small or poorly oriented: {framing}"
        page.evaluate("""async id => {
          const draft = draftMock.saved.get(id);
          const before = previewTest.draftState();
          const detail = await previewTest.draftThumbnailModel(draft);
          for (const texture of draft.textures) {
            if (!detail.materials.some(material => material.vmtPath === texture.vmtPath && material.image === texture.png))
              throw Error('Thumbnail is missing saved texture edits');
          }
          if (JSON.stringify(before) !== JSON.stringify(previewTest.draftState())) throw Error('Thumbnail changed the active editor');
          if (!detail.meshes.length) throw Error('Draft thumbnail has no meshes');
        }""", draft_id)
        row.click()
        assert row.locator("button").count() == 0
        page.locator("#draft-actions").wait_for(state="visible")
        page.wait_for_function("previewTest.draftState().status.includes('restored')")
        restored = page.evaluate("previewTest.draftState()")
        assert restored["selections"] == {"SLOT_SIGHT": "eotech", "SLOT_MAGAZINE": None, "SLOT_STOCK": "stock_ak101"}
        assert restored["edits"] == state["edits"]
        assert page.evaluate("previewTest.activePath()") == data["attachments"]["stock_ak101"]["model"]
        # A native save failure must keep the edit visible, report failure, and support retry.
        page.evaluate("draftMock.failSave = true")
        page.get_by_role("button", name="Save now", exact=True).click()
        page.wait_for_function("previewTest.draftState().status.includes('Save failed')")
        assert page.locator("#draft-status").get_attribute("class") == "error"
        page.evaluate("previewTest.open('models/perpheads/weapons/c_m4a1.mdl')")
        assert page.locator("#model-path").inner_text() == "models/perpheads/weapons/c_ak101.mdl"
        assert page.evaluate("previewTest.draftState().edits.length") == 2, "Failed autosave allowed edits to be discarded"
        page.evaluate("draftMock.failSave = false")
        page.get_by_role("button", name="Save now", exact=True).click()
        page.wait_for_function("previewTest.draftState().status.includes('Saved locally')")
        assert page.evaluate("draftMock.saved.values().next().value.aiOriginals") == legacy_originals
        assert page.locator("#edit-ai, #copilot-account, #ai-edit-dialog").count() == 0
        assert not page.evaluate("draftMock.calls.some(command => command.startsWith('ai_'))")
        page.reload()
        page.wait_for_function("window.previewTest !== undefined")
        page.evaluate("previewTest.open('models/perpheads/weapons/c_m4a1.mdl')")
        page.evaluate("previewTest.showSkins()")
        row = page.locator(f'.skin-item[data-draft-id="{draft_id}"]')
        row.focus()
        row.press("Enter")
        page.wait_for_function("previewTest.draftState().status.includes('restored')")
        assert page.evaluate("previewTest.draftState().edits.length") == 2
        page.evaluate("previewTest.enableInstall()")
        row = page.locator(f'.skin-item[data-draft-id="{draft_id}"]')
        assert "Not installed" in row.inner_text()
        if args.screenshot:
            page.evaluate("previewTest.setView('front')")
            page.wait_for_timeout(600)
            page.screenshot(path=str(args.screenshot.with_stem(args.screenshot.stem + "-saved-draft")))
        page.locator("#install-draft").click()
        page.wait_for_function("draftMock.installed.size === 1")
        page.wait_for_function("""id => document.querySelector('[data-draft-id="' + id + '"] .skin-status')
            .textContent.endsWith('Installed')""", arg=draft_id)
        page.on("dialog", lambda dialog: dialog.accept())
        page.locator("#uninstall-draft").click()
        page.wait_for_function("draftMock.installed.size === 0")
        page.wait_for_function("""id => document.querySelector('[data-draft-id="' + id + '"] .skin-status')
            .textContent.endsWith('Not installed')""", arg=draft_id)
        assert page.evaluate("draftMock.saved.size") == 1
        page.locator("#install-draft").click()
        page.wait_for_function("draftMock.installed.size === 1")
        page.evaluate("draftMock.deferDelete=true")
        page.locator("#delete-draft").click()
        page.wait_for_function("typeof draftMock.resolveDelete==='function'")
        saves_before_delete = page.evaluate("draftMock.calls.filter(command=>command==='save_draft').length")
        page.evaluate("""()=>{
          window.saveDuringDelete=previewTest.saveDraft();
          window.navigationAfterDelete=previewTest.open('models/perpheads/weapons/c_m4a1.mdl');
        }""")
        assert page.evaluate("draftMock.calls.filter(command=>command==='save_draft').length") == saves_before_delete
        page.evaluate("draftMock.resolveDelete();draftMock.deferDelete=false")
        page.evaluate("Promise.all([window.saveDuringDelete,window.navigationAfterDelete])")
        page.wait_for_function("draftMock.saved.size === 0")
        assert page.locator("#model-name").inner_text() == "c_m4a1"
        assert not page.evaluate("previewTest.draftState().id")
        assert "Cannot leave" not in page.locator("#toast").inner_text()
        assert not page.locator("#draft-actions").is_visible()
        assert page.evaluate("draftMock.installed.size") == 0, "Whole-skin delete left installed attachment entries"
        print("PASS: autosave multiple textures, rename, slots, reopen after reload, no installation, and disk-failure retry")
        print("PASS: installed/not-installed library status, uninstall keeps draft, whole-skin delete removes installation")
        print("PASS: draft-card thumbnails include saved edits without changing the editor; card click/Enter opens inspector actions")

        page.evaluate("draftMock.installed.clear()")
        page.evaluate("previewTest.open('models/perpheads/weapons/attachments/rifle_eotech.mdl')")
        page.evaluate("previewTest.setupJarheads()")
        alpha_png = page.evaluate("previewTest.uploadAlphaPng()")
        page.locator("#png-input").set_input_files({
            "name": "alpha.png", "mimeType": "image/png",
            "buffer": base64.b64decode(alpha_png.split(",")[1]),
        })
        page.wait_for_function("previewTest.draftState().status.includes('Saved locally')")
        alpha_id = page.evaluate("previewTest.draftState().id")
        assert page.locator("#export-vtf").is_enabled()
        page.locator("#export-vtf").click()
        page.locator("#alpha-dialog").wait_for(state="visible")
        if args.screenshot:
            page.locator("#alpha-dialog").screenshot(path=str(args.screenshot.with_name("alpha-install-warning.png")))
        page.get_by_role("button", name="Cancel", exact=True).click()
        page.wait_for_function("!document.getElementById('alpha-dialog').open")
        assert page.evaluate("draftMock.exports.length") == 0
        page.locator("#export-vtf").click()
        page.locator("#alpha-keep").click()
        page.wait_for_function("draftMock.exports.length === 1")
        assert not page.evaluate("draftMock.exports[0][0].stripAlpha")
        assert page.evaluate("draftMock.exports[0][0].vmtPath").startswith("materials/models/jarheads/")
        page.locator("#install-all").click()
        page.locator("#alpha-remove").click()
        page.wait_for_function("draftMock.exports.length === 2 && previewTest.draftState().status.includes('Saved locally')")
        assert page.evaluate("draftMock.exports[1][0].stripAlpha")
        assert page.evaluate("draftMock.exports[1][0].pngBase64") == alpha_png
        copies_before = page.evaluate("draftMock.copies.length")
        page.locator("#copy-png").click()
        page.wait_for_function("count => draftMock.copies.length === count + 1", arg=copies_before)
        assert page.evaluate("draftMock.copies.at(-1)") == alpha_png
        assert page.evaluate("draftMock.copyAlphaChoices.at(-1)") is True
        print("PASS: original/edited texture copy requests opaque RGB, UV copy retains transparency, clipboard errors are visible")
        page.wait_for_function("id => draftMock.saved.get(id).textures[0].stripAlpha === true", arg=alpha_id)
        assert page.evaluate("id => draftMock.saved.get(id).textures[0].png", alpha_id) == alpha_png
        page.evaluate("previewTest.showSkins()")
        row = page.locator(f'.skin-item[data-draft-id="{alpha_id}"]')
        page.locator("#install-draft").click()
        page.locator("#alpha-dialog").wait_for(state="visible")
        assert page.locator("#alpha-remove").evaluate("button => button === document.activeElement")
        page.locator("#alpha-keep").click()
        page.wait_for_function("id => draftMock.installed.has(id)", arg=alpha_id)
        assert not page.evaluate("id => draftMock.saved.get(id).textures[0].stripAlpha", alpha_id)
        assert page.evaluate("id => draftMock.saved.get(id).textures[0].png", alpha_id) == alpha_png
        assert not page.evaluate("""previewTest.supportedMaterial({
          vmtPath:'materials/models/jarheads/weapons/attachments/rifle_sights/reticles/eotech.vmt',
          vtfPath:'materials/models/jarheads/weapons/attachments/rifle_sights/reticles/eotech.vtf'
        }, 'models/perpheads/weapons/attachments/rifle_eotech.mdl')""")
        page.evaluate("previewTest.open('models/perpheads/weapons/c_ak101.mdl')")
        page.evaluate("previewTest.setSlotAttachment('SLOT_STOCK', 'stock_ak101')")
        page.evaluate("previewTest.setupSharedTexture()")
        shared_alpha_png = page.evaluate("previewTest.uploadAlphaPng()")
        page.locator("#png-input").set_input_files({
            "name": "shared-alpha.png", "mimeType": "image/png",
            "buffer": base64.b64decode(shared_alpha_png.split(",")[1]),
        })
        page.wait_for_function("previewTest.draftState().status.includes('Saved locally')")
        page.locator("#install-all").click()
        page.locator("#alpha-remove").click()
        page.wait_for_function("draftMock.exports.length === 3 && !document.getElementById('alpha-dialog').open")
        shared_exports = page.evaluate("draftMock.exports[2]")
        assert len(shared_exports) == 2
        assert all(item["stripAlpha"] and item["pngBase64"] == shared_alpha_png for item in shared_exports)
        assert len({item["materialPath"] for item in shared_exports}) == 1
        print("PASS: Jarheads installs, alpha Keep/Remove/Cancel on active/all/saved installs, original draft preservation, reticle exclusion")
        print("PASS: shared VTF installation asks once and applies the same alpha choice to both materials")
        page.evaluate("previewTest.showSkins()")
        assert page.locator("#import-skin").is_visible()
        share_id = page.evaluate("previewTest.draftState().id")
        share_row = page.locator(f'.skin-item[data-draft-id="{share_id}"]')
        page.locator("#export-draft").click()
        page.wait_for_function("draftMock.packageExports?.length === 1")
        exported = page.evaluate("draftMock.packageExports[0]")
        assert exported["command"] == "export_draft" and exported["id"] == share_id
        assert len(exported["draft"]["textures"]) == 2
        assert all(texture["stripAlpha"] for texture in exported["draft"]["textures"])
        package_state = page.evaluate("previewTest.draftState()")
        installs_before_share = page.evaluate("draftMock.calls.filter(command => command === 'install_draft').length")
        page.evaluate("draftMock.cancelPackageExport = true")
        page.locator("#export-draft").click()
        page.wait_for_function("draftMock.packageExports.length === 2")
        page.evaluate("draftMock.cancelPackageExport = false; draftMock.failPackageExport = true")
        page.locator("#export-draft").click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('Simulated package export failure')")
        page.evaluate("draftMock.failPackageExport = false; draftMock.failSave = true")
        page.locator("#export-draft").click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('Simulated disk failure')")
        assert page.evaluate("draftMock.packageExports.length") == 2
        page.evaluate("draftMock.failSave = false")
        saved_before_import = page.evaluate("draftMock.saved.size")
        page.locator("#import-skin").click()
        page.wait_for_function("!document.getElementById('import-skin').disabled")
        assert page.evaluate("draftMock.saved.size") == saved_before_import
        page.evaluate("draftMock.failPackageImport = true")
        page.locator("#import-skin").click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('Invalid skin package')")
        assert page.evaluate("draftMock.saved.size") == saved_before_import
        page.evaluate("draftMock.failPackageImport = false; draftMock.packageImport = draftMock.packageExports[0].draft")
        page.locator("#import-skin").click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('Nothing was installed')")
        assert page.evaluate("draftMock.saved.size") == saved_before_import + 1
        imported_id = page.evaluate("[...draftMock.saved.keys()].at(-1)")
        assert imported_id != share_id
        assert page.locator(f'button.skin-item[data-draft-id="{imported_id}"]').is_visible()
        page.evaluate("""() => {
          draftMock.packageImport = {...draftMock.packageExports[0].draft, name:'Shared backup', textures:[],
            archivedFiles:[{root:'vmt', relative:'models/weapons/v_models/test.vmt', bytes:'dGVzdA=='}]};
        }""")
        page.locator("#import-skin").click()
        page.wait_for_function("count => draftMock.saved.size === count + 2", arg=saved_before_import)
        backup_id = page.evaluate("[...draftMock.saved.keys()].at(-1)")
        backup_row = page.locator(f'.skin-item[data-draft-id="{backup_id}"]')
        assert backup_row.get_by_role("button", name="Open draft", exact=True).count() == 0
        backup_row.get_by_role("button", name="Export skin", exact=True).click()
        page.wait_for_function("draftMock.packageExports.length === 3")
        assert page.evaluate("draftMock.packageExports.at(-1).draft.archivedFiles.length") == 1
        page.evaluate("""() => {
          draftMock.customInventory = [{id:'vmt:installed.vmt', name:'Installed only', vmtPath:'installed.vmt', hasVtf:true}];
          return previewTest.showSkins();
        }""")
        page.locator(".skin-item").filter(has_text="Installed only").get_by_role("button", name="Export skin", exact=True).click()
        page.wait_for_function("draftMock.packageExports.length === 4")
        assert page.evaluate("draftMock.packageExports.at(-1).command") == "export_custom_skin"
        assert page.evaluate("draftMock.calls.filter(command => command === 'install_draft').length") == installs_before_share
        assert page.evaluate("previewTest.draftState().id") == package_state["id"]
        assert page.evaluate("previewTest.draftState().edits") == package_state["edits"]
        installed_row = page.locator(".skin-item").filter(has_text="Installed only")
        installed_row.get_by_role("button", name="Open as editable draft", exact=True).click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('No compatible model')")
        assert not page.evaluate("draftMock.calls.includes('edit_installed_skin')")
        page.evaluate("""() => {
          draftMock.matchingModels=[draftMock.packageExports[0].draft.modelPath,'models/perpheads/weapons/c_ak101.mdl'];
          draftMock.installedImport=draftMock.packageExports[0].draft;
        }""")
        installed_row.get_by_role("button", name="Open as editable draft", exact=True).click()
        page.locator("#installed-draft-dialog").wait_for(state="visible")
        assert page.locator("#installed-draft-model option").count() == 2
        page.locator("#installed-draft-dialog button[value=cancel]").click()
        assert not page.evaluate("draftMock.calls.includes('edit_installed_skin')")
        page.evaluate("draftMock.failInstalledImport=true")
        installed_row.get_by_role("button", name="Open as editable draft", exact=True).click()
        page.locator("#installed-draft-dialog button[value=open]").click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('Installed texture could not be decoded')")
        assert page.evaluate("previewTest.draftState().id") == package_state["id"]
        page.evaluate("draftMock.failInstalledImport=false")
        installed_row.get_by_role("button", name="Open as editable draft", exact=True).click()
        page.locator("#installed-draft-dialog button[value=open]").click()
        page.wait_for_function("previewTest.draftState().id === 'installed-edit-copy'")
        assert page.evaluate("draftMock.importedModel") == page.evaluate("draftMock.matchingModels[0]")
        assert page.evaluate("draftMock.calls.filter(command => command === 'install_draft').length") == installs_before_share
        assert page.locator('button.skin-item[data-draft-id="installed-edit-copy"]').is_visible()
        page.evaluate("previewTest.setupSharedTexture()")
        print("PASS: installed skin editing requires matching-model confirmation, supports cancellation/errors, and never installs automatically")
        page.locator("[data-library='models']").click()
        assert page.locator("#import-skin").is_hidden()
        print("PASS: whole-skin draft/backup/installed export, import/cancel/errors, save-before-export and no automatic installation")
        page.evaluate("draftMock.updateAvailable = '0.3.0'")
        page.locator("#updates-button").click()
        page.locator("#check-updates").click()
        page.wait_for_function("document.getElementById('update-status').textContent.includes('0.3.0')")
        assert page.locator("#install-update").is_enabled()
        if args.screenshot:
            page.screenshot(path=str(args.screenshot.with_name("updates-panel.png")))
        page.evaluate("draftMock.failSave = true")
        attempts = page.evaluate("draftMock.calls.filter(command => command === 'install_update').length")
        page.locator("#install-update").click()
        page.wait_for_function("document.getElementById('update-summary').textContent.includes('Simulated disk failure')")
        assert page.evaluate("draftMock.calls.filter(command => command === 'install_update').length") == attempts
        assert not page.locator("main").evaluate("main => main.inert")
        page.evaluate("draftMock.failSave = false")
        page.locator("#install-update").click()
        page.wait_for_function("document.getElementById('update-summary').textContent.includes('Simulated installer failure')")
        assert page.evaluate("draftMock.calls.filter(command => command === 'install_update').length") == attempts + 1
        assert not page.locator("main").evaluate("main => main.inert")
        calls = page.evaluate("draftMock.calls")
        assert max(i for i, command in enumerate(calls[:calls.index("install_update")]) if command == "save_draft") < calls.index("install_update")
        page.evaluate("draftMock.failUpdateCheck = true")
        page.locator("#check-updates").click()
        page.wait_for_function("document.getElementById('update-status').textContent === 'Update check failed'")
        assert "Update check failed" == page.locator("#update-status").inner_text()
        page.locator("#close-updates").click()
        print("PASS: automatic update check, logo placement, safe release log, confirmation, save-before-install and explicit failures")

        page.evaluate("previewTest.removeSharedOwnerEdit()")
        page.evaluate("previewTest.selectTexture('materials/models/weapons/v_models/test/shared.vtf')")
        export_count = page.evaluate("draftMock.exports.length")
        page.locator("#install-all").click()
        page.locator("#alpha-remove").click()
        page.wait_for_function("count => draftMock.exports.length === count + 1", arg=export_count)
        shared = page.evaluate("draftMock.exports.at(-1)")
        assert len(shared) == 2, "Install all skipped a material that previewed an inherited shared edit"
        assert len({texture["vmtPath"] for texture in shared}) == 2
        assert len({texture["pngBase64"] for texture in shared}) == 1
        key = page.evaluate("previewTest.removeSharedOwnerEdit()")
        page.evaluate("previewTest.saveDraft()")
        assert page.evaluate("key => draftMock.saved.get(previewTest.draftState().id).textures.some(texture => texture.key === key)", key)
        print("PASS: inherited shared texture previews install every owner and persist material mappings to drafts")

        page.evaluate("""async () => {
          const draft = draftMock.saved.get(previewTest.draftState().id);
          const before = draftMock.calls.length;
          await previewTest.stageWorkshopDraft(draft);
          if (draftMock.stagedWorkshop) throw Error('Signed-out user staged a publishing form');
          if (draftMock.calls.slice(before).includes('get_draft')) throw Error('Signed-out publish opened the draft');
          draftMock.workshopConnected = true;
          await previewTest.stageWorkshopDraft(draft);
          if (!draftMock.stagedWorkshop || draftMock.stagedWorkshop.id !== draft.id) throw Error('Workshop preview not staged');
          const image = new Image(); image.src = draftMock.stagedWorkshop.preview; await image.decode();
          if (!image.width || Math.max(image.width, image.height) > 1024) throw Error('Preview dimensions exceed the limit');
          const captured = draftMock.stagedWorkshop;
          draftMock.failSave = true;
          await previewTest.stageWorkshopDraft(draft);
          draftMock.failSave = false;
          if (draftMock.stagedWorkshop !== captured) throw Error('Failed save staged an unreviewed preview');
        }""")
        print("PASS: Workshop captures a bounded preview only after opening and saving successfully")
        page.locator("#workshop-dialog").wait_for(state="visible")
        page.frame_locator("#workshop-frame").locator("h1").wait_for()
        page.locator("#close-workshop").click()
        assert not page.locator("#workshop-dialog").is_visible()
        page.locator("#open-workshop").click()
        assert page.locator("#workshop-dialog").is_visible()
        page.locator("#close-workshop").click()
        print("PASS: Workshop opens inside the editor and returns without creating another window")
        page.locator("#choose-folder").click()
        page.wait_for_function("() => !document.querySelector('#refresh-models').disabled")
        scans = page.evaluate("draftMock.calls.filter(command => command === 'scan_content').length")
        page.locator("#refresh-models").click()
        page.wait_for_function("count => draftMock.calls.filter(command => command === 'scan_content').length === count + 1", arg=scans)
        page.wait_for_function("() => !document.querySelector('#refresh-models').disabled")
        print("PASS: Refresh models rescans the selected folder without opening the folder picker")

        variant_gun = next(path for path, weapon in data["weapons"].items() if "suppressor_54539" in weapon["compatible"])
        prior_install_count = page.evaluate("draftMock.exports.length")
        suppressor = data["attachments"]["suppressor_54539"]["model"]
        page.evaluate("path => previewTest.open(path)", variant_gun)
        page.evaluate("previewTest.setSlotAttachment('SLOT_SILENCER','suppressor_54539')")
        page.evaluate("previewTest.setupVariantFixture()")
        page.evaluate("path => previewTest.selectPart(path)", variant_gun)
        main_png = page.evaluate("previewTest.variantPng('#44aa55')")
        initial_variant_count = page.evaluate("draftMock.variants.size")
        page.locator("#png-input").set_input_files({"name":"body-green.png","mimeType":"image/png","buffer":base64.b64decode(main_png.split(",")[1])})
        page.wait_for_function("previewTest.draftState().status.includes('Saved locally')")
        assert page.evaluate("draftMock.variants.size") == initial_variant_count, "Draft import automatically added a library variant"
        page.locator("#save-texture-variant").click()
        page.locator("#variant-name").fill("body-green")
        page.locator('#save-variant-dialog button[value="save"]').click()
        page.wait_for_function("count=>draftMock.variants.size===count+1",arg=initial_variant_count)
        body_edit = page.evaluate("previewTest.variantEdits()")
        page.evaluate("path => previewTest.selectPart(path)", suppressor)
        red_png = page.evaluate("previewTest.variantPng('#cc2233')")
        blue_png = page.evaluate("previewTest.variantPng('#2255dd')")
        for name, image in (("suppressor-red", red_png), ("suppressor-blue", blue_png)):
            before_count = page.evaluate("draftMock.variants.size")
            page.locator("#png-input").set_input_files({"name":name+".png","mimeType":"image/png","buffer":base64.b64decode(image.split(",")[1])})
            page.wait_for_function("png=>previewTest.variantEdits().some(edit=>edit.png===png)&&previewTest.draftState().status.includes('Saved locally')",arg=image)
            assert page.evaluate("draftMock.variants.size") == before_count, "Draft import automatically added a library variant"
            page.locator("#save-texture-variant").click()
            page.locator("#variant-name").fill(name)
            page.locator('#save-variant-dialog button[value="save"]').click()
            page.wait_for_function("count=>draftMock.variants.size===count+1",arg=before_count)
        red_id = page.evaluate("[...draftMock.variants.values()].find(item=>item.name==='suppressor-red').id")
        blue_id = page.evaluate("[...draftMock.variants.values()].find(item=>item.name==='suppressor-blue').id")
        body_variant = page.evaluate("[...draftMock.variants.values()].find(item=>item.name==='body-green').id")
        unchanged = page.evaluate("previewTest.variantEdits()")
        page.evaluate("id=>previewTest.applyTextureVariant(id)",body_variant)
        assert page.evaluate("previewTest.variantEdits()") == unchanged, "An unrelated material variant was accepted"
        assert "matching VMT / VTF" in page.locator("#toast").inner_text()
        assert set(page.locator("#variant-select option").all_text_contents()) == {"Original / default","suppressor-red","suppressor-blue"}
        page.locator("#variant-select").select_option(red_id)
        page.wait_for_function("png=>previewTest.variantEdits().some(edit=>edit.png===png)",arg=red_png)
        assert all(item in page.evaluate("previewTest.variantEdits()") for item in body_edit), "Changing suppressor changed the gun body"
        page.locator("#variant-select").select_option("original")
        page.wait_for_function("document.getElementById('variant-select').value==='original'")
        assert all(item in page.evaluate("previewTest.variantEdits()") for item in body_edit)
        page.locator("#variant-select").select_option(blue_id)
        page.wait_for_function("png=>previewTest.variantEdits().some(edit=>edit.png===png)",arg=blue_png)
        page.locator('[data-library="textures"]').click()
        page.locator("#search").fill("suppressor")
        assert page.locator(".texture-library-card").count() == 2
        assert page.locator(".texture-library-card .model-thumb").evaluate_all("""thumbs=>thumbs.every(thumb=>{
          const box=thumb.getBoundingClientRect(), image=thumb.querySelector('img').getBoundingClientRect();
          return image.bottom<=box.bottom+1 && image.top>=box.top-1;
        })"""), "Texture thumbnails overflowed into card text"
        page.locator(f'[data-variant-id="{red_id}"] .texture-card-open').click()
        assert page.locator("#texture-variant-dialog").is_visible()
        assert "basetexture" in page.locator("#texture-variant-vmt").inner_text()
        page.locator('#texture-variant-dialog button[value="cancel"]').click()
        count = page.evaluate("draftMock.variants.size")
        page.locator("#save-texture-variant").click()
        page.locator('#save-variant-dialog button[value="cancel"]').click()
        assert page.evaluate("draftMock.variants.size") == count
        page.evaluate("draftMock.failVariantSave=true")
        before_count = page.evaluate("draftMock.variants.size")
        page.locator("#png-input").set_input_files({"name":"failed.png","mimeType":"image/png","buffer":base64.b64decode(red_png.split(",")[1])})
        page.wait_for_function("png=>previewTest.variantEdits().some(edit=>edit.png===png)&&previewTest.draftState().status.includes('Saved locally')",arg=red_png)
        assert page.evaluate("draftMock.variants.size") == before_count, "Import touched the texture library"
        prior = page.evaluate("previewTest.variantEdits()")
        page.locator("#save-texture-variant").click()
        page.locator("#variant-name").fill("failed-save")
        page.locator('#save-variant-dialog button[value="save"]').click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('texture-library disk failure')")
        assert page.evaluate("previewTest.variantEdits()") == prior, "A failed manual library save changed the draft"
        page.evaluate("draftMock.failVariantSave=false")
        page.locator("#png-input").set_input_files([
            {"name":"wrong.vmt","mimeType":"text/plain","buffer":b'UnlitGeneric\\n{\\n$basetexture "wrong"\\n}'},
            {"name":"other.vtf","mimeType":"application/octet-stream","buffer":b"bad"},
        ])
        page.wait_for_function("document.getElementById('toast').textContent.includes('VTF named by the VMT')")
        assert page.evaluate("previewTest.variantEdits()") == prior
        vmt = 'UnlitGeneric\n{\n$basetexture "red_custom"\n$translucent 1\n}'
        page.evaluate("png=>draftMock.decodedImage=png",red_png)
        page.locator("#png-input").set_input_files([
            {"name":"red.vmt","mimeType":"text/plain","buffer":vmt.encode()},
            {"name":"red_custom.vtf","mimeType":"application/octet-stream","buffer":b"VTF mock"},
        ])
        page.wait_for_function("previewTest.variantEdits().some(edit=>edit.vmtText?.includes('UnlitGeneric'))")
        page.wait_for_function("previewTest.draftState().status.includes('Saved locally')")
        assert page.evaluate("previewTest.mountedMaterials().some(material=>material.isMeshBasicMaterial && material.transparent)"), "Saved VMT flags were not applied to preview"
        assert all(item in page.evaluate("previewTest.variantEdits()") for item in body_edit)
        saved_id = page.evaluate("previewTest.draftState().id")
        assert page.evaluate("draftMock.exports.length") == prior_install_count, "Variant selection installed a skin"
        count = page.evaluate("draftMock.variants.size")
        for _ in range(2):
            page.evaluate("id=>previewTest.applyTextureVariant(id)",red_id)
            page.evaluate("previewTest.saveDraft()")
        assert page.evaluate("draftMock.variants.size") == count, "Reusing a saved texture created another variant"
        masked_png = page.evaluate("previewTest.uploadAlphaPng()")
        masked_id = page.evaluate("""async ([id,png])=>{
          const original=draftMock.variants.get(id);
          const variant=await __TAURI__.core.invoke('save_texture_variant',{texture:{
            name:original.name,vmtPath:original.vmtPath,vtfPath:original.vtfPath,vmtText:original.vmtText,png}});
          await previewTest.refreshSkins();
          return variant.id;
        }""",[red_id,masked_png])
        page.locator(f'[data-variant-id="{blue_id}"]').get_by_role("button",name="Delete",exact=True).click()
        page.wait_for_function("id=>!draftMock.variants.has(id)",arg=blue_id)
        assert page.evaluate("id=>draftMock.saved.has(id)",saved_id)
        labels = page.locator("#variant-select option").all_text_contents()
        assert "suppressor-red · Alpha mask" in labels and "suppressor-red · Opaque" in labels, labels
        assert len(labels) == len(set(labels)), "Different named variants cannot share indistinguishable labels"
        page.locator(f'[data-variant-id="{masked_id}"] .texture-card-open').click()
        page.wait_for_function("document.getElementById('texture-variant-preview').complete")
        assert page.locator("#texture-variant-title").inner_text() == "suppressor-red · Alpha mask"
        assert page.locator("#texture-variant-preview").evaluate("""image=>{
          const canvas=document.createElement('canvas');canvas.width=image.naturalWidth;canvas.height=image.naturalHeight;
          const context=canvas.getContext('2d');context.drawImage(image,0,0);
          return context.getImageData(0,0,canvas.width,canvas.height).data.every((value,index)=>index%4!==3||value===255);
        }"""), "VMT/VTF viewer used masked source alpha instead of opaque RGB preview"
        assert page.locator(f'[data-variant-id="{masked_id}"] .model-thumb img').get_attribute("src") == page.evaluate("id=>draftMock.variants.get(id).preview",masked_id)
        assert page.evaluate("id=>draftMock.variants.get(id).png",masked_id) == masked_png, "Preview changed source alpha"
        page.locator('#texture-variant-dialog button[value="cancel"]').click()
        page.reload()
        page.wait_for_function("document.getElementById('texture-count').textContent !== '0'")
        page.locator('[data-library="textures"]').click()
        assert page.locator(f'[data-variant-id="{red_id}"]').count() == 1
        assert page.locator(f'[data-variant-id="{blue_id}"]').count() == 0
        assert page.evaluate("id=>draftMock.saved.has(id)",saved_id), "Deleting a variant removed the draft"
        assert not page.evaluate("draftMock.calls.includes('export_textures')"), "Variant selection installed a skin"
        print("PASS: saved VMT/VTF library, material-matched suppressor variants, default restore, unchanged gun body, raw-pair validation/shaders, errors, persistence and independent deletion")

        page.evaluate("previewTest.open('models/perpheads/weapons/c_ak101.mdl')")
        page.evaluate("previewTest.setSlotAttachment('SLOT_STOCK','stock_ak101')")
        fixture = page.evaluate("previewTest.setupSharedComposition()")
        red_png = page.evaluate("previewTest.compositionPng('#cc2233','#cc2233')")
        blue_png = page.evaluate("previewTest.compositionPng('#2255dd','#2255dd')")
        combined_png = page.evaluate("previewTest.compositionPng('#cc2233','#2255dd')")
        source_ids = page.evaluate("""async ([material,red,blue])=>{
          const result=[];
          for(const [name,png] of [['Component red',red],['Component blue',blue]]) {
            const saved=await __TAURI__.core.invoke('save_texture_variant',{texture:{
              name,png,vmtPath:material.vmtPath,vtfPath:material.vtfPath,
              vmtText:'UnlitGeneric\\n{\\n$basetexture "models/weapons/v_models/test/shared"\\n}'}});
            result.push(saved.id);
          }
          await previewTest.refreshSkins();return result;
        }""",[fixture["material"],red_png,blue_png])
        page.locator("#variant-select").select_option(source_ids[1])
        page.wait_for_function("previewTest.draftState().status.includes('Saved locally')")
        page.evaluate("previewTest.showComponentOverview()")
        card = page.locator(".component-card").filter(has_text="Stock — AK-101 Stock").first
        assert card.locator("select").input_value() == source_ids[1], "Shared texture variant selection was shown as Original / default"
        assert "Applied: Component blue" in page.locator("#shared-texture-cards").inner_text()
        page.evaluate("path=>previewTest.editComponent(path)",fixture["gun"])
        page.evaluate("previewTest.selectActiveShared()")
        page.locator("#variant-select").select_option("original")
        page.wait_for_function("previewTest.draftState().status.includes('Saved locally')")
        page.evaluate("previewTest.showComponentOverview()")
        before = page.evaluate("previewTest.draftState()")
        before_installs = page.evaluate("draftMock.exports.length")
        assert page.locator("#component-overview").is_visible()
        assert not page.locator("#texture-editor").is_visible()
        assert page.locator("#shared-texture-dialog").count() == 0
        assert page.locator("#shared-texture-overview").is_visible()
        assert page.locator(".shared-texture-card").count() == 1
        assert page.evaluate("""()=>{
          const shared=document.getElementById('shared-texture-overview'), components=document.getElementById('component-cards');
          return !!(shared.compareDocumentPosition(components)&Node.DOCUMENT_POSITION_FOLLOWING);
        }"""), "Shared textures were not above the component list"
        page.locator(".shared-texture-card .component-card-open").click()
        assert not page.evaluate("previewTest.componentState().mode")
        assert len({item["path"] for item in page.evaluate("previewTest.highlightedMeshes()")}) == 2
        assert page.locator("#component-name").inner_text().startswith("Shared texture ·")
        page.locator("#show-component-overview").click()
        card = page.locator(".component-card").filter(has_text="Stock — AK-101 Stock").first
        page.wait_for_function("""[...document.querySelectorAll('.component-card img')]
          .every(image=>image.complete && image.naturalWidth===220)""")
        card.locator("select").select_option(source_ids[1])
        page.wait_for_function("previewTest.draftState().status.includes('Saved locally')")
        assert not page.evaluate("previewTest.componentState().selected || previewTest.componentState().mode"), "Changing a component dropdown opened the component editor"
        assert page.locator("#component-overview").is_visible() and page.locator("#texture-editor").is_hidden()
        assert next(item["png"] for item in page.evaluate("previewTest.componentState().edits") if item["partPath"]==fixture["stock"]) == blue_png
        card.locator(".component-card-open").click()
        assert page.locator("#component-name").inner_text() == "Stock — AK-101 Stock · Shared wood"
        assert page.evaluate("previewTest.componentState().mode")
        assert page.locator("#uv-layers").is_hidden()
        assert page.locator("#uv-layers input").count() == 0
        assert page.locator("#show-uv").is_visible()
        assert set(page.evaluate("previewTest.uvStrokes()")) == {"#62c4ff"}, "Component UV overlay included another component"
        page.evaluate("png=>{draftMock.combinedPng=png}",combined_png)
        page.locator("#variant-select").select_option(source_ids[1])
        page.wait_for_function("previewTest.componentState().edits.length===2 && previewTest.draftState().status.includes('Saved locally')")
        components = page.evaluate("previewTest.componentState().edits")
        assert next(item["png"] for item in components if item["partPath"]==fixture["stock"]) == blue_png
        assert next(item["png"] for item in components if item["partPath"]==fixture["gun"]) == fixture["material"]["image"]
        page.evaluate("path=>previewTest.editComponent(path)",fixture["gun"])
        page.locator("#variant-select").select_option(source_ids[0])
        page.wait_for_function("([path,png])=>previewTest.componentState().edits.some(edit=>edit.partPath===path&&edit.png===png)",arg=[fixture["gun"],red_png])
        page.wait_for_function("previewTest.draftState().status.includes('Saved locally')")
        state = page.evaluate("previewTest.draftState()")
        assert all(edit["png"] == combined_png for edit in state["edits"])
        draft = page.evaluate("id=>draftMock.saved.get(id)",state["id"])
        page.evaluate("previewTest.saveDraft()")
        preview_version = page.evaluate("previewTest.draftPreviewVersion()")
        scans_before = page.evaluate("draftMock.calls.filter(command=>['list_custom_skins','list_texture_variants'].includes(command)).length")
        page.evaluate("previewTest.saveDraft()")
        assert page.evaluate("previewTest.draftPreviewVersion()") == preview_version, "Unchanged autosave invalidated the draft thumbnail"
        assert page.evaluate("draftMock.calls.filter(command=>['list_custom_skins','list_texture_variants'].includes(command)).length") == scans_before, "Autosave rescanned installed skins or the texture library"
        assert len(draft["textures"]) == 1, "The same shared VMT must install once"
        assert draft["textures"][0]["vmtText"] == fixture["material"]["vmtText"], "Component VMT shader settings replaced the shared material"
        assert {item["partPath"]:item["png"] for item in draft["componentTextures"]} == {
            fixture["gun"]:red_png, fixture["stock"]:blue_png}
        assert page.evaluate("draftMock.exports.length") == before_installs, "Combining installed automatically"
        page.evaluate("previewTest.rebuild()")
        page.wait_for_function("png=>previewTest.textureEdits().find(edit=>edit.path.endsWith('/stock_ak101.mdl')).source===png",arg=blue_png)
        assert page.evaluate("previewTest.textureEdits().find(edit=>edit.path.endsWith('/c_ak101.mdl')).source") == red_png
        before_component_race = page.evaluate("previewTest.componentState().edits")
        page.evaluate("draftMock.deferClipboard=true")
        page.locator("#import-clipboard").click()
        page.wait_for_function("typeof draftMock.resolveClipboard==='function'")
        page.evaluate("path=>previewTest.editComponent(path)",fixture["stock"])
        page.evaluate("png=>{draftMock.deferClipboard=false;draftMock.resolveClipboard(png)}",red_png)
        page.wait_for_function("document.getElementById('toast').textContent.includes('component changed during import')")
        assert page.evaluate("previewTest.componentState().edits") == before_component_race
        page.evaluate("id=>previewTest.openDraft({id})",state["id"])
        assert len(page.evaluate("previewTest.componentState().edits")) == 2
        assert page.locator("#component-overview").is_visible()
        restored_version = page.evaluate("previewTest.draftPreviewVersion()")
        assert page.locator("#component-cards .component-card").filter(has_text="Stock — AK-101 Stock").locator("select").input_value() == source_ids[1], "Reopened component did not identify its saved library texture"
        assert "Combined component textures" in page.locator("#shared-texture-cards").inner_text()
        page.evaluate("previewTest.saveDraft()")
        assert page.evaluate("previewTest.draftPreviewVersion()") == restored_version, "First unchanged save after reopening regenerated the preview"
        assert page.evaluate("""id=>{
          const draft=structuredClone(draftMock.saved.get(id)), reordered=structuredClone(draft);
          reordered.textures.reverse();reordered.componentTextures.reverse();
          reordered.name='Renamed without visual changes';reordered.activePartPath='another selection';
          return previewTest.sameDraftPreview(draft,reordered);
        }""",state["id"]), "Record order or metadata changes invalidated an unchanged preview"
        page.locator('[data-library="skins"]').click()
        page.locator(f'[data-draft-id="{state["id"]}"]').scroll_into_view_if_needed()
        page.wait_for_function("id=>!!document.querySelector(`[data-draft-id='${id}'] .draft-thumb img`)",arg=state["id"])
        page.evaluate("id=>{window.retainedPreview=document.querySelector(`[data-draft-id='${id}'] .draft-thumb img`)}",state["id"])
        page.evaluate("previewTest.refreshSkins()")
        assert page.evaluate("id=>document.querySelector(`[data-draft-id='${id}'] .draft-thumb img`)===window.retainedPreview",state["id"]), "Unchanged library refresh replaced the preview image"
        page.evaluate("path=>previewTest.editComponent(path)",fixture["stock"])
        extra_png = page.evaluate("previewTest.compositionPng('#88cc11','#88cc11')")
        before_library_export = page.evaluate("draftMock.variants.size")
        page.locator("#png-input").set_input_files({"name":"component-green.png","mimeType":"image/png","buffer":base64.b64decode(extra_png.split(",")[1])})
        page.wait_for_function("previewTest.draftState().status.includes('Saved locally')")
        assert page.evaluate("draftMock.variants.size") == before_library_export, "Component edits automatically added a library copy"
        composed_green = page.evaluate("previewTest.compositionPng('#cc2233','#88cc11')")
        page.evaluate("png=>{draftMock.combinedPng=png}",composed_green)
        components_before_export = page.evaluate("previewTest.componentState().edits")
        page.locator("#export-draft-textures").click()
        page.wait_for_function("count=>draftMock.variants.size===count+1",arg=before_library_export)
        exported = page.evaluate("([...draftMock.variants.values()]).at(-1)")
        assert exported["png"] == composed_green, "Bulk export saved a component source instead of its composed atlas"
        assert exported["vmtText"] == fixture["material"]["vmtText"]
        assert page.evaluate("previewTest.componentState().edits") == components_before_export
        page.locator("#export-draft-textures").click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('no new changed')")
        assert page.evaluate("draftMock.variants.size") == before_library_export+1, "Repeated bulk export created duplicate variants"
        composed_original = page.evaluate("previewTest.compositionPng('#cc2233','#666666')")
        page.evaluate("png=>{draftMock.combinedPng=png}",composed_original)
        page.locator("#variant-select").select_option("original")
        page.wait_for_function("previewTest.draftState().status.includes('Saved locally')")
        page.locator("#export-draft-textures").click()
        page.wait_for_function("count=>draftMock.variants.size===count+2",arg=before_library_export)
        assert page.evaluate("([...draftMock.variants.values()]).at(-1).png") == composed_original, "Library atlas omitted the restored original component"
        page.evaluate("draftMock.componentConflict=true")
        page.evaluate("path=>previewTest.editComponent(path)",fixture["stock"])
        page.locator("#variant-select").select_option(source_ids[1])
        page.wait_for_function("document.getElementById('draft-status').textContent.includes('output blocked')")
        page.locator("#export-draft-textures").click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('could not be prepared')")
        assert page.evaluate("draftMock.variants.size") == before_library_export+2, "Conflicting components exported partial library variants"
        before_output = page.evaluate("draftMock.exports.length")
        page.locator("#export-vtf").click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('conflicting component')")
        assert page.evaluate("draftMock.exports.length") == before_output
        page.evaluate("draftMock.componentConflict=false")
        page.evaluate("png=>{draftMock.combinedPng=png}",combined_png)
        page.locator("#variant-select").select_option(source_ids[0])
        page.wait_for_function("previewTest.draftState().status.includes('Saved locally') && !document.getElementById('draft-status').textContent.includes('blocked')")
        page.evaluate("previewTest.selectActiveShared()")
        assert not page.evaluate("previewTest.componentState().mode")
        assert page.locator("#uv-layers").is_visible()
        assert page.locator("#uv-layers input").count() == 2
        assert page.locator("#component-scope").is_visible()
        assert page.locator("#component-name").inner_text().startswith("Shared texture ·")
        page.wait_for_function("""() => {
          const image=document.getElementById('component-preview');
          return image.complete && image.naturalWidth===220;
        }""")
        page.locator("#export-image").click()
        page.locator("#export-format").select_option("png")
        page.locator('#export-dialog button[value="export"]').click()
        page.wait_for_function("png=>draftMock.imageExports.at(-1)?.pngBase64===png",arg=combined_png)
        page.evaluate("draftMock.clipboardImage=previewTest.compositionPng('#336633','#336633')")
        page.locator("#import-clipboard").click()
        page.wait_for_function("previewTest.componentState().edits.length===0 && previewTest.draftState().status.includes('Saved locally')")
        assert page.evaluate("id=>draftMock.saved.get(id).componentTextures.length",state["id"]) == 0
        print("PASS: illustrated right-panel overview, isolated component variants/3D preview, source persistence, shared shader, automatic atlas and blocked conflicting output")

        page.evaluate("previewTest.setupSharedComposition()")
        page.locator('[data-library="textures"]').click()
        rename_row = page.locator(".texture-library-card").filter(has_text="Component red")
        before_rename = page.evaluate("id=>structuredClone(draftMock.variants.get(id))",source_ids[0])
        before_rename_draft = page.evaluate("previewTest.draftState()")
        rename_row.get_by_role("button",name="Rename",exact=True).click()
        page.locator("#rename-variant-name").fill("Cancelled rename")
        page.locator('#rename-variant-dialog button[value="cancel"]').click()
        assert page.evaluate("id=>draftMock.variants.get(id).name",source_ids[0]) == "Component red"
        rename_row.get_by_role("button",name="Rename",exact=True).click()
        page.locator("#rename-variant-name").fill("Renamed component red")
        page.evaluate("draftMock.failVariantRename=true")
        page.locator('#rename-variant-dialog button[value="rename"]').click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('Simulated texture rename failure')")
        assert page.evaluate("id=>draftMock.variants.get(id).name",source_ids[0]) == "Component red"
        page.evaluate("draftMock.failVariantRename=false")
        rename_row.get_by_role("button",name="Rename",exact=True).click()
        page.locator("#rename-variant-name").fill("  Renamed component red  ")
        page.locator('#rename-variant-dialog button[value="rename"]').click()
        page.wait_for_function("id=>draftMock.variants.get(id).name==='Renamed component red'",arg=source_ids[0])
        after_rename = page.evaluate("id=>structuredClone(draftMock.variants.get(id))",source_ids[0])
        assert after_rename.pop("name") == "Renamed component red"
        before_rename.pop("name")
        assert after_rename == before_rename
        assert page.evaluate("previewTest.draftState()") == before_rename_draft
        assert page.locator("#variant-select").locator(f'option[value="{source_ids[0]}"]').inner_text() == "Renamed component red"
        page.reload(wait_until="networkidle")
        page.wait_for_function("window.previewTest")
        page.locator('[data-library="textures"]').click()
        assert page.locator(".texture-library-card").filter(has_text="Renamed component red").count() == 1
        print("PASS: texture-card Rename, cancel/error handling, stable identity/pixels/draft, updated selector and persistent label after reload")

        page.evaluate("previewTest.open('models/perpheads/weapons/c_ak101.mdl')")
        page.evaluate("previewTest.setupMaskedTexture()")
        resized_source = page.evaluate("""()=>{
          const canvas=document.createElement('canvas');canvas.width=2;canvas.height=1;
          const context=canvas.getContext('2d');context.fillStyle='rgba(200,100,50,0.5)';
          context.fillRect(0,0,2,1);return canvas.toDataURL();
        }""")
        page.locator("#png-input").set_input_files({"name":"resize.png","mimeType":"image/png","buffer":base64.b64decode(resized_source.split(",")[1])})
        page.locator("#resize-image-dialog").wait_for(state="visible")
        assert "distort" in page.locator("#resize-image-dialog").inner_text()
        page.locator('#resize-image-dialog button[value="resize"]').click()
        page.wait_for_function("previewTest.draftState().status.includes('Saved locally')")
        assert page.evaluate("""async ()=>{
          const image=new Image();image.src=previewTest.draftState().edits[0].png;await image.decode();
          const canvas=document.createElement('canvas');canvas.width=canvas.height=4;
          const context=canvas.getContext('2d');context.drawImage(image,0,0);
          const pixel=context.getImageData(2,2,1,1).data;
          return image.width===4&&image.height===4&&pixel[0]>190&&pixel[3]>=126&&pixel[3]<=129;
        }"""), "Resized import did not match target dimensions and retain transparency"
        page.evaluate("previewTest.holdTextureRedraws();previewTest.rebuild()")
        page.wait_for_function("window.pendingTextureRedraws()>0")
        page.wait_for_timeout(100)
        before_edited_texture_ready = page.evaluate("previewTest.previewRenderCount()")
        page.evaluate("window.releaseTextureRedraws()")
        page.wait_for_function("count=>previewTest.previewRenderCount()>count",arg=before_edited_texture_ready)
        print("PASS: mismatched imports warn, cancel without changes, and resize to exact texture dimensions with alpha")

        page.evaluate("""async ()=>{
          await previewTest.open('models/perpheads/weapons/c_ak101.mdl');
          previewTest.setupResolutionFixture();
        }""")
        higher = page.evaluate("previewTest.variantPng('rgba(70,150,220,0.5)',1024)")
        page.locator("#png-input").set_input_files({"name":"higher.png","mimeType":"image/png","buffer":base64.b64decode(higher.split(",")[1])})
        page.wait_for_function("png=>previewTest.draftState().edits.some(edit=>edit.png===png)&&previewTest.draftState().status.includes('Saved locally')",arg=higher)
        assert not page.locator("#resize-image-dialog").is_visible()
        assert page.locator("#texture-size").inner_text() == "1024 × 1024"
        assert page.locator("#texture-canvas").evaluate("canvas=>canvas.width===1024&&canvas.height===1024")
        copies = page.evaluate("draftMock.copies.length")
        page.locator("#copy-overlay").click()
        page.wait_for_function("count=>draftMock.copies.length===count+1",arg=copies)
        assert page.evaluate("""async ()=>{
          const image=new Image();image.src=draftMock.copies.at(-1);await image.decode();
          return image.width===1024&&image.height===1024;
        }""")
        page.locator("#save-texture-variant").click()
        page.locator("#variant-name").fill("High-resolution test")
        page.locator('#save-variant-dialog button[value="save"]').click()
        page.wait_for_function("[...draftMock.variants.values()].some(variant=>variant.name==='High-resolution test')")
        high_id = page.evaluate("[...draftMock.variants.values()].find(variant=>variant.name==='High-resolution test').id")
        page.locator("#variant-select").select_option("original")
        page.wait_for_function("document.getElementById('variant-select').value==='original'")
        page.locator("#variant-select").select_option(high_id)
        page.wait_for_function("png=>previewTest.draftState().edits.some(edit=>edit.png===png)&&previewTest.draftState().status.includes('Saved locally')",arg=higher)
        high_draft = page.evaluate("previewTest.draftState().id")
        assert page.evaluate("id=>previewTest.openDraft({id})",high_draft)
        page.evaluate("previewTest.selectActiveShared()")
        assert page.evaluate("png=>previewTest.draftState().edits.some(edit=>edit.png===png)",higher)
        assert page.locator("#texture-size").inner_text() == "1024 × 1024"
        assert page.locator("#variant-select").input_value() == high_id
        lower = page.evaluate("previewTest.variantPng('#3366aa',256)")
        page.evaluate("png=>draftMock.clipboardImage=png",lower)
        page.locator("#import-clipboard").click()
        page.wait_for_function("png=>previewTest.draftState().edits.some(edit=>edit.png===png)&&previewTest.draftState().status.includes('Saved locally')",arg=lower)
        assert not page.locator("#resize-image-dialog").is_visible()
        assert page.locator("#texture-size").inner_text() == "256 × 256"
        assert page.evaluate("""async ()=>{
          const draft=draftMock.saved.get(previewTest.draftState().id),image=new Image();
          image.src=draft.textures[0].png;await image.decode();
          return image.width===256&&image.height===256;
        }""")
        print("PASS: 512-to-1024 upload, 512-to-256 clipboard, actual-size UV overlay, saved variant and draft reopening preserve resolution without resizing")

        page.evaluate("""()=>{
          draftMock.installConflicts=[{
            path:'C:\\\\GMod\\\\addons\\\\skins\\\\materials\\\\test_custom.vtf',fingerprint:'reviewed-bytes',
            installedPng:previewTest.variantPng('#cc3333'),replacementPng:previewTest.variantPng('#3366aa')
          }];
        }""")
        installed_before = page.evaluate("draftMock.exports.length")
        page.locator("#export-vtf").click()
        page.locator("#overwrite-skin-dialog").wait_for(state="visible")
        assert page.locator("#cancel-skin-overwrite").evaluate("button=>button===document.activeElement")
        assert page.locator(".install-comparison img").count() == 2
        assert page.locator(".install-comparison img").evaluate_all("images=>images[0].src!==images[1].src")
        assert "Installed" in page.locator(".install-comparison h3").nth(0).inner_text()
        assert "New" in page.locator(".install-comparison h3").nth(1).inner_text()
        if args.screenshot:
            page.locator("#overwrite-skin-dialog").screenshot(path=str(args.screenshot.with_name("skin-overwrite-confirmation.png")))
        page.locator("#cancel-skin-overwrite").click()
        assert page.evaluate("draftMock.exports.length") == installed_before
        page.locator("#export-vtf").click()
        page.locator('#overwrite-skin-dialog button[value="overwrite"]').click()
        page.wait_for_function("count=>draftMock.exports.length===count+1",arg=installed_before)
        assert page.evaluate("draftMock.lastInstallOverwrite[draftMock.installConflicts[0].path]") == "reviewed-bytes"
        page.locator("#install-all").click()
        page.locator("#overwrite-skin-dialog").wait_for(state="visible")
        page.keyboard.press("Escape")
        assert page.evaluate("draftMock.exports.length") == installed_before + 1
        page.evaluate("draftMock.failOverwrite=true")
        page.locator("#install-all").click()
        page.locator('#overwrite-skin-dialog button[value="overwrite"]').click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('changed since confirmation')")
        assert page.evaluate("draftMock.exports.length") == installed_before + 1
        page.evaluate("draftMock.failOverwrite=false")
        page.locator("#install-draft").click()
        page.locator("#overwrite-skin-dialog").wait_for(state="visible")
        page.locator('#overwrite-skin-dialog button[value="overwrite"]').click()
        page.wait_for_function("draftMock.installed.has(previewTest.draftState().id)")
        assert page.evaluate("draftMock.lastInstallPreview.id===previewTest.draftState().id")
        assert page.evaluate("draftMock.lastInstallOverwrite[draftMock.installConflicts[0].path]") == "reviewed-bytes"
        page.evaluate("draftMock.failInstallPreview=true")
        page.locator("#export-vtf").click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('Could not read the installed skin')")
        assert page.evaluate("draftMock.exports.length") == installed_before + 1
        page.evaluate("draftMock.failInstallPreview=false;draftMock.installConflicts=[]")
        print("PASS: existing/new VTF comparison, default Cancel/Escape, explicit approvals for active/all/saved installs, changed-file refusal and preflight errors")

        page.evaluate("draftMock.installAffectedModels=['models/perpheads/weapons/c_ak101.mdl']")
        installed_before = page.evaluate("draftMock.exports.length")
        page.locator("#export-vtf").click()
        page.locator("#overwrite-skin-dialog").wait_for(state="visible")
        assert page.locator("#overwrite-skin-title").inner_text() == "Review skin installation"
        assert page.locator("#skin-overwrite-warning").is_hidden()
        assert page.locator("#install-affected-models li").count() == 1
        assert page.locator("#install-affected-models li").get_attribute("title") == "models/perpheads/weapons/c_ak101.mdl"
        assert page.locator("#confirm-skin-overwrite").inner_text() == "Install"
        page.locator("#cancel-skin-overwrite").click()
        assert page.evaluate("draftMock.exports.length") == installed_before
        page.locator("#export-vtf").click()
        page.locator("#confirm-skin-overwrite").click()
        page.wait_for_function("count=>draftMock.exports.length===count+1",arg=installed_before)
        assert page.evaluate("Object.keys(draftMock.lastInstallOverwrite).length") == 0
        page.evaluate("draftMock.installAffectedModels=[]")
        page.evaluate("draftMock.installImpactWarnings=['Could not determine effects for AK-101: installed VTF is unreadable']")
        page.locator("#export-vtf").click()
        page.locator("#overwrite-skin-dialog").wait_for(state="visible")
        assert "could be confirmed" in page.locator("#install-affected-models").inner_text()
        assert "installed VTF is unreadable" in page.locator("#install-impact-warnings").inner_text()
        page.locator("#cancel-skin-overwrite").click()
        page.evaluate("draftMock.installImpactWarnings=[]")
        print("PASS: new installations review affected models without overwrite warnings; Cancel preserves files and Install accepts an empty overwrite set")

        page.evaluate("""async ()=>{
          await previewTest.open('models/perpheads/weapons/c_ak101.mdl');
          await previewTest.setSlotAttachment('SLOT_STOCK','stock_ak101');
          const fixture=previewTest.setupSharedComposition();
          draftMock.combinedPng=null;
          previewTest.editComponent(fixture.gun);
        }""")
        merge_source = page.evaluate("previewTest.compositionPng('#cc3333','#cc3333')")
        page.locator("#png-input").set_input_files({"name":"merge-component.png","mimeType":"image/png","buffer":base64.b64decode(merge_source.split(",")[1])})
        page.wait_for_function("previewTest.componentState().edits.some(edit=>edit.png===%s)&&previewTest.draftState().status.includes('Saved locally')" % json.dumps(merge_source))
        page.evaluate("""()=>{
          const path='C:\\\\GMod\\\\addons\\\\skins\\\\materials\\\\test_custom.vtf';
          draftMock.installConflicts=[{path,fingerprint:'replace-entire-atlas',
            installedPng:previewTest.compositionPng('#666666','#2255dd'),
            replacementPng:previewTest.compositionPng('#cc3333','#666666')}];
          draftMock.mergeConflicts=[{path,fingerprint:'reviewed-merged-atlas',
            installedPng:previewTest.compositionPng('#666666','#2255dd'),
            replacementPng:previewTest.compositionPng('#cc3333','#2255dd')}];
          draftMock.installAffectedModels=['models/perpheads/weapons/c_ak101.mdl','models/perpheads/weapons/attachments/stock_ak101.mdl'];
          draftMock.mergeAffectedModels=['models/perpheads/weapons/c_ak101.mdl'];
        }""")
        sources_before = page.evaluate("previewTest.componentState().edits")
        page.locator("#export-vtf").click()
        page.locator("#merge-component-install").wait_for(state="visible")
        assert page.locator("#install-affected-models li").count() == 2
        page.locator("#merge-component-install").click()
        page.wait_for_function("document.getElementById('overwrite-skin-title').textContent.includes('merged')")
        assert page.locator("#merge-component-install").is_hidden()
        assert page.locator("#install-affected-models li").count() == 1
        assert "stock" not in page.locator("#install-affected-models").inner_text()
        assert "Merged" in page.locator(".install-comparison h3").nth(1).inner_text()
        assert page.evaluate("draftMock.lastMergePreview.componentKeys.length") == 1
        assert page.evaluate("draftMock.lastMergePreview.componentKeys[0].startsWith(previewTest.activePath()+':')")
        assert page.locator(".install-comparison img").nth(1).get_attribute("src") == page.evaluate("draftMock.mergeConflicts[0].replacementPng")
        if args.screenshot:
            page.locator("#overwrite-skin-dialog").screenshot(path=str(args.screenshot.with_name("skin-merge-confirmation.png")))
        page.locator("#cancel-skin-overwrite").click()
        assert page.evaluate("draftMock.mergeInstalls||0") == 0
        page.locator("#export-vtf").click()
        page.locator("#merge-component-install").click()
        page.wait_for_function("document.getElementById('overwrite-skin-title').textContent.includes('merged')")
        page.locator("#confirm-skin-overwrite").click()
        page.wait_for_function("draftMock.mergeInstalls===1")
        page.wait_for_function("document.getElementById('toast').textContent.includes('Source drafts were kept unchanged')")
        assert page.evaluate("draftMock.lastMergeInstall.overwrite[draftMock.mergeConflicts[0].path]") == "reviewed-merged-atlas"
        assert page.evaluate("previewTest.componentState().edits") == sources_before
        assert page.evaluate("draftMock.saved.get('merged-1').archivedFiles.length") == 1
        page.evaluate("draftMock.failMerge=true")
        page.locator("#export-vtf").click()
        page.locator("#merge-component-install").click()
        page.wait_for_function("document.getElementById('toast').textContent.includes('Merge blocked')")
        assert page.evaluate("draftMock.mergeInstalls") == 1
        page.evaluate("draftMock.failMerge=false")
        page.locator("#install-all").click()
        page.locator("#merge-component-install").click()
        page.wait_for_function("document.getElementById('overwrite-skin-title').textContent.includes('merged')")
        assert page.evaluate("draftMock.lastMergePreview.componentKeys===null")
        page.locator("#confirm-skin-overwrite").click()
        page.wait_for_function("draftMock.mergeInstalls===2")
        page.locator("#install-draft").click()
        page.locator("#merge-component-install").click()
        page.wait_for_function("document.getElementById('overwrite-skin-title').textContent.includes('merged')")
        page.locator("#confirm-skin-overwrite").click()
        page.wait_for_function("draftMock.mergeInstalls===3")
        page.evaluate("draftMock.installConflicts=[];draftMock.mergeConflicts=[];draftMock.installAffectedModels=[];draftMock.mergeAffectedModels=[]")
        print("PASS: component merge option, merged-result review/cancel, scoped selection, protected-region errors, all/saved installs and unchanged component sources")

        if args.screenshot:
            for gun, sight, suffix in (("g36", "", "-g36"), ("m4a1", "", "-irons-up"), ("m4a1", "eotech", "-irons-down")):
                page.evaluate("path => previewTest.open(path)", f"models/perpheads/weapons/c_{gun}.mdl")
                if sight:
                    page.evaluate("name => previewTest.setSlotAttachment('SLOT_SIGHT', name)", sight)
                page.evaluate("previewTest.setView('front')")
                page.wait_for_timeout(1000)
                page.screenshot(path=str(args.screenshot.with_stem(args.screenshot.stem + suffix)))
            page.evaluate("previewTest.open('models/perpheads/weapons/c_ak101.mdl')")
            for slot, name in (("STOCK", "stock_sako92"), ("SIGHT", "eotech"), ("MAGAZINE", "none")):
                page.evaluate("([slot, name]) => previewTest.setSlotAttachment('SLOT_' + slot, name)", [slot, name])
            page.wait_for_timeout(1500)
            page.screenshot(path=str(args.screenshot))
            page.evaluate("previewTest.open('models/perpheads/weapons/attachments/rifle_eotech.mdl')")
            page.evaluate("previewTest.setView('side')")
            page.wait_for_timeout(1000)
            if args.fixtures:
                assert page.evaluate("""previewTest.mountedMaterials().some(material =>
                    material.isMeshBasicMaterial && material.blending === previewTest.THREE.CustomBlending
                    && material.transparent && !material.depthWrite)""")
            page.locator(".viewport-wrap").screenshot(
                path=str(args.screenshot.with_stem(args.screenshot.stem + "-sight"))
            )
            if args.fixtures:
                page.evaluate("previewTest.open('models/perpheads/weapons/c_ak101.mdl')")
                page.evaluate("previewTest.setSlotAttachment('SLOT_STOCK', 'stock_ak101')")
                point = page.evaluate("previewTest.surfacePoint('models/perpheads/weapons/attachments/stock_ak101.mdl')")
                page.mouse.click(point["x"], point["y"])
                page.wait_for_timeout(1000)
                page.screenshot(path=str(args.screenshot.with_name("picking-shared-texture.png")))
                page.evaluate("previewTest.open('models/perpheads/weapons/c_870.mdl')")
                page.evaluate("previewTest.selectTexture('materials/models/weapons/v_models/m3/map2.vtf')")
                page.locator("#show-uv").uncheck()
                page.wait_for_function("""() => {
                  const canvas = document.getElementById('texture-canvas');
                  const pixels = canvas.getContext('2d').getImageData(0,0,canvas.width,canvas.height).data;
                  return pixels.some((value, index) => index % 4 === 3 && value === 255);
                }""")
                assert page.locator("#vtf-origin").inner_text() != "Not found"
                assert "No VTF" != page.locator("#texture-size").inner_text()
                page.screenshot(path=str(args.screenshot.with_name("remington-rail-texture.png")))
                print("PASS: actual Remington rail texture displays decoded pixels rather than an empty UV canvas")
        assert not errors, errors
    finally:
        browser.close()
