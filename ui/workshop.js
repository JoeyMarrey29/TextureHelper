const invoke = (command, args = {}) => window.__TAURI__.core.invoke(command, args);
const $ = id => document.getElementById(id);
let skins = [], pending = null, account = {}, polling = null, before = null, refreshing = false;
let tab = 'community', submissions = [], bans = [], banCursor = null, viewSerial = 0;
const cancellable = new Set(['awaiting_upload', 'uploading', 'validating', 'finalizing', 'cancelled']);
const states = {awaiting_upload:'Awaiting upload', uploading:'Uploading', validating:'Validating',
  finalizing:'Publishing', failed:'Failed', cancelled:'Cancelled'};
const previews = new Map(), previewQueue = [];
let previewBytes = 0, previewRequests = 0, publishing = false;
const observer = new IntersectionObserver(entries => {
  for (const entry of entries) if (entry.isIntersecting) {
    observer.unobserve(entry.target);
    previewQueue.push(entry.target);
  }
  loadPreviews();
}, {rootMargin:'200px'});
function loadPreviews() {
  while (previewRequests < 3 && previewQueue.length) {
    const image = previewQueue.shift();
    if (!image.isConnected) continue;
    const {id, hash} = image.dataset, key = `${id}:${hash}`;
    if (previews.has(key)) { image.src = previews.get(key); continue; }
    previewRequests++;
    invoke('workshop_preview', {id, hash}).then(png => {
      if (!previews.has(key)) {
        while (previewBytes + png.length > 16 * 1024 * 1024 && previews.size) {
          const oldest = previews.keys().next().value;
          previewBytes -= previews.get(oldest).length; previews.delete(oldest);
        }
        previews.set(key, png); previewBytes += png.length;
      }
      if (image.isConnected) image.src = png;
    }).catch(error => {
      if (image.isConnected) { image.alt = 'Preview unavailable'; status(`Preview failed: ${error}`, true); }
    }).finally(() => { previewRequests--; loadPreviews(); });
  }
}
function status(message, error = false) { $('status').textContent = message; $('status').classList.toggle('error', error); }
function stopPoll() { clearInterval(polling); polling = null; }
function confirmAction(message) {
  const dialog = $('confirmation');
  if (dialog.open) return Promise.resolve(false);
  $('confirmation-message').textContent = message;
  dialog.returnValue = '';
  return new Promise(resolve => {
    dialog.addEventListener('close', () => resolve(dialog.returnValue === 'confirm'), {once:true});
    dialog.showModal();
  });
}
function uploader(item) {
  const author = document.createElement('div'); author.className = 'uploader';
  const name = document.createElement('small'); name.textContent = item.uploaderName || `Steam ${item.steamId}`;
  name.title = `SteamID: ${item.steamId}`;
  if (/^https:\/\/avatars(?:\.(?:akamai|fastly))?\.steamstatic\.com\/[a-f0-9]+(?:_medium)?\.jpg$/.test(item.uploaderAvatar || '')) {
    const image = document.createElement('img'); image.className = 'uploader-avatar'; image.alt = 'Uploader profile picture';
    image.referrerPolicy = 'no-referrer'; image.loading = 'lazy'; image.src = item.uploaderAvatar;
    image.addEventListener('error', () => { image.hidden = true; image.removeAttribute('src'); name.title += ' · Profile picture unavailable'; }, {once:true});
    author.append(image);
  }
  author.append(name);
  if (item.profileError) author.title = item.profileError;
  return author;
}
function renderAccount(value) {
  account = value;
  $('publish').hidden = !value.steamId || !pending;
  if (!value.steamId) {
    pending = null; $('publish-preview').removeAttribute('src');
    $('title').value = ''; $('description').value = ''; $('consent').checked = false;
  }
  $('account').textContent = value.steamId ? (value.name || `Steam ${value.steamId}`) : 'Browse without signing in';
  $('account').title = value.steamId ? `SteamID: ${value.steamId}${value.moderator ? ' · Moderator' : ''}` : '';
  $('account-avatar').hidden = true; $('account-avatar').removeAttribute('src');
  if (/^https:\/\/avatars(?:\.(?:akamai|fastly))?\.steamstatic\.com\/[a-f0-9]+(?:_medium)?\.jpg$/.test(value.avatar || '')) {
    $('account-avatar').src = value.avatar; $('account-avatar').hidden = false;
  }
  $('profile-note').textContent = value.profileError || '';
  $('profile-note').hidden = !value.profileError;
  $('signin').hidden = !!value.steamId;
  $('signout').hidden = !value.steamId;
  $('tab-submissions').hidden = !value.steamId;
  $('tab-bans').hidden = !value.moderator;
  submissions = []; bans = []; banCursor = null;
  if ((!value.steamId && tab === 'submissions') || (!value.moderator && tab === 'bans')) tab = 'community';
  viewSerial++;
  $('submission-list').replaceChildren(); $('ban-list').replaceChildren();
  renderTabs();
}
function renderTabs() {
  for (const button of document.querySelectorAll('[data-tab]')) {
    const active = button.dataset.tab === tab;
    button.setAttribute('aria-selected', String(active));
    button.tabIndex = active ? 0 : -1;
  }
  $('community-panel').hidden = tab !== 'community';
  $('submissions').hidden = tab !== 'submissions';
  $('moderation').hidden = tab !== 'bans';
  $('search').placeholder = tab === 'bans' ? 'Search SteamID or ban reason' : 'Search skin, weapon or SteamID';
  $('search').setAttribute('aria-label', $('search').placeholder);
  $('submissions-title').textContent = account.moderator ? 'All pending submissions' : 'Your pending submissions';
}
async function selectTab(next) {
  if (next === 'submissions' && !account.steamId || next === 'bans' && !account.moderator) return;
  if (refreshing) return;
  tab = next; viewSerial++; $('search').value = ''; renderTabs();
  await refresh();
}
async function loadSubmissions() {
  if (!account.steamId) return;
  const serial = viewSerial, steamId = account.steamId;
  const result = await invoke('workshop_submissions');
  if (serial !== viewSerial || account.steamId !== steamId) return;
  if (!Array.isArray(result.submissions)) throw new Error('Invalid submission list');
  submissions = result.submissions;
  submissionCards();
}
function matches(text) { return text.toLowerCase().includes($('search').value.toLowerCase().trim()); }
function profileButton(steamId) {
  const button = document.createElement('button'); button.textContent = 'Steam profile';
  button.addEventListener('click', async () => {
    try { await invoke('workshop_profile', {steamId}); } catch (error) { status(String(error), true); }
  });
  return button;
}
function banShortcut(steamId) {
  const button = document.createElement('button'); button.textContent = 'Ban uploader'; button.className = 'danger';
  button.addEventListener('click', async () => {
    await selectTab('bans');
    $('ban-id').value = steamId; $('ban-reason').focus();
  });
  return button;
}
function submissionCards() {
  $('submission-list').replaceChildren();
  for (const submission of submissions.filter(item => matches(`${item.title} ${item.model} ${item.steamId} ${item.uploaderName || ''} ${item.status}`))) {
    const row = document.createElement('article'); row.className = 'card';
    const placeholder = document.createElement('div'); placeholder.className = 'submission-preview';
    placeholder.textContent = states[submission.status] || submission.status;
    const content = document.createElement('div'); content.className = 'card-body';
    const title = document.createElement('h2'); title.textContent = submission.title;
    const author = uploader(submission);
    const date = document.createElement('small'); date.textContent = new Date(submission.created * 1000).toLocaleString();
    const description = document.createElement('p'); description.textContent = submission.description;
    content.append(title, author, date, description, profileButton(submission.steamId));
    if (submission.status === 'failed') {
      const reason = document.createElement('p'); reason.className = 'error';
      reason.textContent = `Failure reason: ${submission.failureReason || 'Unavailable: this older upload did not record a detailed reason.'}`;
      content.append(reason);
    }
    if (account.moderator || account.steamId === submission.steamId) {
      if (cancellable.has(submission.status)) {
        const cancel = document.createElement('button'); cancel.className = 'danger';
        cancel.textContent = submission.status === 'cancelled' ? 'Retry cleanup' : 'Cancel submission';
        cancel.addEventListener('click', async () => {
          if (!await confirmAction(`Cancel "${submission.title}" and remove its queued files? It will not be published.`)) return;
          cancel.disabled = true;
          try {
            await invoke('workshop_moderate', {id:submission.id, action:'cancel', steamId:null});
            await refresh(); status('Submission cancelled and queued files removed.');
          } catch (error) { status(`Cancellation failed: ${error}. Refresh to check its state; cleanup can be retried.`, true); }
          finally { cancel.disabled = false; }
        });
        content.append(cancel);
      }
      if (['failed', 'cancelled'].includes(submission.status)) {
        const remove = document.createElement('button'); remove.textContent = 'Remove upload'; remove.className = 'danger';
        remove.addEventListener('click', async () => {
          if (!await confirmAction(`Remove "${submission.title}" from this list and clean up its files?`)) return;
          remove.disabled = true;
          try { await invoke('workshop_moderate', {id:submission.id, action:'remove', steamId:null}); await refresh(); status('Upload removed.'); }
          catch (error) { status(`Removal failed: ${error}. Refresh and retry cleanup.`, true); }
          finally { remove.disabled = false; }
        });
        content.append(remove);
      }
      if (account.moderator) content.append(banShortcut(submission.steamId));
    }
    row.append(placeholder, content);
    $('submission-list').append(row);
  }
  if (!$('submission-list').childElementCount) {
    const empty = document.createElement('p'); empty.textContent = 'No matching pending or unsuccessful submissions.';
    $('submission-list').append(empty);
  }
}
async function loadBans(older = false) {
  const serial = viewSerial;
  const result = await invoke('workshop_bans', {after:older ? banCursor : null});
  if (serial !== viewSerial || !account.moderator) return;
  if (!Array.isArray(result.bans)) throw new Error('Invalid ban list');
  bans = older ? [...bans, ...result.bans.filter(item => !bans.some(previous => previous.steamId === item.steamId))] : result.bans;
  banCursor = result.next || null;
  $('more-bans').hidden = !banCursor; renderBans();
}
function renderBans() {
  $('ban-list').replaceChildren();
  for (const ban of bans.filter(item => matches(`${item.steamId} ${item.name || ''} ${item.reason}`))) {
    const row = document.createElement('tr');
    const user = document.createElement('td'), reason = document.createElement('td'), date = document.createElement('td'), actions = document.createElement('td');
    const id = document.createElement('strong'); id.textContent = ban.name || ban.steamId;
    const steamId = document.createElement('small'); steamId.textContent = ban.steamId;
    user.append(id, steamId, profileButton(ban.steamId));
    reason.textContent = ban.reason; date.textContent = new Date(ban.created * 1000).toLocaleString();
    const unban = document.createElement('button'); unban.textContent = 'Unban';
    unban.addEventListener('click', async () => {
      if (!await confirmAction(`Unban Steam ${ban.steamId}? They will be able to sign in and upload again.`)) return;
      unban.disabled = true;
      try {
        await invoke('workshop_moderate', {id:'account', action:'unban', steamId:ban.steamId});
        await refresh(); status(`Steam ${ban.steamId} unbanned.`);
      } catch (error) { status(`Unban failed: ${error}`, true); }
      finally { unban.disabled = false; }
    });
    actions.append(unban); row.append(user, reason, date, actions); $('ban-list').append(row);
  }
  $('ban-empty').hidden = !!$('ban-list').childElementCount;
}
async function loadPending() {
  if (publishing || !account.steamId) return;
  const steamId = account.steamId;
  const next = await invoke('workshop_pending');
  if (account.steamId !== steamId) return;
  if (pending?.revision === next?.revision) return;
  pending = next;
  $('publish').hidden = !pending;
  if (!pending) return;
  $('publish-preview').src = pending.preview;
  $('title').value = pending.title;
  $('description').value = '';
  $('consent').checked = false;
  $('publish-info').textContent = `${pending.draft.textures.length} edited materials · ${pending.draft.modelPath}`
    + (pending.draft.workshopSource ? ` · Remix of ${pending.draft.workshopSource.id}` : '');
}
function cards() {
  const query = $('search').value.toLowerCase().trim();
  observer.disconnect(); previewQueue.length = 0;
  $('cards').replaceChildren();
  for (const skin of skins.filter(skin => `${skin.title} ${skin.model} ${skin.steamId} ${skin.uploaderName || ''}`.toLowerCase().includes(query))) {
    const card = document.createElement('article'); card.className = 'card';
    const image = document.createElement('img'); image.alt = `Preview of ${skin.title}`;
    const content = document.createElement('div'); content.className = 'card-body';
    const title = document.createElement('h2'); title.textContent = skin.title;
    const author = uploader(skin);
    const license = document.createElement('small'); license.textContent = skin.license; author.append(license);
    const model = document.createElement('small'); model.textContent = skin.model.split('/').pop();
    const description = document.createElement('p'); description.textContent = skin.description;
    const download = document.createElement('button'); download.textContent = 'Download draft';
    download.addEventListener('click', async () => {
      download.disabled = true;
      try {
        const name = await invoke('workshop_download', {id:skin.id, hash:skin.hash});
        status(`"${name}" added to Custom skins. Open it there to preview/edit, then install separately.`);
      } catch (error) { status(String(error), true); }
      finally { download.disabled = false; }
    });
    const profile = document.createElement('button'); profile.textContent = 'Steam profile';
    profile.addEventListener('click', async () => {
      try { await invoke('workshop_profile', {steamId:skin.steamId}); } catch (error) { status(String(error), true); }
    });
    content.append(title, author, model, description, download, profile);
    if (skin.parent) {
      const remix = document.createElement('small'); remix.textContent = `Remix source: ${skin.parent}`; content.append(remix);
    }
    if (account.moderator) {
      const hide = document.createElement('button'); hide.textContent = 'Remove skin'; hide.className = 'danger';
      hide.addEventListener('click', async () => {
        if (!await confirmAction(`Remove "${skin.title}" from the workshop and GitHub downloads? Existing copies cannot be recalled.`)) return;
        try { await invoke('workshop_moderate', {id:skin.id, action:'hide', steamId:null}); await refresh(); }
        catch (error) { status(String(error), true); }
      });
      content.append(hide);
      content.append(banShortcut(skin.steamId));
    }
    card.append(image, content); $('cards').append(card);
    image.dataset.id = skin.id; image.dataset.hash = skin.previewHash;
    observer.observe(image);
  }
}
async function refresh(older = false) {
  if (refreshing) return;
  refreshing = true; $('refresh').disabled = true; $('more').disabled = true; $('more-bans').disabled = true;
  try {
    if (tab === 'submissions') {
      await loadSubmissions(); status(`${submissions.length} pending or unsuccessful submissions loaded.`); return;
    }
    if (tab === 'bans') {
      await loadBans(older); status(`${bans.length} banned users loaded${banCursor ? '; load more to see older entries' : ''}.`); return;
    }
    const result = await invoke('workshop_list', {before:older ? before : null});
    if (!Array.isArray(result.skins)) throw new Error('Invalid workshop catalogue');
    skins = older ? [...skins, ...result.skins.filter(item => !skins.some(previous => previous.id === item.id))] : result.skins;
    before = result.next || null;
    $('more').hidden = !before;
    cards();
    status(skins.length ? `${skins.length} community skins loaded.` : 'No community skins yet. Choose Publish to workshop on an editable Custom skins draft.');
  } catch (error) { status(String(error), true); }
  finally { refreshing = false; $('refresh').disabled = false; $('more').disabled = false; $('more-bans').disabled = false; }
}
$('signin').addEventListener('click', async () => {
  $('signin').disabled = true;
  try {
    stopPoll(); await invoke('workshop_signin'); status('Complete Steam sign-in in your browser. We never receive your Steam password.');
    const started = Date.now(); let waiting = false;
    polling = setInterval(async () => {
      if (waiting) return;
      if (Date.now() - started > 600000) { stopPoll(); $('signin').disabled = false; status('Sign-in timed out. Start again.', true); return; }
      waiting = true;
      try {
        const value = await invoke('workshop_account', {poll:true});
        if (!value.pending) {
          stopPoll(); renderAccount(value); cards(); $('signin').disabled = false;
          await loadPending(); await refresh(); status('Steam sign-in complete. Choose Publish to workshop on your saved skin to prepare an upload.');
        }
      } catch (error) { stopPoll(); $('signin').disabled = false; status(String(error), true); }
      finally { waiting = false; }
    }, 3000);
  } catch (error) { $('signin').disabled = false; status(String(error), true); }
});
$('signout').addEventListener('click', async () => {
  stopPoll();
  try { await invoke('workshop_signout'); renderAccount({}); cards(); status('Signed out.'); }
  catch (error) { renderAccount({}); cards(); status(`Signed out locally; server revocation failed: ${error}`, true); }
});
$('submit').addEventListener('click', async () => {
  if (publishing || !pending) return;
  if (!account.steamId) { status('Sign in with Steam before publishing.', true); return; }
  if (!$('consent').checked) { status('Confirm sharing permission, attribution and the CC BY 4.0 licence.', true); return; }
  if (!await confirmAction('Publish these textures and this preview publicly? Your SteamID/profile will be shown. Copies may remain available after removal.')) return;
  publishing = true; $('submit').disabled = true; $('cancel-publish').disabled = true;
  try {
    await invoke('workshop_publish', {submission:{revision:pending.revision, title:$('title').value.trim(), description:$('description').value.trim(),
      consent:true, parent:pending?.draft.workshopSource?.id || null}});
    pending = null; $('publish').hidden = true; await selectTab('submissions');
    status('Upload queued for automated validation. Use Refresh to check your submission; it is not public yet.');
  } catch (error) { status(`Publish failed: ${error}. No automatic retry was made.`, true); }
  finally {
    publishing = false; $('submit').disabled = false; $('cancel-publish').disabled = false; $('consent').checked = false;
    await loadPending().catch(error => status(String(error), true));
  }
});
$('cancel-publish').addEventListener('click', async () => {
  if (publishing || !pending) return;
  $('cancel-publish').disabled = true;
  try {
    await invoke('workshop_discard', {revision:pending.revision});
    pending = null; $('publish').hidden = true; $('consent').checked = false;
    $('publish-preview').removeAttribute('src'); $('title').value = ''; $('description').value = '';
    status('Publishing cancelled. Your saved skin is unchanged.');
  } catch (error) { status(`Could not cancel publishing: ${error}`, true); }
  finally { $('cancel-publish').disabled = false; }
});
$('ban-form').addEventListener('submit', async event => {
  event.preventDefault();
  const steamId = $('ban-id').value.trim();
  const reason = $('ban-reason').value.trim();
  if (!/^7656119\d{10}$/.test(steamId)) { status('Enter a SteamID64.', true); return; }
  if (!reason || reason.length > 300) { status('Enter a ban reason of 1–300 characters.', true); return; }
  if (!await confirmAction(`Ban Steam ${steamId} from Workshop sign-in and uploads?\nReason: ${reason}`)) return;
  $('ban').disabled = true;
  try {
    await invoke('workshop_moderate', {id:'account', action:'ban', steamId, reason});
    $('ban-id').value = ''; await refresh(); status(`Steam ${steamId} banned.`);
  }
  catch (error) { status(String(error), true); }
  finally { $('ban').disabled = false; }
});
for (const button of document.querySelectorAll('[data-tab]')) {
  button.addEventListener('click', () => selectTab(button.dataset.tab));
  button.addEventListener('keydown', event => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const buttons = [...document.querySelectorAll('[data-tab]')].filter(item => !item.hidden);
    const index = buttons.indexOf(button);
    const next = event.key === 'Home' ? buttons[0] : event.key === 'End' ? buttons.at(-1)
      : buttons[(index + (event.key === 'ArrowRight' ? 1 : -1) + buttons.length) % buttons.length];
    next.focus(); selectTab(next.dataset.tab);
  });
}
$('refresh').addEventListener('click', () => refresh());
$('more').addEventListener('click', () => refresh(true));
$('more-bans').addEventListener('click', () => refresh(true));
$('search').addEventListener('input', () => {
  if (tab === 'community') cards();
  else if (tab === 'submissions') submissionCards();
  else renderBans();
});
window.addEventListener('beforeunload', stopPoll);
$('account-avatar').addEventListener('error', () => {
  $('account-avatar').hidden = true;
  $('profile-note').hidden = false; $('profile-note').textContent = 'Steam profile picture could not be loaded.';
});
window.addEventListener('focus', () => loadPending().catch(error => status(String(error), true)));
Promise.all([refresh(), loadPending(), invoke('workshop_account', {poll:false}).then(async value => {
  renderAccount(value); cards(); await loadPending();
})])
  .catch(error => status(String(error), true));
