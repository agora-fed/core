<script lang="ts">
  // Caixa de texto pra publicar uma Note pública (compose primário) OU uma
  // resposta (reply, quando `replyTo` é passado). Conta caracteres, mostra
  // fanout na resposta, reseta no sucesso.
  //
  // 0.18.0 (Mastodon parity fase 1):
  //   - toggle "Aviso de conteúdo" (CW / spoiler_text) que revela um campo
  //     de texto + marca o post como sensitive automaticamente;
  //   - modo reply (`replyTo` prop): mostra hint "Respondendo a @handle" e
  //     envia in_reply_to_uri; o autor pode opcionalmente preservar/limpar
  //     um "@handle " pré-pendurado no texto;
  //   - contador visual usa Chip token quando perto do limite.
  import {
    postNote,
    uploadNoteMedia,
    editNote,
    type PostNoteOptions,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Button from '../ui/Button.svelte';
  import Textarea from '../ui/Textarea.svelte';
  import Icon from '../ui/Icon.svelte';

  interface Attached {
    id: string;
    url: string;
    alt_text: string;
    uploading: boolean;
  }

  interface Props {
    variant?: 'settings' | 'feed' | 'reply' | 'edit';
    onposted?: () => void;
    replyTo?: {
      /** ActivityPub object URI of the parent note. */
      uri: string;
      /** Display handle of the parent's author (for the hint + optional @-pre). */
      handle: string;
    };
    /** Edit mode: pre-fills content/CW and calls editNote on submit. */
    edit?: {
      uri: string;
      content: string;
      spoiler_text?: string | null;
      sensitive?: boolean;
    };
    autofocus?: boolean;
    /** Cancel-out for reply/edit mode (close the inline composer). */
    oncancel?: () => void;
  }

  let {
    variant = 'settings',
    onposted,
    replyTo,
    edit,
    autofocus = false,
    oncancel,
  }: Props = $props();

  let content = $state(
    edit ? edit.content : replyTo ? `@${replyTo.handle} ` : '',
  );
  let cwOpen = $state(Boolean(edit?.spoiler_text));
  let spoilerText = $state(edit?.spoiler_text ?? '');
  let busy = $state(false);
  let attached = $state<Attached[]>([]);
  let dragActive = $state(false);
  const MAX_ATT = 4;
  let fileInput = $state<HTMLInputElement | null>(null);
  const isEdit = $derived(!!edit);

  // 0.18.0-rc1: poll builder.
  let pollOpen = $state(false);
  let pollOptions = $state<string[]>(['', '']);
  let pollMultiple = $state(false);
  let pollExpires = $state(24 * 60); // 1 day
  const MAX_POLL_OPTS = 8;
  const pollExpireChoices: { label: string; minutes: number }[] = [
    { label: '30 min', minutes: 30 },
    { label: '1 h', minutes: 60 },
    { label: '6 h', minutes: 360 },
    { label: '1 dia', minutes: 24 * 60 },
    { label: '3 dias', minutes: 3 * 24 * 60 },
    { label: '7 dias', minutes: 7 * 24 * 60 },
  ];
  function addPollOption() {
    if (pollOptions.length < MAX_POLL_OPTS) pollOptions = [...pollOptions, ''];
  }
  function removePollOption(i: number) {
    if (pollOptions.length > 2)
      pollOptions = pollOptions.filter((_, idx) => idx !== i);
  }
  const pollReady = $derived(
    pollOpen &&
      pollOptions.filter((o) => o.trim().length > 0).length >= 2,
  );

  const MAX = 5000;
  const MAX_CW = 500;
  const charCount = $derived(content.length);
  const cwCount = $derived(spoilerText.length);
  const valid = $derived(
    content.trim().length > 0 &&
      charCount <= MAX &&
      cwCount <= MAX_CW &&
      (!pollOpen || pollReady),
  );

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy || attached.some((a) => a.uploading)) return;
    busy = true;
    if (edit) {
      const opts: { sensitive?: boolean; spoiler_text?: string } = {};
      if (cwOpen && spoilerText.trim().length > 0) {
        opts.spoiler_text = spoilerText.trim();
        opts.sensitive = true;
      }
      const res = await editNote(edit.uri, content, opts);
      busy = false;
      if (res.success && res.data) {
        toast.success('Publicação atualizada.');
        onposted?.();
      } else {
        toast.error(res.error?.message ?? 'Não foi possível editar.');
      }
      return;
    }
    const options: PostNoteOptions = {};
    if (replyTo?.uri) options.in_reply_to_uri = replyTo.uri;
    if (cwOpen && spoilerText.trim().length > 0) {
      options.spoiler_text = spoilerText.trim();
      options.sensitive = true;
    }
    const ids = attached.map((a) => a.id);
    if (ids.length > 0) {
      options.media_ids = ids;
      options.media_alts = attached.map((a) => a.alt_text);
    }
    if (pollOpen && pollReady) {
      options.poll = {
        options: pollOptions.map((o) => o.trim()).filter((o) => o.length > 0),
        multiple: pollMultiple,
        expires_in_minutes: pollExpires,
      };
    }
    const res = await postNote(content, options);
    busy = false;
    if (res.success && res.data) {
      const n = res.data.fanout_count;
      toast.success(
        n === 0
          ? 'Publicado. (Você ainda não tem seguidores remotos.)'
          : `Publicado. Entregando a ${n} ${n === 1 ? 'seguidor' : 'seguidores'} no fediverso.`,
      );
      content = replyTo ? `@${replyTo.handle} ` : '';
      spoilerText = '';
      cwOpen = false;
      attached = [];
      pollOpen = false;
      pollOptions = ['', ''];
      pollMultiple = false;
      pollExpires = 24 * 60;
      onposted?.();
    } else {
      toast.error(res.error?.message ?? 'Não foi possível publicar.');
    }
  }

  async function addFiles(list: FileList | File[]) {
    const room = MAX_ATT - attached.length;
    if (room <= 0) {
      toast.warning(`Máximo de ${MAX_ATT} imagens por publicação.`);
      return;
    }
    const files = Array.from(list).slice(0, room);
    for (const file of files) {
      if (!file.type.startsWith('image/')) {
        toast.warning(`${file.name}: envie apenas imagens.`);
        continue;
      }
      const localUrl = URL.createObjectURL(file);
      const tempId = `t-${Date.now()}-${Math.random()}`;
      attached = [
        ...attached,
        { id: tempId, url: localUrl, alt_text: '', uploading: true },
      ];
      const res = await uploadNoteMedia(file);
      if (res.success && res.data) {
        attached = attached.map((a) =>
          a.id === tempId
            ? { id: res.data!.id, url: res.data!.url, alt_text: '', uploading: false }
            : a,
        );
      } else {
        attached = attached.filter((a) => a.id !== tempId);
        toast.error(res.error?.message ?? 'Falha ao enviar a imagem.');
      }
    }
  }

  function removeAttached(id: string) {
    attached = attached.filter((a) => a.id !== id);
  }

  function onDragOver(e: DragEvent) {
    e.preventDefault();
    dragActive = true;
  }
  function onDragLeave(_: DragEvent) {
    dragActive = false;
  }
  async function onDrop(e: DragEvent) {
    e.preventDefault();
    dragActive = false;
    if (e.dataTransfer?.files?.length) {
      await addFiles(e.dataTransfer.files);
    }
  }
  async function onFilePick(e: Event) {
    const input = e.target as HTMLInputElement;
    if (input.files?.length) {
      await addFiles(input.files);
      input.value = '';
    }
  }
</script>

<section class={`composer v-${variant}`}>
  {#if variant === 'settings'}
    <h2>Publicar uma nota</h2>
    <p class="hint">
      Sua nota vai pro fediverso público — seguidores de qualquer instância
      recebem. Precisa ter perfil público.
    </p>
  {/if}

  {#if replyTo}
    <div class="reply-hint">
      <Icon name="reply" size={14} />
      Respondendo a <strong>@{replyTo.handle}</strong>
    </div>
  {/if}

  <form
    onsubmit={submit}
    novalidate
    ondragover={onDragOver}
    ondragleave={onDragLeave}
    ondrop={onDrop}
    class:drag={dragActive}
  >
    {#if cwOpen}
      <div class="cw-row">
        <Icon name="cw" size={14} />
        <input
          type="text"
          class="cw-input"
          bind:value={spoilerText}
          placeholder="Aviso de conteúdo (o que vem a seguir?)"
          maxlength={MAX_CW}
          aria-label="Aviso de conteúdo"
        />
        <button
          type="button"
          class="cw-close"
          onclick={() => {
            cwOpen = false;
            spoilerText = '';
          }}
          aria-label="Remover aviso de conteúdo"
        >
          <Icon name="x" size={14} />
        </button>
      </div>
    {/if}

    <Textarea
      rows={variant === 'settings' ? 4 : 3}
      bind:value={content}
      placeholder={replyTo
        ? 'Sua resposta…'
        : cwOpen
          ? 'O que vem sob o aviso (todo mundo vê depois de clicar)…'
          : 'O que você quer dizer hoje?'}
      autoResize={variant !== 'settings'}
      maxlength={MAX}
    />

    {#if pollOpen}
      <section class="poll-builder" aria-label="Configurar enquete">
        <ol>
          {#each pollOptions as opt, i (i)}
            <li>
              <input
                type="text"
                bind:value={pollOptions[i]}
                placeholder={`Opção ${i + 1}`}
                maxlength={200}
                aria-label={`Opção ${i + 1}`}
              />
              {#if pollOptions.length > 2}
                <button
                  type="button"
                  class="rm-opt"
                  aria-label="Remover opção"
                  onclick={() => removePollOption(i)}
                >
                  <Icon name="x" size={12} />
                </button>
              {/if}
            </li>
          {/each}
        </ol>
        {#if pollOptions.length < MAX_POLL_OPTS}
          <button type="button" class="add-opt" onclick={addPollOption}>
            <Icon name="plus" size={12} />
            Adicionar opção
          </button>
        {/if}
        <div class="poll-controls">
          <label class="poll-toggle">
            <input type="checkbox" bind:checked={pollMultiple} />
            <span>Permitir múltipla escolha</span>
          </label>
          <label class="poll-toggle">
            <span>Encerra em</span>
            <select bind:value={pollExpires}>
              {#each pollExpireChoices as c (c.minutes)}
                <option value={c.minutes}>{c.label}</option>
              {/each}
            </select>
          </label>
          <button
            type="button"
            class="cw-close"
            onclick={() => (pollOpen = false)}
            aria-label="Fechar enquete"
          >
            <Icon name="x" size={12} />
          </button>
        </div>
      </section>
    {/if}

    {#if attached.length > 0}
      <ul class={`atts n-${attached.length}`}>
        {#each attached as a (a.id)}
          <li>
            <img src={a.url} alt="" />
            {#if a.uploading}
              <span class="uploading" role="status">enviando…</span>
            {:else}
              <input
                type="text"
                class="alt-in"
                placeholder="Descrição da imagem (alt text) — recomendado"
                bind:value={a.alt_text}
                maxlength={1500}
              />
              <button
                type="button"
                class="rm"
                aria-label="Remover"
                onclick={() => removeAttached(a.id)}
              >
                <Icon name="x" size={14} />
              </button>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    <input
      bind:this={fileInput}
      type="file"
      accept="image/*"
      multiple
      hidden
      onchange={onFilePick}
    />

    <div class="row">
      <div class="tools">
        <button
          type="button"
          class="tool"
          class:on={cwOpen}
          onclick={() => (cwOpen = !cwOpen)}
          aria-pressed={cwOpen}
          title="Aviso de conteúdo (CW)"
        >
          <Icon name="cw" size={16} />
          <span>CW</span>
        </button>
        <button
          type="button"
          class="tool"
          onclick={() => fileInput?.click()}
          disabled={attached.length >= MAX_ATT || pollOpen}
          title="Anexar imagem"
          aria-label="Anexar imagem"
        >
          <Icon name="camera" size={16} />
          <span>Imagem</span>
        </button>
        {#if !isEdit && !replyTo}
          <button
            type="button"
            class="tool"
            class:on={pollOpen}
            onclick={() => (pollOpen = !pollOpen)}
            aria-pressed={pollOpen}
            disabled={attached.length > 0}
            title="Adicionar enquete"
          >
            <Icon name="poll" size={16} />
            <span>Enquete</span>
          </button>
        {/if}
        <span class="counter" class:warn={charCount > MAX * 0.9}>
          {charCount}/{MAX}
        </span>
      </div>
      <div class="submit-group">
        {#if oncancel}
          <Button variant="ghost" onclick={oncancel}>Cancelar</Button>
        {/if}
        <Button
          type="submit"
          variant="primary"
          loading={busy}
          disabled={!valid || attached.some((a) => a.uploading)}
        >
          {isEdit ? 'Salvar' : replyTo ? 'Responder' : 'Publicar'}
        </Button>
      </div>
    </div>
  </form>
</section>

<style>
  .composer.v-settings {
    border-top: 1px solid var(--border-subtle);
    padding-top: var(--sp-6);
    margin-top: var(--sp-8);
  }
  .composer h2 {
    margin: 0 0 var(--sp-1);
    font-size: var(--fs-lg);
  }
  .composer .hint {
    margin: 0 0 var(--sp-3);
    font-size: var(--fs-sm);
    color: var(--text-3);
  }
  .reply-hint {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    padding: var(--sp-1) var(--sp-2);
    background: var(--surface-2);
    color: var(--text-3);
    border-radius: var(--r-full);
    font-size: var(--fs-xs);
    margin-bottom: var(--sp-2);
  }
  .cw-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 0 var(--sp-3);
    margin-bottom: var(--sp-2);
    background: var(--warning-soft);
    border: 1px solid color-mix(in srgb, var(--warning) 25%, transparent);
    border-radius: var(--r-sm);
    color: var(--warning);
  }
  .cw-input {
    flex: 1;
    background: transparent;
    border: 0;
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--text-1);
    padding: var(--sp-2) 0;
    outline: none;
    min-width: 0;
  }
  .cw-input::placeholder {
    color: var(--text-3);
  }
  .cw-close {
    background: transparent;
    border: 0;
    color: var(--text-3);
    cursor: pointer;
    padding: 2px;
    border-radius: var(--r-xs);
    display: inline-flex;
  }
  .cw-close:hover {
    color: var(--text-1);
    background: var(--surface-2);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
    margin-top: var(--sp-2);
  }
  .tools {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-3);
  }
  .tool {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    padding: var(--sp-1) var(--sp-3);
    background: transparent;
    border: 1px solid var(--border-subtle);
    color: var(--text-2);
    border-radius: var(--r-full);
    font: inherit;
    font-size: var(--fs-xs);
    font-weight: var(--fw-semibold);
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .tool:hover {
    background: var(--surface-2);
    color: var(--text-1);
  }
  .tool.on {
    background: var(--warning-soft);
    color: var(--warning);
    border-color: var(--warning);
  }
  .counter {
    font-variant-numeric: tabular-nums;
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
  .counter.warn {
    color: var(--warning);
    font-weight: var(--fw-semibold);
  }
  .submit-group {
    display: inline-flex;
    gap: var(--sp-2);
  }
  form.drag {
    outline: 2px dashed var(--accent);
    outline-offset: 4px;
    border-radius: var(--r-base);
  }
  .atts {
    display: grid;
    gap: var(--sp-2);
    padding: 0;
    margin: var(--sp-2) 0 var(--sp-3);
    list-style: none;
  }
  .atts.n-2,
  .atts.n-3,
  .atts.n-4 {
    grid-template-columns: 1fr 1fr;
  }
  .atts li {
    position: relative;
    background: var(--surface-2);
    border-radius: var(--r-sm);
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
  .atts img {
    width: 100%;
    max-height: 180px;
    object-fit: cover;
    display: block;
  }
  .alt-in {
    width: 100%;
    font: inherit;
    font-size: var(--fs-xs);
    color: var(--text-1);
    background: var(--surface-1);
    border: 0;
    border-top: 1px solid var(--border-subtle);
    padding: var(--sp-2) var(--sp-3);
    outline: none;
  }
  .alt-in::placeholder {
    color: var(--text-3);
  }
  .rm {
    position: absolute;
    top: 6px;
    right: 6px;
    width: 26px;
    height: 26px;
    border-radius: 50%;
    background: rgba(0, 0, 0, 0.55);
    color: #f1f5f9;
    border: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
  }
  .rm:hover {
    background: rgba(0, 0, 0, 0.75);
  }
  .uploading {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    color: #f1f5f9;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: var(--fs-xs);
    font-weight: var(--fw-semibold);
  }
  .poll-builder {
    margin: var(--sp-2) 0;
    padding: var(--sp-3);
    background: var(--surface-2);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
  }
  .poll-builder ol {
    list-style: none;
    padding: 0;
    margin: 0 0 var(--sp-2);
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .poll-builder li {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
  }
  .poll-builder input[type='text'] {
    flex: 1;
    padding: var(--sp-2) var(--sp-3);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--text-1);
    outline: none;
  }
  .poll-builder input[type='text']:focus {
    border-color: var(--accent);
    box-shadow: var(--shadow-focus);
  }
  .rm-opt {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: 0;
    background: transparent;
    color: var(--text-3);
    cursor: pointer;
    border-radius: var(--r-sm);
  }
  .rm-opt:hover {
    background: var(--surface-3);
    color: var(--text-1);
  }
  .add-opt {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    background: transparent;
    border: 1px dashed var(--border-strong);
    color: var(--text-2);
    padding: var(--sp-1) var(--sp-3);
    border-radius: var(--r-full);
    font: inherit;
    font-size: var(--fs-xs);
    font-weight: var(--fw-semibold);
    cursor: pointer;
    margin-bottom: var(--sp-2);
  }
  .add-opt:hover {
    background: var(--surface-3);
    color: var(--text-1);
  }
  .poll-controls {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    flex-wrap: wrap;
  }
  .poll-toggle {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: var(--fs-xs);
    color: var(--text-2);
    font-weight: var(--fw-medium);
  }
  .poll-toggle select {
    padding: 4px var(--sp-2);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    color: var(--text-1);
    font: inherit;
    font-size: var(--fs-xs);
  }
</style>
