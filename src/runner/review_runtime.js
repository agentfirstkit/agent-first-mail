'use strict';

(() => {
  const root = document.querySelector('[data-afmail-review]');
  if (!root) return;

  const statusLine = root.querySelector('[data-afmail-status]');
  let reviewContent = root.querySelector('[data-afmail-review-content]');
  const agentWorkbench = root.querySelector('[data-afmail-agent-workbench]');
  let savers = [];
  const dirty = new Set();
  const timers = new Map();
  const revisions = new Map();
  let refreshGeneration = 0;
  let appliedRefreshGeneration = 0;
  let refreshing = false;
  let localEditGeneration = 0;
  let sessionRuntime = null;
  const imageBlobs = new Map();

  const copy = key => root.dataset[key] || '';

  function formObject(form) {
    return Object.fromEntries(new FormData(form).entries());
  }

  function setSaveState(form, state, copyKey) {
    const status = form.querySelector('[data-afmail-save-status]');
    if (!status) return;
    status.dataset.state = state;
    status.textContent = copy(copyKey);
  }

  function updateStatusLine() {
    if (!statusLine) return;
    const hasSaveError = savers.some(form =>
      form.querySelector('[data-afmail-save-status][data-state="error"]')
    );
    if (hasSaveError) statusLine.textContent = copy('copySaveError');
    else if (dirty.size > 0) statusLine.textContent = copy('copyPending');
    else statusLine.textContent = '';
    pageHead.status(statusLine.textContent);
  }

  /* This window's head, and the one it may be inside.
   *
   * Framed, the name of the program and which review this is are already in
   * the frame's own head, from the listener rather than from here — so that
   * half of this bar is the same words twice. Told there is a head out there,
   * the name goes and the line about unsaved work goes up into it.
   *
   * What stays either way is the claim that nothing here leaves this machine.
   * That is not identity and no frame makes it, so it is not a frame's to
   * carry and not this bar's to drop. */
  const pageHead = afui.frame({
    onHead: head => {
      document.body.dataset.frameHead = head ? 'true' : 'false';
    },
  });

  async function persist(form) {
    const revision = revisions.get(form) || 0;
    const timer = timers.get(form);
    if (timer) clearTimeout(timer);
    timers.delete(form);
    setSaveState(form, 'saving', 'copySaving');
    updateStatusLine();
    try {
      const result = await sessionRuntime.call({
        type: `save_${form.dataset.afmailAutosave}`,
        edit: formObject(form),
      });
      if (!result || result.type !== 'saved') throw new Error('unexpected_ui_reply');
      const hash = form.querySelector('[data-afmail-base-hash]');
      if (hash) hash.value = result.base_hash || hash.value;
      if ((revisions.get(form) || 0) === revision) {
        dirty.delete(form);
        setSaveState(form, 'saved', 'copySaved');
      } else {
        schedule(form);
      }
      updateStatusLine();
      requestReviewRefresh();
      return true;
    } catch (error) {
      setSaveState(form, 'error', error.code === 'review_stale' ? 'copyConflict' : 'copySaveError');
      updateStatusLine();
      return false;
    }
  }

  function schedule(form) {
    const prior = timers.get(form);
    if (prior) clearTimeout(prior);
    timers.set(form, setTimeout(() => persist(form), 650));
  }

  function bindSaver(form) {
    revisions.set(form, 0);
    form.addEventListener('submit', event => event.preventDefault());
    form.addEventListener('input', () => {
      localEditGeneration += 1;
      revisions.set(form, (revisions.get(form) || 0) + 1);
      dirty.add(form);
      setSaveState(form, 'pending', 'copyPending');
      schedule(form);
      updateStatusLine();
    });
  }

  function decideFieldsGroup(decisionValue) {
    return decisionValue.startsWith('decide_') ? decisionValue.slice('decide_'.length) : '';
  }

  function confirmationMissing(form) {
    const input = form.querySelector('[data-afmail-required-confirmation]');
    if (!input) return false;
    const condition = input.dataset.afmailRequiredWhen || '';
    const separator = condition.indexOf(':');
    if (separator < 1) return !input.checked;
    const name = condition.slice(0, separator);
    const expected = condition.slice(separator + 1);
    // `elements.namedItem` reads a radio group as its checked value.
    const controller = form.elements.namedItem(name);
    return controller?.value === expected && !input.checked;
  }

  function bindSettler(form) {
    form.addEventListener('submit', event => event.preventDefault());

    const decisionGroup = form.querySelector('[data-afmail-decision]');
    const decideGroups = [...form.querySelectorAll('[data-afmail-decide-fields]')];
    const reasonField = form.querySelector('[data-afmail-reason]');
    const decisionOf = () => form.elements.namedItem('decision')?.value || '';
    function syncDecideFields() {
      const decision = decisionOf();
      const activeGroup = decideFieldsGroup(decision);
      for (const group of decideGroups) {
        const active = group.dataset.afmailDecideFields === activeGroup;
        group.hidden = !active;
        for (const control of group.querySelectorAll('input, select, textarea')) {
          control.disabled = !active;
          if (control.type !== 'checkbox' && control.type !== 'hidden') control.required = active;
        }
      }
      if (reasonField instanceof HTMLTextAreaElement) {
        // One reason field, inside whichever card is chosen: required when
        // refusing or changing the suggestion, an optional note on accepting.
        const chosen = form.querySelector('[data-afmail-decision-choice]:has(input:checked)');
        const required = decision === 'reject' || decision.startsWith('override_');
        reasonField.hidden = !chosen;
        reasonField.disabled = !chosen;
        reasonField.required = required;
        const label = required ? reasonField.dataset.afmailReasonLabel : reasonField.dataset.afmailNoteLabel;
        reasonField.setAttribute('aria-label', label || '');
        reasonField.placeholder = required ? (reasonField.dataset.afmailReasonPlaceholder || label || '') : (label || '');
        if (chosen && reasonField.parentNode !== chosen) chosen.appendChild(reasonField);
      }
      if (button && form.dataset.afmailSettlementPersisted !== 'true') {
        button.disabled = Boolean(decisionGroup) && !decision;
      }
    }
    const button = form.querySelector('[data-afmail-settle-button]');
    decisionGroup?.addEventListener('change', syncDecideFields);
    syncDecideFields();

    const status = form.querySelector('[data-afmail-settle-status]');
    button?.addEventListener('click', async () => {
      if (!form.reportValidity()) return;
      if (confirmationMissing(form)) {
        if (status) {
          status.dataset.state = 'error';
          status.textContent = copy('copyConfirmation');
        }
        return;
      }
      if (status) {
        status.dataset.state = 'saving';
        status.textContent = copy('copySettling');
      }
      button.disabled = true;
      try {
        const reply = await sessionRuntime.call({type: 'settle', fields: formObject(form)});
        const result = reply && reply.type === 'settlement' ? reply.result : {};
        if (result.settled) {
          if (status) {
            status.dataset.state = result.repair_required ? 'error' : 'saved';
            status.textContent = copy(
              result.repair_required ? 'copySettledRepairRequired' : 'copySettled'
            );
          }
          // A normal settlement may deliberately be replaced: that is how a
          // person revises one item without a confirmation-dialog maze. Only
          // the partial-commit case must not be repeated before repair.
          if (result.repair_required) form.dataset.afmailSettlementPersisted = 'true';
        } else if (status) {
          status.dataset.state = 'error';
          status.textContent = copy('copySettleError');
        }
      } catch (error) {
        if (status) {
          status.dataset.state = 'error';
          status.textContent = copy(
            error.code === 'review_stale' ? 'copyConflict' : 'copySettleError'
          );
        }
      } finally {
        button.disabled = form.dataset.afmailSettlementPersisted === 'true' || !decisionOf();
      }
    });
  }

  function bindReviewContent() {
    if (!reviewContent) return;
    savers = [...reviewContent.querySelectorAll('form[data-afmail-autosave]')];
    for (const form of savers) bindSaver(form);
    for (const form of reviewContent.querySelectorAll('form[data-afmail-settle]')) {
      bindSettler(form);
    }
    for (const image of reviewContent.querySelectorAll('[data-afmail-attachment-preview]')) {
      void loadAttachmentImage(image);
    }
    updateStatusLine();
  }

  async function loadAttachmentImage(image) {
    let reply;
    try {
      reply = await sessionRuntime.call({
        type: 'load_image',
        message_id: image.dataset.afmailMessageId || '',
        part_id: image.dataset.afmailPartId || '',
      });
      if (!reply || reply.type !== 'image') throw new Error('unexpected_ui_reply');
      if (!image.isConnected) {
        sessionRuntime.releaseBlob(reply.blob);
        return;
      }
      const prior = imageBlobs.get(image);
      if (prior) sessionRuntime.releaseBlob(prior);
      imageBlobs.set(image, reply.blob);
      image.src = sessionRuntime.blobUrl(reply.blob);
    } catch (_error) {
      image.removeAttribute('src');
    }
  }

  function discardReviewBindings() {
    for (const timer of timers.values()) clearTimeout(timer);
    timers.clear();
    revisions.clear();
    dirty.clear();
    savers = [];
    for (const blob of imageBlobs.values()) sessionRuntime.releaseBlob(blob);
    imageBlobs.clear();
  }

  async function refreshReviewContentWhenSafe() {
    if (
      appliedRefreshGeneration >= refreshGeneration ||
      refreshing ||
      dirty.size > 0 ||
      !reviewContent
    ) return;
    const generation = refreshGeneration;
    const localEditsAtStart = localEditGeneration;
    refreshing = true;
    let pageBlob = null;
    try {
      const reply = await sessionRuntime.call({type: 'refresh'});
      if (!reply || reply.type !== 'page') throw new Error('review refresh failed');
      pageBlob = reply.blob;
      const response = await sessionRuntime.fetchBlob(pageBlob);
      const parsed = new DOMParser().parseFromString(await response.text(), 'text/html');
      const next = parsed.querySelector('[data-afmail-review-content]');
      if (!next) throw new Error('review refresh is incomplete');
      // The request began while every editor was clean, but a person may have
      // typed while it was in flight. Even if that edit already autosaved and
      // left `dirty`, this response may predate it. Leave the refresh pending
      // and reread canonical state with the newer local edit generation.
      if (dirty.size > 0 || localEditGeneration !== localEditsAtStart) return;
      if (agentWorkbench && reviewContent.contains(agentWorkbench)) {
        const nextAgentWorkbench = next.querySelector('[data-afmail-agent-workbench]');
        const currentSuggestion = agentWorkbench.querySelector(
          '[data-afmail-agent-suggestion-content]'
        );
        const nextSuggestion = nextAgentWorkbench?.querySelector(
          '[data-afmail-agent-suggestion-content]'
        );
        if (!nextAgentWorkbench || !currentSuggestion || !nextSuggestion) {
          throw new Error('agent workbench refresh is incomplete');
        }
        // The canonical suggestion is refreshed, while the UI-owned workbench
        // node itself moves into the new page content so its visible turns,
        // composer and in-flight AFUI runtime calls survive the reread.
        currentSuggestion.replaceWith(nextSuggestion);
        nextAgentWorkbench.replaceWith(agentWorkbench);
      }
      discardReviewBindings();
      reviewContent.replaceWith(next);
      reviewContent = next;
      appliedRefreshGeneration = generation;
      bindReviewContent();
    } catch (_error) {
      appliedRefreshGeneration = generation;
      setChannelStatus('copyChannelError');
    } finally {
      if (pageBlob) sessionRuntime.releaseBlob(pageBlob);
      refreshing = false;
      void refreshReviewContentWhenSafe();
    }
  }

  function requestReviewRefresh() {
    refreshGeneration += 1;
    void refreshReviewContentWhenSafe();
  }

  const channelStatus = root.querySelector('[data-afmail-channel-status]');
  /* This line carries afmail's own words as well as the connection's, so it
   * takes AFUI's mark and none of its wording: the baseline says what a page
   * that has stopped hearing from its session looks like, and the sentence in
   * it stays translated. */
  const showConnection = afui.connection(channelStatus);
  const agentForm = root.querySelector('[data-afmail-agent-form]');
  const agentInput = root.querySelector('[data-afmail-agent-input]');
  const agentHistory = root.querySelector('[data-afmail-agent-history]');
  const agentSubmit = agentForm?.querySelector('button[type="submit"]');
  const agentShortcut = agentForm?.querySelector('[data-afmail-agent-shortcut]');
  const channelTurn = root.querySelector('[data-afmail-channel-turn]');
  let runtimeLive = false;
  let agentPending = false;

  if (agentShortcut) {
    const applePlatform = /Mac|iPhone|iPad|iPod/.test(navigator.platform);
    const label = applePlatform
      ? agentShortcut.dataset.appleLabel
      : agentShortcut.dataset.controlLabel;
    if (label) agentShortcut.textContent = label;
  }

  function setChannelStatus(copyKey) {
    if (channelStatus) channelStatus.textContent = copy(copyKey);
    if (!channelTurn) return;
    const states = {
      copyChannelConnecting: 'connecting',
      copyChannelLive: 'live',
      copyChannelWaiting: 'waiting',
      copyChannelDisconnected: 'disconnected',
      copyChannelError: 'error',
    };
    const state = states[copyKey] || 'error';
    channelTurn.dataset.state = state;
    channelTurn.hidden = state === 'live';
    if (state === 'waiting') channelTurn.scrollIntoView({ block: 'nearest' });
  }

  function enableComposer(enabled) {
    if (agentInput) agentInput.disabled = !enabled;
    if (agentSubmit) agentSubmit.disabled = !enabled;
  }

  // The language template owns each turn's markup and labels. This trusted
  // runtime only clones that declared structure and binds untrusted text with
  // textContent. Keeping every visible turn is this UI's choice, not AFUI's.
  function appendAgentTurn(role, message) {
    if (!agentHistory) return;
    const template = root.querySelector(`[data-afmail-agent-turn-template="${CSS.escape(role)}"]`);
    if (!(template instanceof HTMLTemplateElement)) return;
    const fragment = template.content.cloneNode(true);
    const messageNode = fragment.querySelector('[data-afmail-agent-turn-message]');
    if (messageNode) messageNode.textContent = message;
    agentHistory.append(fragment);
    agentHistory.lastElementChild?.scrollIntoView({ block: 'nearest' });
  }

  if (agentForm && agentInput && agentHistory && channelStatus) {
    // One AFUI call remains open from the person's message through the Agent's
    // answer. No Review request id or second correlation protocol exists.
    let selectingAgentMessage = null;

    // A drag or double-click that starts inside one turn belongs to that turn.
    // Clamp a WebKit selection that leaks across its DOM boundary, while
    // leaving normal word/paragraph selection and copying fully available.
    agentHistory.addEventListener('pointerdown', event => {
      selectingAgentMessage = event.target instanceof Element
        ? event.target.closest('[data-afmail-agent-turn-message]')
        : null;
    });
    document.addEventListener('pointerup', () => {
      setTimeout(() => { selectingAgentMessage = null; }, 0);
    });
    document.addEventListener('selectionchange', () => {
      const selection = window.getSelection();
      const anchorNode = selection?.anchorNode;
      const focusNode = selection?.focusNode;
      if (
        !selectingAgentMessage ||
        !selection ||
        selection.isCollapsed ||
        !anchorNode ||
        !focusNode ||
        !selectingAgentMessage.contains(anchorNode) ||
        selectingAgentMessage.contains(focusNode)
      ) return;

      const range = document.createRange();
      const focusIsAfter = Boolean(
        selectingAgentMessage.compareDocumentPosition(focusNode) &
        Node.DOCUMENT_POSITION_FOLLOWING
      );
      if (focusIsAfter) {
        range.setStart(anchorNode, selection.anchorOffset);
        range.setEnd(selectingAgentMessage, selectingAgentMessage.childNodes.length);
      } else {
        range.setStart(selectingAgentMessage, 0);
        range.setEnd(anchorNode, selection.anchorOffset);
      }
      selection.removeAllRanges();
      selection.addRange(range);
    });

    enableComposer(false);
    setChannelStatus('copyChannelConnecting');

    function askAgent(message) {
      agentPending = true;
      enableComposer(false);
      sessionRuntime.call(
        {type: 'agent_message', message},
        {onProgress: update => {
          if (update && update.type === 'agent_accepted') {
            setChannelStatus('copyChannelWaiting');
          }
        }},
      ).then(update => {
        if (update && update.type === 'agent_response' && typeof update.message === 'string') {
          appendAgentTurn('agent', update.message);
          setChannelStatus('copyChannelLive');
        } else {
          setChannelStatus('copyChannelError');
        }
      }).catch(() => {
        setChannelStatus('copyChannelError');
      }).finally(() => {
        agentPending = false;
        enableComposer(runtimeLive);
      });
    }

    agentInput.addEventListener('keydown', event => {
      if (
        event.key !== 'Enter' ||
        event.isComposing ||
        (!event.metaKey && !event.ctrlKey)
      ) return;
      event.preventDefault();
      agentForm.requestSubmit();
    });

    agentForm.addEventListener('submit', async event => {
      event.preventDefault();
      if (!runtimeLive || agentPending || !agentForm.reportValidity()) return;
      const message = agentInput.value.trim();
      if (!message) return;
      if (agentSubmit) agentSubmit.disabled = true;
      if (dirty.size > 0) {
        let saved = true;
        // Workspace mutations use one exclusive lock. Flush forms in order so
        // this send does not manufacture lock contention between the page's
        // own notes and draft saves.
        for (const form of [...dirty]) {
          if (!await persist(form)) {
            saved = false;
            break;
          }
        }
        if (!saved || dirty.size > 0) {
          if (agentSubmit && runtimeLive) agentSubmit.disabled = false;
          return;
        }
      }
      if (!runtimeLive || agentPending) return;
      askAgent(message);
      appendAgentTurn('person', message);
      agentInput.value = '';
      setChannelStatus('copyChannelWaiting');
    });
  }

  /* Links in a message body never navigate this surface: a window has no
   * address bar, so a person could not see where they had gone, and the
   * session they were in would be over. A click fills the server-rendered
   * link panel with the destination and offers the two safe moves — copy the
   * address, or hand it to the system browser through AFUI, which opens it
   * outside the page (a new tab when this is a remote view). The URL is only
   * ever written as text. */
  const linkPanel = root.querySelector('[data-afmail-link-panel]');
  const linkHost = linkPanel ? linkPanel.querySelector('[data-afmail-link-host]') : null;
  const linkUrl = linkPanel ? linkPanel.querySelector('[data-afmail-link-url]') : null;
  const linkStatus = linkPanel ? linkPanel.querySelector('[data-afmail-link-status]') : null;
  const linkOpen = linkPanel ? linkPanel.querySelector('[data-afmail-link-open]') : null;
  const linkCopy = linkPanel ? linkPanel.querySelector('[data-afmail-link-copy]') : null;
  const linkClose = linkPanel ? linkPanel.querySelector('[data-afmail-link-close]') : null;

  function showLinkPanel(href, hostLabel) {
    if (!linkPanel || !linkHost || !linkUrl) return;
    linkHost.textContent = hostLabel;
    linkUrl.textContent = href;
    if (linkStatus) linkStatus.textContent = '';
    if (typeof linkPanel.showModal === 'function') linkPanel.showModal();
    else linkPanel.setAttribute('open', '');
  }

  if (linkOpen && linkUrl) {
    linkOpen.addEventListener('click', async () => {
      if (linkStatus) linkStatus.textContent = '';
      linkOpen.disabled = true;
      try {
        await sessionRuntime.openExternal(linkUrl.textContent);
        if (linkStatus) linkStatus.textContent = copy('copyLinkOpened');
      } catch (failure) {
        const code = failure && failure.code ? ' (' + failure.code + ')' : '';
        if (linkStatus) linkStatus.textContent = copy('copyLinkOpenFailed') + code;
      } finally {
        linkOpen.disabled = false;
      }
    });
  }
  if (linkCopy && linkUrl) {
    linkCopy.addEventListener('click', async () => {
      const text = linkUrl.textContent;
      let copied = false;
      try {
        if (navigator.clipboard && navigator.clipboard.writeText) {
          await navigator.clipboard.writeText(text);
          copied = true;
        }
      } catch (_failure) {
        copied = false;
      }
      if (!copied) {
        // A loopback page is not always a secure context for the async
        // clipboard; selecting the text lets the person copy it themselves.
        const selection = window.getSelection();
        if (selection) {
          const range = document.createRange();
          range.selectNodeContents(linkUrl);
          selection.removeAllRanges();
          selection.addRange(range);
        }
      }
      if (linkStatus) linkStatus.textContent = copied ? copy('copyLinkCopied') : '';
    });
  }
  if (linkClose && linkPanel) linkClose.addEventListener('click', () => linkPanel.close());

  function linkFrom(target) {
    return target && typeof target.closest === 'function'
      ? target.closest('.md-link[data-afmail-href]')
      : null;
  }
  root.addEventListener('click', event => {
    const link = linkFrom(event.target);
    if (!link) return;
    event.preventDefault();
    showLinkPanel(link.dataset.afmailHref || '', link.dataset.afmailHost || '');
  });
  root.addEventListener('keydown', event => {
    if (event.key !== 'Enter' && event.key !== ' ') return;
    const link = linkFrom(event.target);
    if (!link) return;
    event.preventDefault();
    showLinkPanel(link.dataset.afmailHref || '', link.dataset.afmailHost || '');
  });

  sessionRuntime = afui.connect({
    onState: state => {
      if (state && state.type === 'changed') requestReviewRefresh();
    },
    onConnectionState: state => {
      runtimeLive = state === 'live';
      enableComposer(runtimeLive && !agentPending);
      showConnection(state);
      if (state === 'live') {
        setChannelStatus(agentPending ? 'copyChannelWaiting' : 'copyChannelLive');
      } else if (state === 'reconnecting') {
        setChannelStatus('copyChannelConnecting');
      } else {
        setChannelStatus('copyChannelDisconnected');
      }
    },
  });
  bindReviewContent();

  // Closing is the ordinary way every review ends — there is no Done button
  // to disable, only this: a person mid-edit on a note or a draft gets a
  // chance to notice before the window (and the edit) goes away. A settled
  // item is already written to disk by the time it shows as settled, so it
  // never needs this warning.
  afui.guardUnload(() => dirty.size > 0);
})();
