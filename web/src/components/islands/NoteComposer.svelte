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
  import { postNote, type PostNoteOptions } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Button from '../ui/Button.svelte';
  import Textarea from '../ui/Textarea.svelte';
  import Icon from '../ui/Icon.svelte';

  interface Props {
    variant?: 'settings' | 'feed' | 'reply';
    onposted?: () => void;
    replyTo?: {
      /** ActivityPub object URI of the parent note. */
      uri: string;
      /** Display handle of the parent's author (for the hint + optional @-pre). */
      handle: string;
    };
    autofocus?: boolean;
    /** Cancel-out for reply mode (close the inline composer). */
    oncancel?: () => void;
  }

  let {
    variant = 'settings',
    onposted,
    replyTo,
    autofocus = false,
    oncancel,
  }: Props = $props();

  let content = $state(replyTo ? `@${replyTo.handle} ` : '');
  let cwOpen = $state(false);
  let spoilerText = $state('');
  let busy = $state(false);

  const MAX = 5000;
  const MAX_CW = 500;
  const charCount = $derived(content.length);
  const cwCount = $derived(spoilerText.length);
  const valid = $derived(
    content.trim().length > 0 &&
      charCount <= MAX &&
      cwCount <= MAX_CW,
  );

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    busy = true;
    const options: PostNoteOptions = {};
    if (replyTo?.uri) options.in_reply_to_uri = replyTo.uri;
    if (cwOpen && spoilerText.trim().length > 0) {
      options.spoiler_text = spoilerText.trim();
      options.sensitive = true;
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
      onposted?.();
    } else {
      toast.error(res.error?.message ?? 'Não foi possível publicar.');
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

  <form onsubmit={submit} novalidate>
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
          disabled={!valid}
        >
          {replyTo ? 'Responder' : 'Publicar'}
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
</style>
