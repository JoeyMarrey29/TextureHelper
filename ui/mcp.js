export function startMcp({invoke, snapshot, apply, toast}) {
  const $ = id => document.getElementById(id);
  let connection = null, busy = false, operation = 0, proposal = null;
  const isOpenAi = () => $('ai-provider').value === 'openai';
  const isCopilot = () => $('ai-provider').value === 'copilot';
  const isCodex = () => $('ai-provider').value === 'codex';
  const installations = new Map();
  let installing = false;
  const disconnect = () => Promise.all([invoke('mcp_disconnect'), invoke('openai_disconnect'), invoke('copilot_disconnect'), invoke('codex_disconnect')]);
  const status = (message, error = false) => {
    $('mcp-status').textContent = message;
    $('mcp-status').classList.toggle('error', error);
  };
  const discard = () => {
    proposal = null;
    $('mcp-previews').replaceChildren();
    $('mcp-apply').disabled = true;
  };
  const accountControls = () => {
    const connected = !!connection, copilot = isCopilot(), codex = isCodex(), openai = isOpenAi();
    const login = (copilot || codex) && connection?.login;
    $('ai-account').classList.toggle('connected', connected);
    $('ai-account-name').textContent = login ? (copilot ? `@${login}` : login) : copilot ? 'GitHub Copilot' : codex ? 'ChatGPT / Codex' : openai ? 'OpenAI API' : 'Remote MCP';
    $('ai-account-avatar').textContent = login ? login.slice(0, 2).toUpperCase() : copilot ? 'GH' : codex ? 'GPT' : openai ? 'AI' : 'MCP';
    $('ai-account-detail').textContent = connected
      ? copilot ? (login ? 'Signed in with GitHub' : 'GitHub account connected; username unavailable')
        : codex ? `ChatGPT${connection.plan ? ` / ${connection.plan}` : ''} subscription` : openai ? 'API key stored for this session' : 'Image service connected'
      : copilot ? 'Connect your GitHub account' : codex ? 'Connect your ChatGPT account' : openai ? 'Connect with your API key' : 'Connect an image service';
    $('ai-account-badge').textContent = connected ? (openai ? 'Key ready' : 'Connected') : installing ? 'Installing...' : busy ? 'Connecting...' : 'Not connected';
    $('mcp-disconnect').classList.toggle('hidden', !connected);
    $('copilot-signin').classList.toggle('hidden', connected && copilot);
    $('copilot-connect').classList.toggle('hidden', connected && copilot);
    $('codex-signin').classList.toggle('hidden', connected && codex);
    $('codex-connect').classList.toggle('hidden', connected && codex);
    for (const provider of ['copilot', 'codex']) {
      const installation = installations.get(provider);
      $(provider + '-install-settings').classList.toggle('hidden', connected || !installation || installation.installed);
      $(provider + '-signin').disabled = !!installation && !installation.installed;
      $(provider + '-connect').disabled = !!installation && !installation.installed;
    }
  };
  const controls = () => {
    accountControls();
    $('ai-provider').disabled = busy;
    $('mcp-settings').disabled = busy;
    $('mcp-request').disabled = busy || !connection;
    $('mcp-disconnect').disabled = busy || !connection;
    $('mcp-apply').disabled = busy || !proposal;
    $('mcp-close').disabled = installing;
  };
  const providerControls = () => {
    $('openai-settings').classList.toggle('hidden', !isOpenAi());
    $('openai-generation-settings').classList.toggle('hidden', !isOpenAi());
    $('copilot-settings').classList.toggle('hidden', !isCopilot());
    $('codex-settings').classList.toggle('hidden', !isCodex());
    $('mcp-connection-settings').classList.toggle('hidden', isOpenAi() || isCopilot() || isCodex());
    $('mcp-tool-settings').classList.toggle('hidden', isOpenAi() || isCopilot() || isCodex());
  };
  const checkRuntime = async () => {
    if (!isCopilot() && !isCodex()) return;
    const provider = $('ai-provider').value;
    try {
      installations.set(provider, await invoke(provider + '_installation'));
      controls();
    } catch (error) { status(`Could not check the CLI installation: ${error}`, true); }
  };
  const installRuntime = async provider => {
    if (busy) return;
    if (!confirm(`Install the official ${provider === 'codex' ? 'OpenAI Codex' : 'GitHub Copilot'} CLI through Windows Package Manager?\nThis downloads software, accepts its package/source agreements, and may install required dependencies.`)) return;
    busy = installing = true; controls();
    status('Installing the official CLI through Windows Package Manager. This may take several minutes...');
    try {
      installations.set(provider, await invoke(provider + '_install', {confirmed:true}));
      status('CLI installed. Sign in to your account to continue.');
    } catch (error) { status(`CLI installation failed: ${error}`, true); }
    finally { busy = installing = false; controls(); }
  };
  const showTool = () => {
    discard();
    const tool = connection?.tools?.find(tool => tool.name === $('mcp-tool').value);
    $('mcp-tool-description').textContent = tool?.description || '';
    $('mcp-schema').textContent = tool ? JSON.stringify(tool.inputSchema, null, 2) : '';
  };
  const connect = async signIn => {
    if (busy) return;
    discard();
    const openai = isOpenAi(), copilot = isCopilot(), codex = isCodex();
    const url = $('mcp-url').value.trim();
    if (!openai && !copilot && !codex && !url) { status('Enter the remote MCP endpoint URL.', true); return; }
    if (openai && !$('openai-key').value.trim()) { status('Enter your OpenAI API key in the password field, not your ChatGPT password.', true); return; }
    const current = ++operation;
    busy = true; controls();
    status(codex ? (signIn ? 'Complete ChatGPT sign-in in your browser (three-minute limit)...' : 'Checking your ChatGPT account; no generation request is being made...') :
      copilot ? (signIn ? 'Complete GitHub sign-in in your browser (three-minute limit)...' : 'Checking the signed-in Copilot account; no generation request is being made...') :
      openai ? 'Preparing the API key locally; no generation request is being made...' :
      signIn ? 'Discovering OAuth settings. Complete sign-in in your browser (three-minute limit)...' : 'Connecting and discovering tools...');
    try {
      await disconnect();
      connection = null;
      accountControls();
      const connected = codex ? await invoke('codex_connect', {signIn}) :
        copilot ? await invoke('copilot_connect', {signIn}) :
        openai ? await invoke('openai_connect', {apiKey:$('openai-key').value}) :
        await invoke(signIn ? 'mcp_signin' : 'mcp_connect', signIn
        ? {url, clientId:$('mcp-client-id').value.trim(), scope:$('mcp-scopes').value.trim()}
        : {url, token:$('mcp-token').value});
      if (current !== operation) { await disconnect(); return; }
      connection = connected;
      $('mcp-token').value = '';
      $('openai-key').value = '';
      $('mcp-tool').replaceChildren(...(connection.tools || []).map(tool => {
        const option = document.createElement('option');
        option.value = tool.name; option.textContent = tool.name;
        return option;
      }));
      $('mcp-consent').checked = false;
      $('mcp-send-original').checked = false;
      showTool();
      status(codex ? 'ChatGPT connected through Codex. Uses subscription limits, not API billing.' :
        copilot ? `Copilot connected${connection.login ? ` as ${connection.login}` : ''}.` :
        openai ? 'API key ready. It has not been verified with OpenAI.' :
        `Connected. ${connection.tools.length} tools found. Select an image-editing tool and configure its arguments.`);
    } catch (error) {
      connection = null;
      if (current === operation) status(`Connection failed: ${error}`, true);
    } finally { busy = false; controls(); }
  };
  $('open-mcp').addEventListener('click', () => {
    if (!$('mcp-dialog').open) $('mcp-dialog').showModal();
    checkRuntime();
  });
  $('mcp-connect').addEventListener('click', () => connect(false));
  $('openai-connect').addEventListener('click', () => connect(false));
  $('copilot-connect').addEventListener('click', () => connect(false));
  $('copilot-signin').addEventListener('click', () => connect(true));
  $('codex-connect').addEventListener('click', () => connect(false));
  $('codex-signin').addEventListener('click', () => connect(true));
  $('copilot-install').addEventListener('click', () => installRuntime('copilot'));
  $('codex-install').addEventListener('click', () => installRuntime('codex'));
  $('mcp-signin').addEventListener('click', () => connect(true));
  $('mcp-disconnect').addEventListener('click', async () => {
    try {
      await disconnect();
      $('openai-key').value = ''; $('mcp-token').value = '';
      connection = null; discard(); showTool(); controls();
      status('Disconnected. App connections cleared; the CLI remains signed in.');
    } catch (error) { status(`Disconnect failed: ${error}`, true); }
  });
  $('mcp-tool').addEventListener('change', showTool);
  for (const id of ['mcp-arguments', 'mcp-prompt', 'mcp-consent', 'mcp-send-original', 'openai-model', 'openai-quality']) $(id).addEventListener('input', discard);
  const resetConnection = () => {
    connection = null; discard(); controls();
    $('mcp-consent').checked = false;
    $('mcp-send-original').checked = false;
    status('Connection settings changed. Connect again before generating.');
    disconnect().catch(error => status(`Could not clear the previous connection: ${error}`, true));
  };
  for (const id of ['mcp-url', 'mcp-token', 'mcp-client-id', 'mcp-scopes', 'openai-key']) $(id).addEventListener('input', resetConnection);
  $('ai-provider').addEventListener('change', () => {
    $('openai-key').value = ''; $('mcp-token').value = '';
    providerControls(); resetConnection(); checkRuntime();
  });
  $('mcp-generate').addEventListener('click', async () => {
    if (busy || !connection) return;
    discard();
    const openai = isOpenAi(), copilot = isCopilot(), codex = isCodex(), model = $('openai-model').value.trim(), quality = $('openai-quality').value;
    const prompt = $('mcp-prompt').value.trim(), tool = $('mcp-tool').value;
    if (!prompt || !$('mcp-consent').checked) { status('Enter a prompt and consent to sending textures to this service.', true); return; }
    if (new TextEncoder().encode(prompt).length > 16000) { status('The prompt exceeds 16,000 UTF-8 bytes.', true); return; }
    if (openai && (!/^[a-zA-Z0-9._-]{1,100}$/.test(model) || !['low', 'medium', 'high'].includes(quality))) {
      status('Enter an OpenAI image model ID and choose low, medium or high quality.', true); return;
    }
    let argumentsTemplate;
    if (!openai && !copilot && !codex) try {
      argumentsTemplate = JSON.parse($('mcp-arguments').value);
      if (!argumentsTemplate || Array.isArray(argumentsTemplate) || typeof argumentsTemplate !== 'object') throw Error('Expected a JSON object.');
      const values = [];
      const visit = value => {
        if (typeof value === 'string') values.push(value);
        else if (value && typeof value === 'object') Object.values(value).forEach(visit);
      };
      visit(argumentsTemplate);
      if (!values.includes('$PROMPT') || !values.some(value => value === '$IMAGE' || value === '$IMAGE_BASE64')) {
        throw Error('Include $PROMPT and either $IMAGE or $IMAGE_BASE64 as complete string values.');
      }
    } catch (error) { status(`Invalid tool arguments: ${error}`, true); return; }
    const current = ++operation, connectionId = connection.connectionId;
    busy = true; controls();
    try {
      const sendOriginal = $('mcp-send-original').checked;
      const captured = await snapshot({sendOriginal});
      if (current !== operation) return;
      if ((copilot || codex) && captured.originalCount + captured.targets.length > 24) {
        throw Error('This batch would exceed the draft limit of 24 saved AI originals. No textures were sent; use a draft with room for this batch.');
      }
      const results = [];
      for (const target of captured.targets) {
        status(`Generating ${results.length + 1} of ${captured.targets.length}: ${target.label}. ${captured.note} No local edits applied.`);
        const reference = sendOriginal ? target.png : target.referencePng;
        if (!reference) throw Error(`Missing ${sendOriginal ? 'texture' : 'UV overlay'} reference for ${target.label}. No original texture was sent.`);
        const referencePrompt = sendOriginal ? `The attached image is the current texture reference. Use its existing detail and layout as the basis for this cosmetic edit.\n${prompt}`
          : `The attached image is a UV wireframe guide only, not the original texture. Create a new cosmetic texture using its UV island positions and dimensions. Do not reproduce the guide lines in the finished texture.\n${prompt}`;
        const shared = {connectionId, prompt:referencePrompt, png:reference, label:target.label};
        const output = codex ? await invoke('codex_edit_texture', shared) : copilot
          ? await invoke('copilot_edit_texture', shared)
          : openai
          ? await invoke('openai_edit_texture', {...shared, model, quality})
          : await invoke('mcp_edit_texture', {...shared, tool, arguments:argumentsTemplate});
        if (current !== operation) return;
        let png = copilot || codex ? output.png : output;
        const originalPng = copilot || codex ? output.originalPng : null;
        if (copilot || codex) {
          if (typeof originalPng !== 'string' || !originalPng.startsWith('data:image/png;base64,')) throw Error('The AI provider did not return its original PNG; no edits applied.');
          const original = new Image(); original.src = originalPng; await original.decode();
        }
        const image = new Image(); image.src = png; await image.decode();
        if (current !== operation) return;
        if (image.width !== target.width || image.height !== target.height) throw Error(`Unexpected output size for ${target.label}.`);
        if (!sendOriginal) {
          const source = new Image(); source.src = target.png; await source.decode();
          const canvas = document.createElement('canvas'); canvas.width = target.width; canvas.height = target.height;
          const context = canvas.getContext('2d', {willReadFrequently:true});
          context.drawImage(source, 0, 0);
          const alpha = context.getImageData(0, 0, target.width, target.height).data;
          context.clearRect(0, 0, target.width, target.height); context.drawImage(image, 0, 0);
          const edited = context.getImageData(0, 0, target.width, target.height);
          for (let pixel = 3; pixel < edited.data.length; pixel += 4) edited.data[pixel] = alpha[pixel];
          context.putImageData(edited, 0, 0); png = canvas.toDataURL('image/png');
          image.src = png; await image.decode();
        }
        if (current !== operation) return;
        results.push({target, png, image, originalPng, originalProvider:codex ? 'ChatGPT / Codex' : 'GitHub Copilot'});
      }
      const cards = results.map(({target, png}) => {
        const card = document.createElement('section'); card.className = 'install-conflict';
        const heading = document.createElement('h3'); heading.textContent = target.label;
        const comparison = document.createElement('div'); comparison.className = 'install-comparison';
        for (const [label, source] of [['Current', target.png], ['Proposed', png]]) {
          const item = document.createElement('div'), title = document.createElement('h3'), image = document.createElement('img');
          title.textContent = label; image.src = source; image.alt = `${label}: ${target.label}`;
          item.append(title, image); comparison.append(item);
        }
        card.append(heading, comparison); return card;
      });
      $('mcp-previews').replaceChildren(...cards);
      proposal = {captured, results};
      status(`Review all ${results.length} proposed textures below. ${captured.note} Nothing has changed locally.`);
    } catch (error) {
      discard();
      if (current === operation) status(`Generation stopped; no edits applied: ${error}`, true);
    } finally { busy = false; controls(); }
  });
  $('mcp-apply').addEventListener('click', async () => {
    if (busy || !proposal) return;
    busy = true; controls();
    const {captured, results} = proposal;
    discard();
    try {
      const warning = await apply(captured, results);
      status(`Applied and saved ${results.length} textures to the local draft. Nothing was installed.${warning ? ` Installation is blocked: ${warning}` : ''}`, !!warning);
      toast(warning ? `AI edits saved, but installation is blocked: ${warning}` : 'AI texture edits saved to the local draft. Nothing was installed.', !!warning);
    } catch (error) { status(String(error), true); }
    finally { busy = false; controls(); }
  });
  $('mcp-close').addEventListener('click', () => $('mcp-dialog').close());
  $('mcp-dialog').addEventListener('cancel', event => { if (installing) event.preventDefault(); });
  $('mcp-dialog').addEventListener('close', () => {
    operation++; discard();
    status('Unapplied previews discarded. In-flight server work may still complete; no further textures will be sent.');
    if (busy && isCodex()) {
      connection = null;
      invoke('codex_disconnect').catch(error => toast(`Could not stop Codex: ${error}`, true));
    }
    invoke('mcp_cancel_signin').catch(error => toast(`Could not cancel sign-in: ${error}`, true));
  });
  providerControls(); controls();
}
