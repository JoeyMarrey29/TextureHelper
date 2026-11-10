# Quick guide

## Getting started

1. Download the latest Windows x64 **Setup** from [GitHub Releases](https://github.com/JoeyMarrey29/TextureHelper/releases/latest) and install it.
2. Have Garry's Mod and PERPHEADS Workshop content installed. The helper does not include game content.
3. Choose your `Steam\steamapps\common\GarrysMod\garrysmod` folder.

Windows may warn about this unsigned application. Only install releases from the official repository. Setup downloads Microsoft's WebView2 runtime only if it is missing.

## Find and inspect a weapon

- Search the **Models** library or filter by Weapons, Sight, Stock, Magazine and other categories.
- Choose attachments from the slots below the model. These choices follow PERPHEADS compatibility rules.
- Hold **right click** and drag to rotate. **Shift + right drag** rolls; scroll zooms.
- **Left-click a part** to open its texture on the right. Parts sharing a VTF are highlighted together.
- The compact texture dropdown can select materials hidden inside the model. Click empty space to clear highlights.
- Toggle UV layers to inspect each part's mapping. Layer colors match the 3D highlights.

## Edit and save

1. Export the texture as **PNG** or **DDS**. Save the UV overlay separately if useful.
2. Edit it in your preferred image editor, keeping its original dimensions.
3. Import the edited PNG/DDS. Every visible part sharing that texture updates.
4. Name your draft above the model. Imports start autosaving without installing anything.
5. Wait for **Saved locally · safe to close**. Reopen through **Custom skins > Open draft**.

Texture images show RGB by default because some Source textures use alpha as a shader mask. **Show texture alpha** displays the original transparency. Exports retain the original alpha.

**Copy PNG to clipboard** copies the active texture (including imported edits) as opaque RGB pixels for pasting into an image editor, without its alpha mask or UV lines. Hidden RGB colors are preserved rather than composited onto a background. **Copy UV overlay to clipboard** copies only the visible colored UV layers on a transparent background. Both keep the original dimensions. This does not change the original texture, draft, imports, or PNG/DDS file exports, which retain alpha. **Export UV overlay** saves those layers as a PNG file. The receiving application may handle clipboard transparency differently.

**Export texture** opens a format popup: choose **PNG** or **DDS**, then **Choose save location**. Cancelling either popup leaves your texture and draft unchanged.

To bring edits back without saving a file, copy image pixels in your editor, select the intended texture, and click **Import from clipboard**. The image must match the original texture dimensions. It previews on all parts sharing that VTF and saves to your local draft, just like importing a PNG or DDS file; it does not install a skin. Clipboard text and copied file paths are not image pixels. Clipboard transparency depends on what your editor supplies.

The experimental AI integration has been removed. Previously saved drafts remain usable; any embedded original images are retained when resaving them.

## Install or remove a skin

- **Install custom skin** installs the active edited texture; **Install all edited textures** installs the current assembly's supported edits.
- If the texture contains transparency, choose **Keep alpha** (recommended), **Remove alpha**, or **Cancel**. Removing alpha affects only the installed VTF, but can change in-game transparency or shader effects. Draft pixels stay intact.
- Game files are written under `garrysmod\addons\skins` and `garrysmod\data\phcustomtextures`. Original Workshop archives are never changed.
- Existing different skins are not overwritten. Use **Custom skins > Uninstall** first; the saved copy remains available.
- **Delete skin** removes the saved copy and its matching installed textures/materials, including attachments. Shared textures still referenced by another skin and files changed since saving are kept. **Uninstall** removes the matching installation but keeps your saved copy. If no game folder is selected, deletion explicitly warns that only the saved copy can be removed.
- Jarheads sight bodies are supported. Reticles are preview-only because PERPHEADS excludes them from custom skins.
- Flip iron sights have separate body/detail textures from the weapon receiver. Edit those textures too, then use **Install all edited textures** (or Install on the saved draft). Shared texture edits are saved and installed for every currently mounted material using them, including parts mounted after the import. Up/down poses use the same material paths.

## Share complete skins

In **Custom skins**, click **Export skin** on a saved draft, uninstalled backup, or installed skin to save a `.phskin` package. Draft packages include every edited texture, alpha choice, material setting, selected attachment and draft name. Backups and installed skins include their saved VMT/VTF files; they reinstall as backups rather than editable assembly drafts. Exporting an installed skin does not uninstall it. Only edited/custom textures are included, not game models or the original content pack.

Click **Import skin** beside the Custom skins heading to add a package as a new saved copy. Import does not install anything, replace an existing draft, or change the assembly you are editing. Open an imported draft to preview it, then install separately; backups have an Install button. Recipients need the corresponding PERPHEADS content for preview. Existing different installed skins are never overwritten.

Packages are self-contained JSON files, limited to 256 MiB, with up to 256 edited textures (32 MiB PNG / 8192 pixels per side) or 512 backup files. Import validates format/version, images and relative paths; backups only accept VMT/VTF files in the existing skin folders. Only import skins from people you trust, and only share textures you have permission to distribute. Packages include the draft's name and relative game material/model paths, but not its local storage ID or absolute installation paths.

## Updates and troubleshooting

The **version/update button immediately beside the logo** checks for stable GitHub releases at startup. Open it to read the release log or check again. Download and installation require confirmation; edited textures must save successfully first. Installed releases close during updating. Old portable test ZIPs should migrate by installing Setup once.

If something looks wrong, send the weapon, attachment and material names, reproduction steps, any error text and a screenshot to [Issues](https://github.com/JoeyMarrey29/TextureHelper/issues). A failed update check may mean you are offline or GitHub is rate-limiting requests; check again later or download Setup manually.

Drafts are local to your Windows account, not cloud-synced. Back them up before moving PCs. This is an unofficial tool and a static preview: advanced Source shaders and weapon animation can differ from the game.
