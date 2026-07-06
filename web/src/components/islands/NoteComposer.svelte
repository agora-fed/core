<script lang="ts">
  // Caixa de texto pra publicar uma Note pública. Conta caracteres, mostra fan-out na resposta
  // ("publicado, entregue a 12 seguidores"), reseta o textarea no sucesso.
  // Reutilizável: em /configuracoes rende como seção standalone (variant="settings", default);
  // no /feed rende compacto dentro do card do feed (variant="feed") e avisa o pai via `onposted`
  // pra ele recarregar a lista.
  import { postNote } from '../../lib/api';

  let {
    variant = 'settings',
    onposted,
  }: { variant?: 'settings' | 'feed'; onposted?: () => void } = $props();

  let content = $state('');
  let busy = $state(false);
  let status = $state<
    | { kind: 'ok'; text: string }
    | { kind: 'error'; text: string }
    | null
  >(null);

  const MAX = 5000;
  let charCount = $derived(content.length);
  let valid = $derived(content.trim().length > 0 && charCount <= MAX);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    busy = true;
    status = null;
    const res = await postNote(content);
    busy = false;
    if (res.success && res.data) {
      const n = res.data.fanout_count;
      status = {
        kind: 'ok',
        text:
          n === 0
            ? 'Publicado. (Você ainda não tem seguidores remotos — quando tiver, eles receberão suas próximas publicações.)'
            : `Publicado. Entregando a ${n} ${n === 1 ? 'seguidor' : 'seguidores'} no fediverso.`,
      };
      content = '';
      onposted?.();
    } else {
      status = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível publicar.',
      };
    }
  }
</script>

<section class={`composer ${variant === 'feed' ? 'in-feed' : ''}`}>
  {#if variant === 'settings'}
    <h2>Publicar uma nota</h2>
    <p class="muted">
      Sua nota vai pro fediverso público — seguidores de qualquer instância recebem.
      Precisa ter perfil público.
    </p>
  {/if}

  <form onsubmit={submit} novalidate>
    <textarea
      class="input"
      rows="4"
      bind:value={content}
      placeholder="O que você quer dizer hoje?"
      maxlength={MAX}
      aria-invalid={!valid && content.length > 0}
    ></textarea>
    <div class="row">
      <span class={`count ${charCount > MAX * 0.9 ? 'warn' : 'muted'}`}>
        {charCount}/{MAX}
      </span>
      <button
        class="btn btn-primary"
        type="submit"
        disabled={!valid || busy}
      >
        {busy ? 'Publicando…' : 'Publicar'}
      </button>
    </div>
    {#if status}
      <p
        class={`note ${status.kind === 'ok' ? 'hint-ok' : 'hint-error'}`}
        role="status"
      >
        {status.text}
      </p>
    {/if}
  </form>
</section>

<style>
  .composer {
    border-top: 1px solid var(--c-border);
    padding-top: 1.5rem;
    margin-top: 2rem;
  }
  .composer.in-feed {
    border-top: 0;
    padding-top: 0;
    margin-top: 0;
  }
  .composer h2 {
    margin: 0 0 0.4rem;
    font-size: 1.1rem;
  }
  textarea.input {
    width: 100%;
    resize: vertical;
    margin-top: 0.75rem;
    font-family: inherit;
  }
  .in-feed textarea.input {
    margin-top: 0;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    margin-top: 0.6rem;
  }
  .count {
    font-variant-numeric: tabular-nums;
    font-size: 0.88rem;
  }
  .count.warn {
    color: var(--c-acted);
  }
  .note {
    margin-top: 0.8rem;
    font-size: 0.92rem;
  }
  .hint-ok {
    color: var(--c-green-dark);
  }
</style>
