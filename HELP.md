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
2. Edit it in your preferred image editor, keeping its aspect ratio. Higher or lower resolutions work with power-of-two width and height up to 4096 pixels per side; for example, 1024 x 1024 can replace 512 x 512.
3. Import the edited PNG/DDS. Every visible part sharing that texture updates.
4. Name your draft above the model. Imports start autosaving without installing anything.
5. Wait for **Saved locally · safe to close**. Reopen through **Custom skins > Open draft**.

Texture images show RGB by default because some Source textures use alpha as a shader mask. **Show texture alpha** displays the original transparency. Exports retain the original alpha.

**Copy PNG to clipboard** copies the active texture (including imported edits) as opaque RGB pixels for pasting into an image editor, without its alpha mask or UV lines. Hidden RGB colors are preserved rather than composited onto a background. **Copy UV overlay to clipboard** copies only the visible colored UV layers on a transparent background. Both use the selected image's current dimensions. This does not change the original texture, draft, imports, or PNG/DDS file exports, which retain alpha. **Export UV overlay** saves those layers as a PNG file. The receiving application may handle clipboard transparency differently.

**Export texture** opens a format popup: choose **PNG** or **DDS**, then **Choose save location**. Cancelling either popup leaves your texture and draft unchanged.

To bring edits back without saving a file, copy image pixels in your editor, select the intended texture, and click **Import from clipboard**. Same-ratio images with supported power-of-two dimensions import unchanged; unsupported dimensions or aspect ratios offer a resize to the original size with a distortion warning. It previews on all parts sharing that VTF and saves to your local draft, just like importing a PNG or DDS file; it does not install a skin. Clipboard text and copied file paths are not image pixels. Clipboard transparency depends on what your editor supplies.

Components sharing a VTF can use different resolutions. Their exported/installed atlas uses the largest source (including the shared base), with nearest-neighbour upscaling for smaller images to preserve alpha and hidden RGB. Local drafts keep the individual sources; conflicting UV overlaps still block output.

### GitHub Copilot skin generation

Open a model or editable draft, choose **AI / MCP** beside **Save now**, and select **GitHub Copilot subscription** (the default).

1. If the official [GitHub Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli) is missing, choose **Install Copilot CLI** and confirm the Windows Package Manager installation. Installation downloads GitHub's official package, accepts its package/source agreements, and may install dependencies. Progress and failures appear in the dialog; sign-in becomes available after detection succeeds. Nothing installs automatically. Alternatively run `winget install --id GitHub.Copilot --exact` in Windows Terminal. If WinGet is missing, install or update Microsoft's **App Installer** from the Microsoft Store first.
2. Choose **Sign in with GitHub** to complete the CLI's browser sign-in, or **Use signed-in account** if the CLI already knows your account. Once connected, the account card shows your GitHub username and a green **Connected** badge; **Disconnect** returns to the connection controls without signing out the CLI. No API key or password is entered in Texture Helper. Connecting checks authentication only; it does not prove image-generation entitlement or generate an image.
3. Enter a cosmetic skin prompt and review the consent/AI-credit notice. **Generate draft preview** sends each editable texture source sequentially. Both reasoning and image generation use Copilot AI credits. Each source authorizes at most one image-tool call, with no automatic image retries. This creates a separate Copilot session, not a connection to an existing chat.
4. Review every proposed image, then choose **Apply all to local draft**. Nothing is installed automatically. Closing discards unapplied previews and prevents subsequent source requests, but cannot undo in-flight work or credit use.

[Copilot image generation](https://docs.github.com/en/copilot/concepts/agent-capabilities/agent-powered-workflows-and-operations/image-generation) is a preview feature requiring an eligible paid plan; Free and Student plans are not supported. Organization policies and service/runtime availability can block access. A refusal, missing inline PNG, changed aspect ratio or incomplete batch is reported without applying edits or automatically retrying. No OpenAI API account is needed.

The official Rust SDK starts the installed CLI in an isolated empty workspace with only the built-in image-generation tool enabled. Repository tools, shell tools, ambient instructions, skills, agents and external MCP servers are not enabled. The app does not inherit GitHub token environment overrides for this connection. The CLI manages its own credentials outside the app, normally using the OS credential store, with a possible configuration-file fallback. Disconnecting or closing Texture Helper does not sign out the CLI; use its own logout command when needed. GitHub's data-use/retention terms apply, and runtime session data may persist outside drafts.

Exact generated PNG originals, including their provenance metadata, are retained in the draft and shared-skin package. Derived texture copies are resized without cropping and have the original alpha masks restored; they are not byte-identical and do not retain the original PNG metadata. Keep the saved originals and all visible provenance labels/watermarks. Drafts support **24 AI originals total**; the app blocks batches that would exceed this before sending textures. Review UV alignment and detail before applying. Automated tests use mocked generation; live paid generation has not been exercised.

All providers send **only a UV wireframe guide on a blank canvas by default**, at the current texture dimensions, with your prompt and texture label. Original/current texture pixels are not sent. Select **Send original VTF** to send the current edited texture (or the original if unedited) as a PNG reference instead; this may be more likely to trigger provider content moderation. It does not upload the raw VTF file. The option starts unchecked and resets when connecting or switching providers. Provider policies still apply to either reference. All relevant UV layers are included regardless of which overlays are hidden in the editor. Saved detached textures use their own model's geometry; if it is unavailable, the batch stops before sending any images, with no fallback to original pixels. Alpha masks stay local in UV-only mode and are restored onto the proposed result before preview/apply.

Shared **Senior Game Texture Artist** instructions prioritise UV layout, supplied detail and the user's requested style, without imposing colours or excluding neon/futuristic aesthetics. They distinguish UV-only guides from texture references, prohibit unsolicited tool execution or repeat attempts, and request one PNG without claiming unperformed model validation. Copilot receives appended system instructions, Codex receives developer instructions, and OpenAI Images/MCP receive the same context in their prompt because those interfaces do not provide a separate system-message field.

Eligible casing/ammunition textures are included alongside other editable sources, including saved detached edits. They use the same reference selection (UV-only by default), failure cards and explicit preview/apply workflow. A failure skips that source and continues the rest of the batch without retrying it; provider policies still apply.

All AI connections display **Content blocked by [provider]** when the provider explicitly reports a safety/content-filter block, followed by its supplied reason, category and input/output stage where available. Other failures (including quota errors) are not labeled as content blocks. If a provider withholds the reason, the app says so rather than guessing. Codex currently omits moderation reasons from some image events, so the app also reads its runtime's JSON error stream, associates failures with the serialized generation request, and retains only sanitized image-failure details in memory; raw diagnostics and account data are not displayed. No automatic retry is made.

### ChatGPT subscription skin generation through Codex

Select **AI / MCP > ChatGPT subscription (Codex)**.

1. If needed, choose **Install Codex CLI** and confirm. The app installs the official `OpenAI.Codex` Windows Package Manager package, not an unofficial subscription proxy. You can instead run `winget install --id OpenAI.Codex --exact` in Windows Terminal. No CLI is downloaded until you confirm installation.
2. Choose **Sign in with ChatGPT**, complete the official browser flow, and return to the app. **Use signed-in account** reuses the account previously signed in through Texture Helper. The connected card shows the account email and plan when available.
3. Enter a skin prompt and check consent, then **Generate draft preview**. This option uses Codex's built-in image generation and your eligible ChatGPT/Codex subscription limits, **not** the separately billed Images API. Availability depends on your plan, workspace policy and current Codex service/runtime. Image generation consumes included limits faster than ordinary turns. The app rejects API-key sign-ins and never falls back to API billing.
4. Review every proposed image before applying to the local draft. The same draft-wide grouping, alpha restoration, unchanged-aspect-ratio requirement, exact-original retention and 24-original limit apply as for Copilot. Every per-texture failure is listed while remaining textures continue, without retrying failed requests. Successful previews can be applied together; failed textures remain unchanged.

The official [Codex app-server](https://developers.openai.com/codex/app-server/) manages ChatGPT authentication using OpenAI's browser flow. Its account/config directory is a dedicated `codex` subdirectory of Texture Helper's local application data, **outside the repository and distributed builds**. This keeps it separate from a developer's normal Codex account/configuration; sign in once through Texture Helper even if another Codex installation is signed in. Credentials use Codex's `auto` storage mode (OS credential store where available, otherwise a local authentication file). Neither credentials nor account email are saved in skin drafts. Disconnect stops the app's runtime connection but does not erase the cached sign-in.

Each texture uses an ephemeral thread in an empty temporary workspace with a read-only sandbox. Shell execution, external MCP servers, plugins, ambient skills, repository instructions and subagents are disabled; the host rejects all runtime requests for commands, file modifications or additional permissions. Generated images can be returned inline or read from Codex's own generated-image directory, not arbitrary paths. Exact originals are retained with the draft; the runtime may also retain generated images under its app-owned account directory. OpenAI's subscription data-use/retention policies still apply.

Closing during Codex sign-in/generation stops the local runtime and prevents subsequent source calls, but cannot undo server work or subscription usage already incurred. Additional image attempts detected in a turn are interrupted; no API fallback is made. Automated generation tests use mocks. The actual runtime handshake and ephemeral-thread setup are checked without signing in or sending any generation prompt; live subscription generation still requires user validation.

See OpenAI's [authentication documentation](https://developers.openai.com/codex/auth/) and [built-in image-generation documentation](https://learn.chatgpt.com/docs/image-generation?surface=app) for current eligibility and usage limits.

### OpenAI skin generation

Open a model or editable draft, choose **AI / MCP** beside **Save now**, and select **OpenAI API**:

1. Sign in to your existing OpenAI developer account at [API keys](https://platform.openai.com/api-keys), create a key, enable API billing and ensure your project has access to the chosen image model. OpenAI may require organization verification. ChatGPT website/app subscriptions and API usage are [billed separately](https://help.openai.com/en/articles/9039756-managing-billing-for-chatgpt-and-the-api-platform); this is not ChatGPT browser sign-in.
2. Paste the API key into the app's password field and choose **Use API key**. Do not send keys in chat. This only prepares the key locally; it does not verify it with OpenAI or generate an image. The field is cleared after use.
3. Keep the default image model (`gpt-image-2.5-sunburst`) or enter another Images API edit-compatible model ID available to your project. Choose low, medium or high quality. The app uses the official `https://api.openai.com/v1/images/edits` endpoint with JSON inline-image inputs, one PNG result per request, and a fixed 1024 x 1024 generation canvas. See the [OpenAI image editing reference](https://developers.openai.com/api/reference/resources/images/methods/edit).
4. Describe the cosmetic skin and read the privacy, billing and resize notices. Check consent, then choose **Generate draft preview**. Each editable texture source is a separate, sequential, potentially billed request. Original pixels, shader alpha masks and local drafts remain unchanged while generating.
5. Review every current/proposed image, then choose **Apply all to local draft**. Confirmed edits are autosaved locally; they are never automatically installed. Closing discards the proposal and stops further calls, but in-flight work may still complete or incur charges.

The app sends opaque RGB copies of textures so hidden colour is not lost behind Source shader alpha masks. Each input is fitted **without stretching** onto a centred 1024 x 1024 square, with dark padding if needed. The returned square is cropped to that same rectangle and resized back to the source texture's exact dimensions; every original alpha value is restored. Small images are upscaled for generation and large images are downscaled, so detail can change. Preview all results: image models may shift UV islands or produce inconsistent styles across separate calls.

The existing draft-wide preview covers mounted parts and saved unmounted edits, groups shared atlases, and retains independent component sources as described below. Game-cosmetic context accompanies each request; provider policies still apply and refusals are never bypassed. Every per-texture failure (including moderation, auth/access, billing/quota, network, invalid images and missing originals) is reported and skipped, with **no automatic retries**. Remaining textures continue, even if the same account/runtime failure affects later requests. Successful results are retained for review and explicit application together, leaving failed sources unchanged. If all fail, Apply remains disabled. Batch preparation failures still stop before generation; Close/cancel stops further requests. Requests may already have incurred charges.

Your key is retained only in app memory, never saved to drafts or browser storage. Only the official OpenAI API receives it. Disconnect, switching providers, or closing the app clears the stored session; switching providers also clears consent. Closing only the dialog keeps an established session until disconnect/app exit. Texture images, material/component labels and the prompt are sent only when you generate. Follow OpenAI's current data-use and retention terms, and only send assets you have permission to share. Live account billing and generation require your own credentials and are not exercised by the automated tests.

### AI skin editing over MCP

Open a model or editable draft, choose **AI / MCP** beside **Save now**, then select **Remote MCP image tool**:

1. Enter a trusted service's **MCP Streamable HTTP URL**, not its website or a normal image API URL. Use HTTPS, or HTTP for a localhost service.
2. Choose **Sign in with service** to authorize in your default browser, or enter a bearer token and choose **Connect with token / no auth**. Sign-in requires OAuth authorization-server metadata, PKCE S256, public clients and a localhost callback. Dynamic client registration is used when available; otherwise expand OAuth settings and enter a public client ID registered with that service. Enter scopes if its documentation requires them. Protected-resource discovery supports a single advertised authorization server. Cancel closes the browser handoff locally; it does not close your browser.
3. Select a discovered **image-editing tool**. Inspect its input schema and adapt **Tool arguments** to its field names, required options and model selection. For example, a service accepting `prompt` and a PNG data URL can use:

   ```json
   {"prompt":"$PROMPT","image":"$IMAGE"}
   ```

   Markers must be complete JSON string values, including inside nested objects/arrays. `$PROMPT` supplies the user's request plus explicit fictional-game context; `$IMAGE` supplies a PNG data URL, `$IMAGE_BASE64` supplies raw PNG base64, `$MIME_TYPE` is `image/png`, and `$LABEL` identifies the material/component. Both a prompt marker and an image marker are required. No local paths, model meshes, game folder, saved package, or password are sent automatically. Custom literal arguments you enter are sent as entered.
4. Describe the cosmetic skin, consent to sending textures and choose **Generate draft preview**. Each editable texture source is sent once, sequentially, with the same request; independent component sources are sent separately. The tool must edit an input image and return exactly one standard MCP image content block (PNG, JPEG or WebP) with the **exact input dimensions**. Tools returning only text, URLs, files, tasks or multiple images are not supported. No autonomous agent, MCP sampling, local stdio process or legacy HTTP+SSE transport is provided.
5. Each texture has a live card with **Queued**, **Generating**, **Validating**, **Ready** or **Failed** status. Its loading bar is indeterminate while the provider is working (no percentage is supplied), and successful previews appear as they arrive. Failures stay visible on their cards while the batch continues. Review the current/proposed texture images, then choose **Apply all to local draft** (or **Apply N successful textures** when some fail). Nothing changes until confirmation; successful results are applied and autosaved together, leaving failed textures unchanged. Installation remains a separate action. Draft/model/loadout changes invalidate the preview. If saving fails after applying, keep the app open and use **Save now** to retry.

The request covers editable textures on the current assembly, including original textures and attachments, plus saved edits for unmounted parts. Non-editable materials such as reticles are excluded and counted in the generation status. Shared atlases are grouped unless independent component sources exist. Material scripts, attachment choices, shader alpha masks and existing attribution are preserved. Existing UV conflicts can still block installation; an image model cannot guarantee UV alignment, consistent styling between calls or acceptable output.

Requests accurately describe the images as UV atlases for cosmetic PERPHEADS / Garry's Mod game assets, not real-world weapon design or operation. This context does **not** override any provider policy or guarantee approval. Refusals are displayed; the app does not rewrite prompts or retry to evade them.

**Accounts and privacy:** the selected MCP service supplies its underlying integration and billing. Provider websites are not interchangeable MCP endpoints. For direct GitHub Copilot sign-in, use the separate Copilot connection type above; other consumer subscriptions are not automatically included. Only sign in to a service you trust and only use assets you have permission to send. Connecting discovers tools; generation sends texture images, material/component labels, the prompt and your configured arguments. Tools can have server-side effects and incur charges even when their local preview is discarded.

Connection settings and credentials are kept in memory, not saved in drafts or browser storage. Disconnect clears this app's credentials but does not revoke authorization at the service. Access tokens are not refreshed; sign in again when expired. Closing cancels remaining calls and discards unapplied previews, but cannot undo in-flight server work or charges. MCP versions 2025-03-26, 2025-06-18 and 2025-11-25 are accepted. Limits: 128 sources / 64 million pixels per preview, 4096 pixels per side, 32 MiB per encoded-image payload after base64 decoding, 48 MiB per HTTP response, and five minutes per tool request.

Previously saved drafts remain usable; any embedded original images are retained when resaving them.

## Install or remove a skin

- **Install custom skin** installs the active edited texture; **Install all edited textures** installs the current assembly's supported edits.
- If the texture contains transparency, choose **Keep alpha** (recommended), **Remove alpha**, or **Cancel**. Removing alpha affects only the installed VTF, but can change in-game transparency or shader effects. Draft pixels stay intact.
- Game files are written under `garrysmod\addons\skins` and `garrysmod\data\phcustomtextures`. Original Workshop archives are never changed.
- The install popup lists **Models that will change** across indexed finished weapons and attachments. It compares against currently installed PH skins, falling back to original content. Sharing a VTF does not flag a model unless its own UV-covered RGBA pixels or material settings actually change; unchanged settings/pixels with a renamed texture reference or equivalent resolution are excluded. The list updates again for a merge. Unreadable textures show explicit uncertainty warnings, not a claim that the model is unchanged. This static check cannot fully simulate Source shaders.
- Existing different skins open a replacement confirmation: installed VTF on the left, new VTF on the right (RGB previews); material-only changes show VMT scripts. **Cancel** or Escape keeps the installed files; **Replace entire texture** replaces the reviewed files. Changes since confirmation require another review. This works for individual, all-edited, saved-draft and backup installs. Shared textures can affect other components. Saved drafts/library entries are kept, but replaced game files are not automatically backed up; export first if needed.
- **Merge component textures**, when component assignments are available, preserves other installed UV regions and existing VMT settings. Review the merged atlas, then choose **Merge and install**. Conflicting UV overlaps with other indexed components block merging. The highest installed/source resolution is used; alpha removal affects incoming regions only. Flattened VTFs/backups cannot identify component edits, and shared-only edits must be installed separately or replaced.
- A merge saves a separate **merged installation** skin with exact combined files for status, export, uninstall and reinstall. Source drafts stay unchanged. Uninstall removes that combined installation; it does not undo only one component or restore the previously installed skin. Files still referenced by other materials are preserved.
- **Delete skin** removes the saved copy and its matching installed textures/materials, including attachments. Shared textures still referenced by another skin and files changed since saving are kept. **Uninstall** removes the matching installation but keeps your saved copy. If no game folder is selected, deletion explicitly warns that only the saved copy can be removed.
- Jarheads sight bodies are supported. Reticles are preview-only because PERPHEADS excludes them from custom skins.
- Flip iron sights have separate body/detail textures from the weapon receiver. Edit those textures too, then use **Install all edited textures** (or Install on the saved draft). Shared texture edits are saved and installed for every currently mounted material using them, including parts mounted after the import. Up/down poses use the same material paths.

## Share complete skins

In **Custom skins**, click **Export skin** on a saved draft, uninstalled backup, or installed skin to save a `.phskin` package. Draft packages include every edited texture, alpha choice, material setting, selected attachment and draft name. Backups and installed skins include their saved VMT/VTF files; they reinstall as backups rather than editable assembly drafts. Exporting an installed skin does not uninstall it. Only edited/custom textures are included, not game models or the original content pack.

Click **Import skin** beside the Custom skins heading to add a package as a new saved copy. Import does not install anything, replace an existing draft, or change the assembly you are editing. Open an imported draft to preview it, then install separately; backups have an Install button. Recipients need the corresponding PERPHEADS content for preview. Existing different installed skins require explicit overwrite confirmation.

Packages are self-contained JSON files, limited to 256 MiB, with up to 256 edited textures (32 MiB PNG / 8192 pixels per side) or 512 backup files. Import validates format/version, images and relative paths; backups only accept VMT/VTF files in the existing skin folders. Only import skins from people you trust, and only share textures you have permission to distribute. Packages include the draft's name and relative game material/model paths, but not its local storage ID or absolute installation paths.

## Updates and troubleshooting

The **version/update button immediately beside the logo** checks for stable GitHub releases at startup. Open it to read the release log or check again. Download and installation require confirmation; edited textures must save successfully first. Installed releases close during updating. Old portable test ZIPs should migrate by installing Setup once.

If something looks wrong, send the weapon, attachment and material names, reproduction steps, any error text and a screenshot to [Issues](https://github.com/JoeyMarrey29/TextureHelper/issues). A failed update check may mean you are offline or GitHub is rate-limiting requests; check again later or download Setup manually.

Drafts are local to your Windows account, not cloud-synced. Back them up before moving PCs. This is an unofficial tool and a static preview: advanced Source shaders and weapon animation can differ from the game.
