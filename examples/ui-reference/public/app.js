import {BridgeClient} from './bridge.js';

const state = document.querySelector('#state'), output = document.querySelector('#result'), message = document.querySelector('#message');
const resultContext = document.querySelector('#result-context');
const call = document.querySelector('#call'), open = document.querySelector('#open-window');
let bridge, intent;

function applyTheme(value) {
  for (const name of ['accent', 'desktop', 'surface', 'text', 'muted_text', 'border', 'danger']) document.documentElement.style.setProperty(`--${name.replaceAll('_', '-')}`, value.theme_tokens[name]);
  document.documentElement.dataset.reduceMotion = String(value.reduce_motion);
  document.documentElement.dataset.reduceTransparency = String(value.reduce_transparency);
}
try {
  bridge = new BridgeClient({onState(event, value) {
    if (event === 'ready') {
      applyTheme(value); call.disabled = !bridge.canCall; if (open) open.disabled = false;
      state.textContent = bridge.canCall ? 'Approved channel ready.' : 'Page is open; the echo Skill is not granted.';
    } else if (event === 'theme.changed') applyTheme(value);
    else if (event === 'visibility.changed') document.documentElement.dataset.visible = String(value.visible);
    else if (event === 'widget.refresh') state.textContent = 'Widget refresh requested. Business calls require the button.';
    else if (event === 'closed') { call.disabled = true; if (open) open.disabled = true; state.textContent = value.message; output.textContent = ''; resultContext.textContent = ''; }
  }});
} catch (error) { state.textContent = error.message; }

message.addEventListener('input', () => { intent = undefined; });
call.addEventListener('click', async () => {
  if (!bridge?.canCall) return;
  const current = message.value;
  if (!intent || intent.message !== current) intent = {message: current, key: crypto.randomUUID()};
  call.disabled = true; state.textContent = 'Waiting for the actual Skill result…'; output.textContent = '';
  // Preserve the submitted input alongside the result even if the editable
  // draft changes before (or after) the response arrives.
  resultContext.textContent = `Submitted message: ${JSON.stringify(current)}`;
  try {
    const result = await bridge.callEcho(current, intent.key);
    if (!bridge.ready) return;
    // This is actual backend output, never a local echo or fabricated result.
    output.textContent = result.call.output.message;
    state.textContent = 'Skill call completed.';
  } catch (error) { if (bridge.ready) state.textContent = `${error.code || 'UNAVAILABLE'}: ${error.message}`; }
  finally { call.disabled = !bridge.canCall; }
});
open?.addEventListener('click', async () => {
  open.disabled = true;
  try { await bridge.openWindow(); if (bridge.ready) state.textContent = 'The host opened this deployment’s echo window.'; }
  catch (error) { if (bridge.ready) state.textContent = `${error.code || 'UNAVAILABLE'}: ${error.message}`; }
  finally { open.disabled = !bridge.ready; }
});
addEventListener('pagehide', () => bridge?.close());
