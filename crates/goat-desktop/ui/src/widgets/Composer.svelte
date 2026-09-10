<script lang="ts">
  import { tick, untrack } from 'svelte';
  import type { InputAttachment, Op } from '../shared/api';
  import type { CommandSpec } from '../shared/ipc';
  import { imageSource } from '../shared/markdown';
  import Icon from '../shared/Icon.svelte';
  let {
    value = $bindable(''), attachments = $bindable<InputAttachment[]>([]), mode, busy = false,
    disabled = false, plan = false, specs = [], files = [], focusToken = 0, paletteToken = 0,
    onsubmit, oninterrupt, onop, onerror,
  }: {
    value?: string; attachments?: InputAttachment[]; mode: 'code' | 'agent'; busy?: boolean;
    disabled?: boolean; plan?: boolean; specs?: CommandSpec[]; files?: string[]; focusToken?: number; paletteToken?: number;
    onsubmit: (text: string, attachments: InputAttachment[]) => Promise<void>;
    oninterrupt: () => Promise<void>; onop: (op: Op) => Promise<void>; onerror: (message: string) => void;
  } = $props();
  let textarea = $state<HTMLTextAreaElement>();
  let fileInput = $state<HTMLInputElement>();
  let cursor = $state(0);
  let selected = $state(0);
  let menuHidden = $state(false);
  let sending = $state(false);
  let dragging = $state(false);
  let filesRequested = false;
  let previousPalette = untrack(() => paletteToken);
  const commandQuery = $derived(mode === 'code' && /^\/[^\s]*$/.test(value) ? value.slice(1).toLowerCase() : null);
  const mention = $derived(mode === 'code' ? value.slice(0, cursor).match(/(?:^|\s)@([^\s@]*)$/) : null);
  const commands = $derived(commandQuery === null ? [] : specs.filter(spec => `${spec.name} ${spec.aliases?.join(' ') ?? ''} ${spec.description}`.toLowerCase().includes(commandQuery)).slice(0, 12));
  const mentions = $derived(mention ? files.filter(file => file.toLowerCase().includes(mention[1].toLowerCase())).slice(0, 12) : []);
  const menuCount = $derived(menuHidden ? 0 : commandQuery !== null ? commands.length : mentions.length);
  $effect(() => {
    if (focusToken) { void tick().then(() => textarea?.focus()); }
  });
  $effect(() => {
    if (paletteToken !== previousPalette) {
      previousPalette = paletteToken;
      value = '/'; cursor = 1; selected = 0; menuHidden = false;
      void tick().then(() => textarea?.focus());
    }
  });
  $effect(() => {
    value;
    if (textarea) { textarea.style.height = 'auto'; textarea.style.height = `${Math.min(textarea.scrollHeight, 190)}px`; }
  });
  function input(event: Event) {
    value = (event.currentTarget as HTMLTextAreaElement).value;
    cursor = (event.currentTarget as HTMLTextAreaElement).selectionStart;
    selected = 0;
    menuHidden = false;
    if (mode === 'code' && /(?:^|\s)@[^\s@]*$/.test(value.slice(0, cursor)) && !filesRequested) {
      filesRequested = true;
      void onop({ type: 'ListFiles' }).catch(error => { filesRequested = false; onerror(String(error)); });
    }
  }
  async function choose(index: number) {
    if (commandQuery !== null && commands[index]) {
      value = `/${commands[index].name} `;
      cursor = value.length;
    } else if (mention && mentions[index]) {
      const start = cursor - mention[1].length - 1;
      const insert = `@${mentions[index]} `;
      value = value.slice(0, start) + insert + value.slice(cursor);
      cursor = start + insert.length;
    }
    menuHidden = true;
    await tick();
    textarea?.focus(); textarea?.setSelectionRange(cursor, cursor);
  }
  async function submit() {
    if (disabled || sending || (!value.trim() && !attachments.length)) return;
    sending = true;
    const text = value;
    const images = attachments;
    try {
      await onsubmit(text, images);
      if (value === text) value = '';
      attachments = attachments.filter(attachment => !images.includes(attachment));
      menuHidden = true;
      await tick();
      textarea?.focus();
    } catch (error) {
      onerror(String(error));
    } finally {
      sending = false;
    }
  }
  function keydown(event: KeyboardEvent) {
    if (event.isComposing) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      if (menuCount) menuHidden = true;
      else if (busy) void oninterrupt().catch(error => onerror(String(error)));
      return;
    }
    if (menuCount && ['ArrowDown', 'ArrowUp', 'Tab'].includes(event.key)) {
      event.preventDefault();
      if (event.key === 'Tab') void choose(selected);
      else selected = (selected + (event.key === 'ArrowDown' ? 1 : -1) + menuCount) % menuCount;
      return;
    }
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      if (mention && menuCount) void choose(selected);
      else {
        if (commandQuery !== null && commands[selected] && !specs.some(spec => spec.name === commandQuery || spec.aliases?.includes(commandQuery))) value = `/${commands[selected].name}`;
        void submit();
      }
    }
  }
  async function addImages(files: File[]) {
    if (!files.length) return;
    if (mode === 'agent') { onerror('images go to code sessions for now'); return; }
    for (const file of files) {
      if (!['image/png', 'image/jpeg', 'image/webp', 'image/gif'].includes(file.type)) {
        onerror(`${file.name || 'This file'} is not a supported image (PNG, JPEG, WebP, GIF).`);
        continue;
      }
      try {
        const data = await new Promise<string>((resolve, reject) => {
          const reader = new FileReader();
          reader.onerror = () => reject(new Error(`Could not read ${file.name}`));
          reader.onload = () => resolve(String(reader.result).split(',', 2)[1]);
          reader.readAsDataURL(file);
        });
        attachments = [...attachments, { media_type: file.type, data, label: file.name || 'Pasted image' }];
      } catch (error) { onerror(String(error)); }
    }
  }
  function paste(event: ClipboardEvent) {
    const images = Array.from(event.clipboardData?.items ?? []).filter(item => item.kind === 'file').map(item => item.getAsFile()).filter((file): file is File => file !== null);
    if (images.length) { event.preventDefault(); void addImages(images); }
  }
  function drop(event: DragEvent) {
    event.preventDefault(); dragging = false;
    void addImages(Array.from(event.dataTransfer?.files ?? []));
  }
</script>

<div class="composer-wrap">
  {#if !menuHidden && (commandQuery !== null || mention)}
    <div class="completion-menu" role="listbox" aria-label={commandQuery !== null ? 'Commands' : 'Files'}>
      <div class="completion-heading">{commandQuery !== null ? 'Commands' : 'Project files'}<span>↑↓ navigate · Tab select</span></div>
      {#if commandQuery !== null}
        {#each commands as spec, index}<button class:highlighted={index === selected} role="option" aria-selected={index === selected} onclick={() => choose(index)}><span class="command-name">/{spec.name}</span><span>{spec.description}</span></button>{/each}
        {#if !commands.length}<p class="muted">No matching commands. Press Enter to send it to the command registry.</p>{/if}
      {:else}
        {#each mentions as file, index}<button class:highlighted={index === selected} role="option" aria-selected={index === selected} onclick={() => choose(index)}><Icon name="code" /><span class="path">{file}</span></button>{/each}
        {#if !mentions.length}<p class="muted">No matching project files.</p>{/if}
      {/if}
    </div>
  {/if}
  <form class="composer" class:dragging onsubmit={event => { event.preventDefault(); void submit(); }} ondragover={event => { event.preventDefault(); dragging = true; }} ondragleave={() => dragging = false} ondrop={drop} aria-label="Message composer">
    {#if attachments.length}<div class="composer-attachments">{#each attachments as attachment, index}<div><img src={imageSource(attachment)} alt={attachment.label} /><button type="button" class="remove-image" aria-label={`Remove ${attachment.label}`} onclick={() => attachments = attachments.filter((_, position) => position !== index)}><Icon name="close" size={12} /></button><span>{attachment.label}</span></div>{/each}</div>{/if}
    <textarea bind:this={textarea} bind:value disabled={disabled || sending} rows="2" placeholder={mode === 'agent' ? 'What would you like your agent to do?' : plan ? 'Describe what you want to plan…' : 'Build something, ask a question, or describe a fix…'} aria-label={mode === 'agent' ? 'Message your agent' : 'Message your coding session'} oninput={input} onkeyup={event => cursor = event.currentTarget.selectionStart} onclick={event => cursor = event.currentTarget.selectionStart} onkeydown={keydown} onpaste={paste}></textarea>
    <div class="composer-toolbar">
      <div class="composer-tools">
        <input class="visually-hidden" type="file" accept="image/png,image/jpeg,image/webp,image/gif" multiple bind:this={fileInput} onchange={event => { void addImages(Array.from(event.currentTarget.files ?? [])); event.currentTarget.value = ''; }} />
        <button type="button" class="icon-button" title="Attach images" aria-label="Attach images" disabled={disabled} onclick={() => fileInput?.click()}><Icon name="paperclip" /></button>
        {#if mode === 'code'}<button type="button" class="mode-pill" class:active={plan} disabled={disabled} onclick={() => onop({ type: 'SetMode', mode: plan ? 'normal' : 'plan' }).catch(error => onerror(String(error)))}><Icon name={plan ? 'check' : 'code'} size={13} />{plan ? 'Plan' : 'Code'}<Icon name="down" size={11} /></button><span class="composer-hint">/ commands <span>·</span> @ files <span>·</span> ! shell</span>{:else}<span class="composer-hint">A direct line to your agent</span>{/if}
      </div>
      <div class="composer-actions">{#if busy && mode === 'code'}<button type="button" class="icon-button stop-button" title="Interrupt · Esc" aria-label="Interrupt turn" onclick={() => oninterrupt().catch(error => onerror(String(error)))}><Icon name="stop" size={14} /></button>{/if}<button type="submit" class="send-button" disabled={disabled || sending || (!value.trim() && !attachments.length)} aria-label={busy && mode === 'code' ? 'Queue message' : 'Send message'}><Icon name="send" size={17} /></button></div>
    </div>
  </form>
  <div class="composer-footer"><span>{busy && mode === 'code' ? 'Messages sent now are queued for the next turn' : 'Enter to send · Shift + Enter for a new line'}</span><span>goat works on your machine</span></div>
</div>
