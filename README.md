# PERPHEADS Texture Helper

Windows desktop prototype for finding PERPHEADS models, inspecting their textures and UVs, and preparing first-person custom skins. The app runs locally in a Tauri/WebView2 window; no hosting is needed.

**[Download the latest Windows release](https://github.com/JoeyMarrey29/TextureHelper/releases/latest)** · **[Quick usage guide](HELP.md)** · **[Report a bug](https://github.com/JoeyMarrey29/TextureHelper/issues)**

Unofficial, open-source software under [GPL-3.0](LICENSE). PERPHEADS and Garry's Mod game assets are not included or relicensed; users supply their installed content. Three.js retains its [MIT license](ui/vendor/THREE-LICENSE.txt).

## Run

Use the Setup executable from GitHub Releases for automatic updates. The button immediately beside the logo checks stable releases on startup, displays the release log, and asks before downloading/installing. Drafts must save successfully before the app closes for installation. Update packages are verified with an embedded public signing key. Manual Setup downloads and SHA256 checksums are also provided, following the public demo player's release distribution pattern. Portable development builds are not the supported automatic-update install channel.

Double-click the prebuilt `src-tauri/target/release/texture-helper.exe`. It runs locally and requires the Microsoft Edge WebView2 Runtime, which is included on most current Windows systems.

Pass `--minimized` to start without focusing the window, for example while gaming.

To run from source, install the Rust MSVC toolchain and run:

```powershell
cd src-tauri
cargo run
```

### Publishing a release

Update both versions in `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, add `releases/<version>.md`, run the tests, then commit and push `main`. Install the release CLI with `cargo install tauri-cli --locked`. Run `.\tools\release.ps1 -Publish` to build a signed Windows Setup, publish versioned GitHub assets, checksums, release notes and `latest.json`.

The private updater signing key must stay outside Git and must be backed up securely. The script accepts `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, or uses a private local key under `%LOCALAPPDATA%\TextureHelperRelease`. Losing the key prevents existing installations from trusting future updates. Never commit it. Signed updater payloads are separate from Windows Authenticode signing; current builds may trigger SmartScreen warnings.

Choose your `GarrysMod/garrysmod` folder. The app indexes GMA archives in `addons` and `steamapps/workshop/content/4000`, loose content files, and Garry's Mod/Source VPKs for material fallback. You can also choose a folder of extracted content for inspection. Use **Refresh models** to save the current draft and rescan that folder for new or changed content without choosing it again.

## Workflow

Share complete skins from **Custom skins > Export skin** as self-contained `.phskin` packages. **Import skin** beside the library heading creates a new local copy without installing or replacing existing drafts. Editable drafts retain their textures, materials, alpha choices and loadout; uninstalled backups and installed-only skins retain their VMT/VTF files. See [sharing instructions and limits](HELP.md#share-complete-skins).

**Skin Workshop (development):** browse community skins inside the main app and download them as editable local drafts, never automatic installations. **Back to editor** returns to your work. Publishing an editable draft captures a preview and requires Steam sign-in in your default browser plus explicit public-sharing consent. Submissions queue for automated validation before appearing publicly; use **Refresh** to check their status. See [service setup and security boundaries](workshop/README.md).

**Pending submissions** shows your queued and unsuccessful uploads. You can cancel your own pending uploads and remove failed or cancelled entries; moderators can manage everyone's. Removal cleans up files but retains the server audit record and quota accounting. The moderator-only **Bans** tab provides a searchable, paginated ban table with reasons, profile links and Unban buttons. Cards show uploaders' cached Steam names and profile pictures, and the account header shows your Steam name and avatar. Names are for display only: permissions always use verified SteamIDs. Bans restrict Workshop access, not the offline editor. Steam sign-in opens a localhost handoff page in your browser; return to the app after it confirms the handoff.

Failed uploads show the recorded failure reason. Older failures may have no recorded detail. Workshop confirmations use in-app dialogs rather than browser-origin popups.

Use **Cancel** beside **Publish skin** to discard the staged publishing form without deleting the saved local skin. The trusted Workshop model catalogue includes editor-compatible melee weapons such as the katana, not only guns with attachment mappings.

Sign in with Steam before choosing **Publish to workshop** on a saved skin. Signed-out publishing opens the Workshop sign-in screen without opening the publishing form or preparing a preview; choose Publish again after signing in.

Workshop uploads allow **40 attempts per account per UTC day**, with **4,000 attempts per day across everyone** and no lifetime cap. Failed or removed uploads count for that day; limits reset at midnight UTC. Free infrastructure limits may make the service unavailable before these application limits are reached.

Custom skin draft cards show lazy-loaded previews with saved texture edits and attachment choices. Click a card (or focus it and press Enter) to open the draft. Publish, Export skin, Install/Uninstall and Delete skin are in the draft inspector; backups and installed-only skins retain their library actions.

1. The library opens on **Compatible** models: finished `c_` weapons, attachment models, and magazines with a referenced weapon VMT and VTF under `models/weapons/v_models` or `models/jarheads/weapons`. Reticles are excluded because PERPHEADS skips them when loading custom skins. Crafting components are excluded. Switch to **All models** to inspect the rest. Filter either view by Weapons, Sight, Stock, Muzzle, Magazine, Underbarrel, Laser, Rails / Mounts, or Iron sights, or search its paths. Attachment categories use the Lua-defined slot rather than treating every weapon-folder model as a weapon. Each tile shows a rendered thumbnail, and the detail view shows its exact source file or GMA path.
2. Hold right click and drag the 3D view to rotate freely, Shift+right-drag to roll, and scroll to zoom. Left-click a mesh to open its texture, VMT and UVs in the right-hand editor and highlight all mounted parts sharing that VTF. Left-drag does not rotate or select; clicking empty space clears highlights without closing the texture. The editor shows only the active texture rather than a long material list; its compact **Selected texture** selector also provides access to obscured or unused materials. Use Reset, Front, Side, or Top for quick views. Each PERPHEADS gun shows its **Attachment slots**: Sight, Stock, Muzzle (suppressors, compensators, and flash hiders), Underbarrel (flashlights), Laser, Magazine, and other slots the gun supports. Each slot lists every compatible item, with its item ID, and you choose what is fitted. The assembly follows the game's `GetAttachmentList` rules: rails or mounts that a part requires (for example `Rifle rail 1` under an AK101 sight, or `Scope rail 2` for the Schmidt & Bender scope) and the gun's default parts (such as the AK101 magazine or M4A1 flip sights) are added automatically and shown as read-only rows. Every part is bonemerged onto the gun bone that shares its root bone name, as the game does with `EF_BONEMERGE`.
   Choose **No part** in the Magazine slot to hide even an automatically added magazine; choose **Default** to restore it. This preview-only override does not change the game's attachment rules.
   Gun meshes and mounting bones use the first idle-animation frame, so animation-positioned parts such as the G36 magazine seat correctly. Flip-up iron sights use their Lua-selected sequence: raised without an optic, folded with an optic.
   The material inspector also shows the GMA, VPK, or loose file that supplied each VMT and VTF.
   Base-texture lookup normalizes Source slashes, optional `materials/` prefixes and explicit `.vtf`/mistaken `.vmt` suffixes. Quoted texture paths containing spaces are preserved, including Jarheads sight mounts.
   Materials sharing the same VTF appear as one texture entry, listing every part that uses it. Each part/material has a separately colored UV overlay with its own visibility checkbox; the UV coordinate list and exported overlay follow those choices. Importing a texture updates all mounted materials sharing that VTF while preserving their individual VMT settings.
   Click the active texture summary to highlight its matching meshes in their UV overlay colors, or use a UV layer's **Highlight** button to identify just that part in the same color. Clicking empty space clears highlights; right-dragging retains them. This temporary overlay never changes textures, UVs, exports, or saved drafts.
   UVs outside 0–1 repeat onto the texture image, and the coordinate list shows their wrapped pixel positions. The preview and thumbnails follow VMT alpha test, translucency, additive blending, and `UnlitGeneric` settings, so sight reticles do not show opaque black backgrounds. Imported texture previews retain these material settings.
3. The texture panel and material swatches show RGB by default: Source often uses VTF alpha for shader masks, so displaying it as transparency can make a textured weapon's image look blank. Enable **Show texture alpha** to inspect the original transparency. This display choice never changes the 3D material, imported pixels, saved drafts, or exports; PNG/DDS exports retain the original alpha. Click **Export texture**, choose PNG or uncompressed RGBA DDS in the popup, then choose a save location. Export the transparent UV overlay separately if helpful.
   **Copy PNG to clipboard** copies opaque RGB pixels from the original or edited texture at its original dimensions, without the alpha mask or UV lines. Hidden RGB is preserved, not composited onto a background. Only the clipboard texture copy loses transparency: originals, drafts, imports, and PNG/DDS exports retain alpha. **Copy UV overlay to clipboard** copies the visible colored layers on a transparent background; **Export UV overlay** saves them as a PNG file. Paste into your image editor; overlay transparency support depends on the receiving application.
4. Import an edited PNG or DDS (uncompressed 32-bit RGBA or DXT1/3/5) with the original texture dimensions to preview it on every visible part using that material. Alternatively, copy image pixels in your editor and click **Import from clipboard** on the intended texture. Clipboard images use the same dimension checks and draft behavior; copied text/file paths are not supported, and alpha depends on the source editor. Edits to different materials remain in the assembly preview while you switch parts.
   The first import starts a **local draft** automatically. Further imports, attachment changes, and material selections update that same draft. It stores the weapon, slot selections (including No part), every edited texture, and the active part/material without installing anything in Garry's Mod. Name it at the top of the model view, even before the first import; **Save now** retries a failed save. Wait for **Saved locally · safe to close** before closing the app.
5. For materials on finished `c_` weapons, attachments, and magazines under `materials/models/weapons/v_models` or `materials/models/jarheads/weapons`, choose **Install custom skin**, or **Install all edited textures** to install every edited compatible material in the current assembly. Jarheads sight bodies and flip sights use their exact material paths, just like other weapons. Reticles remain preview-only: the game's `SetupCustomTextures` explicitly ignores material names containing `reticles`. The app converts the preview pixels to VTF with mipmaps and writes:

   - `garrysmod/addons/skins/materials/.../<texture>_custom.vtf`
   - `garrysmod/data/phcustomtextures/.../<material>.vmt`, with `$basetexture` updated to the custom VTF.

   When edited pixels contain non-opaque alpha, installation offers **Keep alpha (recommended)**, **Remove alpha**, or **Cancel**, once per shared VTF. Removal makes only the installed texture opaque; original draft pixels, previews and PNG/DDS exports remain unchanged. Alpha may control transparency or shader masks, so removing it can affect in-game appearance. The chosen installation setting is saved separately so installed status, reinstall and uninstall compare the correct bytes. This warning also applies when installing saved drafts; archived backups restore their original files unchanged.

The **Custom skins** tab lists saved drafts, retained skin backups, and installed custom materials/textures, including incomplete pairs. **Open draft** restores the weapon, attachment choices, edited texture previews, and UV workspace; select the original content folder first so the meshes and materials can be loaded. Drafts remain available even before choosing a Garry's Mod folder. Their status distinguishes **Not installed**, **Installed**, and **Partly installed**, based on matching the actual installed files. Installation never overwrites a different skin.

**Install** installs a saved draft or backup. **Uninstall** removes matching game files but keeps the local saved copy. For existing installed skins with no draft, Uninstall saves a durable VMT/VTF backup first, which can be reinstalled from the same tab. **Delete skin** removes the saved copy and all matching installed materials/textures, including attachment files and incomplete matching leftovers. Modified files and shared VTFs still referenced by another material are preserved. Without a selected game folder, confirmation warns that deletion can only remove the local copy. **Delete** on an installed-only row permanently removes its game files.

Drafts and backups are stored under the application's Windows app-data directory in `drafts`, separate from Garry's Mod. Draft saves replace their JSON snapshot atomically. Removed attachments' edits remain in the draft and reappear when that part is fitted again. These local drafts are not cloud-synced.

This follows the [PERPHEADS custom skin guide](https://help.perpheads.com/page/using-and-creating-custom-skins). The original GMA stays untouched. The app refuses to replace an existing custom skin file.

## Current scope

The 3D view shows each model's first bodygroup and root LOD in a static idle pose, with bone-weighted mesh skinning and Lua-selected flip-sight poses. It does not play weapon animations or load animations from external `.ani` files or include models; unavailable requested poses are reported rather than silently replaced. It approximates Source material lighting, so advanced shader effects may differ from the game. Skin installation follows the PERPHEADS guide for first-person weapon materials; other materials can still be inspected, mapped, exported, and previewed.

Attachment slots come from `ui/attachment-data.js`. It covers 55 guns and 99 attachments and records each gun's compatible item-backed attachments, defaults, and per-gun requirements. Revolvers and break-action shotguns have no attachment slots. The file is generated from the gamemode's client Lua (`weaponsystem/sh_attachments.lua`, `weaponsystem/attachments/*.lua`, the item registries, translations, and weapon SWEPs). To regenerate it after a PERPHEADS update:

1. Export the gluapack Lua from a recorded demo with gmoddem: `gmoddem json <demo.dem> -o out.json --gluapack <garrysmod/download/data/gluapack/*.bsp>`. This writes the decoded Lua to an `out.lua` folder. The `LooksLikeLua` check in gmoddem's `Gluapack.cs` must accept non-ASCII bytes; otherwise it stops at the first file with UTF-8 text and the weaponsystem files stay as `return gluapack()()` stubs.
2. Run `pip install lupa`, then `python tools/extract_attachment_data.py out.lua`.

The model, GMA, and VTF readers were adapted from the local demoplayer project. Three.js is vendored under `ui/vendor` with its MIT license.

## Validation

Run `cargo test` from `src-tauri`. When available, set `PERP_TEST_CONTENT` to an extracted PERPHEADS content folder, `PERP_TEST_GMA` to a PERPHEADS `content.gma`, and `PERP_TEST_GMOD` to the installed `garrysmod` folder to enable the corresponding integration checks.

Run `python tools\test_preview.py` for headless rendering and magazine-slot regression checks (requires the Python `playwright` package and Microsoft Edge). These tests verify additive reticle pixels, lighting independence, depth occlusion, thumbnails, imported-texture material settings, and magazine removal/restoration, without moving the mouse or opening a visible browser.

For actual model geometry checks, set `PERP_TEST_CONTENT` and `PERP_PREVIEW_FIXTURES` (an output folder), then run `cargo test dump_preview_fixtures -- --ignored` from `src-tauri`. Run `python tools\test_preview.py --fixtures <output-folder> --screenshot <image.png>` to verify the G36 magazine, flip-sight sequences, and library categories and save headless screenshots.
