'use strict';

(() => {
  const root = document.querySelector('[data-afmail-review]');
  if (!root) return;

  const active = root.querySelector('[data-afmail-active]');
  const complete = root.querySelector('[data-afmail-complete]');
  const done = root.querySelector('[data-afmail-done]');
  const cancel = root.querySelector('[data-afmail-cancel]');
  const submitStatus = root.querySelector('[data-afmail-submit-status]');
  const savers = [...root.querySelectorAll('form[data-afmail-autosave]')];
  const dirty = new Set();
  const timers = new Map();
  const revisions = new Map(savers.map(form => [form, 0]));

  const copy = key => root.dataset[key] || '';

  async function post(body) {
    return fetch(location.pathname, {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      body,
    });
  }

  function setSaveState(form, state, copyKey) {
    const status = form.querySelector('[data-afmail-save-status]');
    if (!status) return;
    status.dataset.state = state;
    status.textContent = copy(copyKey);
  }

  function requiredConfirmationMissing() {
    return [...root.querySelectorAll('[data-afmail-required-confirmation]')].some(input => {
      const condition = input.dataset.afmailRequiredWhen || '';
      const separator = condition.indexOf(':');
      if (separator < 1) return !input.checked;
      const name = condition.slice(0, separator);
      const expected = condition.slice(separator + 1);
      const controller = root.querySelector(`[name="${CSS.escape(name)}"]`);
      return controller?.value === expected && !input.checked;
    });
  }

  function updateDone() {
    if (!done) return;
    const hasSaveError = savers.some(form =>
      form.querySelector('[data-afmail-save-status][data-state="error"]')
    );
    const missingConfirmation = requiredConfirmationMissing();
    done.disabled = dirty.size > 0 || hasSaveError || missingConfirmation;
    if (!submitStatus) return;
    if (hasSaveError) submitStatus.textContent = copy('copySaveError');
    else if (dirty.size > 0) submitStatus.textContent = copy('copyPending');
    else if (missingConfirmation) submitStatus.textContent = copy('copyConfirmation');
    else submitStatus.textContent = '';
  }

  async function persist(form) {
    const revision = revisions.get(form) || 0;
    const timer = timers.get(form);
    if (timer) clearTimeout(timer);
    timers.delete(form);
    setSaveState(form, 'saving', 'copySaving');
    updateDone();
    try {
      const response = await post(new URLSearchParams(new FormData(form)));
      const result = await response.json();
      if (!response.ok || !result.saved) {
        setSaveState(
          form,
          'error',
          response.status === 409 ? 'copyConflict' : 'copySaveError'
        );
        updateDone();
        return false;
      }
      const hash = form.querySelector('[data-afmail-base-hash]');
      if (hash) hash.value = result.base_hash || hash.value;
      if ((revisions.get(form) || 0) === revision) {
        dirty.delete(form);
        setSaveState(form, 'saved', 'copySaved');
      } else {
        schedule(form);
      }
      updateDone();
      return true;
    } catch (_error) {
      setSaveState(form, 'error', 'copySaveError');
      updateDone();
      return false;
    }
  }

  function schedule(form) {
    const prior = timers.get(form);
    if (prior) clearTimeout(prior);
    timers.set(form, setTimeout(() => persist(form), 650));
  }

  for (const form of savers) {
    form.addEventListener('submit', event => event.preventDefault());
    form.addEventListener('input', () => {
      revisions.set(form, (revisions.get(form) || 0) + 1);
      dirty.add(form);
      setSaveState(form, 'pending', 'copyPending');
      schedule(form);
      updateDone();
    });
  }

  for (const input of root.querySelectorAll('[data-afmail-required-confirmation], [data-afmail-submit-field]')) {
    input.addEventListener('change', updateDone);
    input.addEventListener('input', updateDone);
  }

  const pickerButtons = [...root.querySelectorAll('[data-afmail-draft-choice]')];
  const pickerPanels = [...root.querySelectorAll('[data-afmail-draft-panel]')];
  function selectDraft(index) {
    for (const button of pickerButtons) {
      const selected = button.dataset.afmailDraftChoice === index;
      button.setAttribute('aria-selected', selected ? 'true' : 'false');
    }
    for (const panel of pickerPanels) {
      panel.hidden = panel.dataset.afmailDraftPanel !== index;
    }
  }
  for (const button of pickerButtons) {
    button.addEventListener('click', () => selectDraft(button.dataset.afmailDraftChoice || '0'));
  }
  if (pickerButtons.length) selectDraft(pickerButtons[0].dataset.afmailDraftChoice || '0');

  function showComplete() {
    if (active) active.hidden = true;
    if (complete) complete.hidden = false;
  }

  async function submit(action) {
    if (done) done.disabled = true;
    if (submitStatus) submitStatus.textContent = copy('copyFinishing');
    if (action === 'submit') {
      for (const form of [...dirty]) {
        if (!await persist(form)) return;
      }
      if (requiredConfirmationMissing()) {
        updateDone();
        return;
      }
    }
    const body = new URLSearchParams({ action });
    if (action === 'submit') {
      for (const field of root.querySelectorAll('[data-afmail-submit-field][name]')) {
        if ((field.type === 'checkbox' || field.type === 'radio') && !field.checked) continue;
        body.append(field.name, field.value);
      }
    }
    try {
      const response = await post(body);
      const result = await response.json();
      if (response.ok && (result.submitted || result.cancelled)) {
        showComplete();
        return;
      }
    } catch (_error) {
      // The localized template owns the message shown below.
    }
    if (submitStatus) submitStatus.textContent = copy('copySubmitError');
    updateDone();
  }

  done?.addEventListener('click', () => submit('submit'));
  cancel?.addEventListener('click', () => submit('cancel'));
  window.addEventListener('beforeunload', event => {
    if (!dirty.size) return;
    event.preventDefault();
    event.returnValue = '';
  });
  updateDone();
})();
