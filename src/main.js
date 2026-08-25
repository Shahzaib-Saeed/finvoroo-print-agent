const invoke = window.__TAURI__.core.invoke;

const $ = (id) => document.getElementById(id);

function setMessage(text, isError = false) {
  const el = $('message');
  el.textContent = text || '';
  el.style.color = isError ? '#b42318' : '';
}

async function load() {
  const status = await invoke('agent_status');
  const settings = await invoke('agent_settings');
  $('status-label').textContent = 'Running';
  $('version').textContent = status.version || '1.0.0';
  $('api').textContent = `127.0.0.1:${settings.port || 17392}`;
  $('platform').textContent = settings.platform || '';
  $('token').textContent = settings.token || '';
  await refreshPrinters(settings.default_printer_id || '');
}

async function refreshPrinters(selected) {
  const select = $('printers');
  select.innerHTML = '<option value="">Select printer…</option>';
  try {
    const printers = await invoke('list_printers');
    if (!printers.length) {
      select.innerHTML = '<option value="">No printers found</option>';
      return;
    }
    for (const p of printers) {
      const opt = document.createElement('option');
      opt.value = p.id;
      opt.textContent = `${p.name}${p.default ? ' (Windows default)' : ''} · ${p.type}`;
      if (selected && selected === p.id) opt.selected = true;
      if (!selected && p.default) opt.selected = true;
      select.appendChild(opt);
    }
  } catch (err) {
    setMessage(String(err), true);
  }
}

$('copy-token').addEventListener('click', async () => {
  await navigator.clipboard.writeText($('token').textContent.trim());
  setMessage('Token copied.');
});

$('regen-token').addEventListener('click', async () => {
  if (!confirm('Regenerate token? Finvoroo on this PC must be updated with the new token.')) return;
  const token = await invoke('regenerate_token');
  $('token').textContent = token;
  setMessage('New token generated. Update Finvoroo settings.');
});

$('refresh').addEventListener('click', () => refreshPrinters($('printers').value));

let pinTimer = null;

function showPin(code, ttl) {
  $('pair-code').textContent = code;
  $('pair-ttl').textContent = ttl ? `Expires in ${ttl}s` : '';
  if (pinTimer) clearInterval(pinTimer);
  let remaining = Number(ttl) || 60;
  pinTimer = setInterval(() => {
    remaining -= 1;
    if (remaining <= 0) {
      clearInterval(pinTimer);
      $('pair-code').textContent = '------';
      $('pair-ttl').textContent = 'Expired — generate a new code';
      return;
    }
    $('pair-ttl').textContent = `Expires in ${remaining}s`;
  }, 1000);
}

$('issue-pin').addEventListener('click', async () => {
  const result = await invoke('issue_pairing_code');
  showPin(result.code, result.ttl_seconds);
  setMessage('Enter this code in Finvoroo on this PC.');
});

$('printers').addEventListener('change', async (e) => {
  await invoke('set_default_printer', { printerId: e.target.value });
});

$('test-print').addEventListener('click', async () => {
  const printerId = $('printers').value;
  $('test-print').disabled = true;
  setMessage('Sending test print…');
  try {
    await invoke('set_default_printer', { printerId });
    await invoke('test_print', { printerId });
    setMessage('Test print sent to the printer.');
  } catch (err) {
    setMessage(String(err), true);
  } finally {
    $('test-print').disabled = false;
  }
});

load().catch((err) => {
  $('status-label').textContent = 'Error';
  setMessage(String(err), true);
});
