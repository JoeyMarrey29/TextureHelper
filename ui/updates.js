export function startUpdates({invoke, saveBeforeUpdate, canUpdate, toast}) {
  const $ = id => document.getElementById(id);
  let available = null, busy = false, logLoaded = false;
  function setBusy(value) {
    busy = value;
    $('check-updates').disabled = value;
    $('install-update').disabled = value || !available;
  }
  async function check() {
    if (busy) return;
    setBusy(true);
    $('update-status').textContent = 'Checking for updates...';
    try {
      const result = await invoke('check_update');
      available = result.availableVersion;
      $('app-version').textContent = `v${result.currentVersion}`;
      $('update-status').textContent = available ? `Update v${available} available` : 'Up to date';
      $('updates-button').classList.toggle('available', !!available);
      $('update-summary').textContent = available
        ? `Version ${available} is ready. Review the log, then choose Download and install.`
        : `You are running version ${result.currentVersion}.`;
      if (result.notes) $('latest-update-notes').textContent = result.notes;
    } catch (error) {
      $('update-status').textContent = 'Update check failed';
      $('update-summary').textContent = String(error);
      toast(String(error), true);
    } finally { setBusy(false); }
  }
  async function loadLog() {
    $('update-log').textContent = 'Loading release notes...';
    try {
      const releases = await invoke('update_log');
      $('update-log').replaceChildren();
      if (!releases.length) $('update-log').textContent = 'No published releases yet.';
      for (const release of releases) {
        const heading = document.createElement('h3');
        heading.textContent = release.name || release.tag_name;
        const notes = document.createElement('pre');
        notes.textContent = release.body || 'No release notes supplied.';
        $('update-log').append(heading, notes);
      }
      logLoaded = true;
    } catch (error) {
      $('update-log').textContent = String(error);
      toast(`Update log could not be loaded: ${error}`, true);
    }
  }
  $('updates-button').addEventListener('click', () => {
    $('updates-dialog').showModal();
    if (!logLoaded) loadLog();
  });
  $('check-updates').addEventListener('click', async () => { await check(); await loadLog(); });
  $('install-update').addEventListener('click', async () => {
    if (busy || !available) return;
    if (!canUpdate()) { toast('Wait for the skin installation to finish before updating.', true); return; }
    if (!confirm(`Download and install version ${available}?\nThe app will close for installation. Edited textures will be saved to your local draft first.`)) return;
    setBusy(true);
    document.querySelector('main').inert = true;
    $('choose-folder').disabled = true;
    $('close-updates').disabled = true;
    $('update-summary').textContent = 'Saving edits, then downloading and verifying the update...';
    try {
      await saveBeforeUpdate();
      await invoke('install_update');
      $('update-summary').textContent = 'Installer started. Follow its instructions.';
    } catch (error) {
      $('update-summary').textContent = String(error);
      toast(`Update was not completed: ${error}`, true);
    } finally {
      document.querySelector('main').inert = false;
      $('choose-folder').disabled = false;
      $('close-updates').disabled = false;
      setBusy(false);
    }
  });
  $('updates-dialog').addEventListener('cancel', event => { if (busy) event.preventDefault(); });
  setBusy(false);
  check();
}
