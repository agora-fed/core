<script lang="ts">
  // Search a fediverse account by `@user@host` and (optionally) follow it. The result card is
  // intentionally minimal — name, handle, summary, avatar, Seguir button. Follow goes async
  // ("Solicitação enviada"); the Accept comes back to our inbox and the row flips to ACK'd then.
  import {
    lookupRemoteActor,
    followRemoteActor,
    type RemoteActorDto,
  } from '../../lib/api';

  let acct = $state('');
  let looking = $state(false);
  let result = $state<RemoteActorDto | null>(null);
  let error = $state<string | null>(null);
  let following = $state(false);
  let followState = $state<'idle' | 'sent' | 'failed'>('idle');

  let valid = $derived(/^@?[^\s@]+@[^\s@]+\.[^\s@]+$/.test(acct.trim()));

  async function lookup(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || looking) return;
    looking = true;
    error = null;
    result = null;
    followState = 'idle';
    const res = await lookupRemoteActor(acct.trim());
    looking = false;
    if (res.success && res.data) {
      result = res.data;
    } else {
      error = res.error?.message ?? 'Não consegui encontrar essa conta.';
    }
  }

  async function follow() {
    if (!result || following) return;
    following = true;
    const res = await followRemoteActor(result.remote_actor_url);
    following = false;
    if (res.success) {
      followState = 'sent';
    } else {
      followState = 'failed';
      error = res.error?.message ?? 'Não consegui enviar o pedido de seguir.';
    }
  }

  // Drop every HTML tag from a Mastodon summary so we render text-only without XSS risk.
  // Trade-off: lose links and emphasis; gain safety without pulling in a sanitizer dep.
  function stripHtml(html: string): string {
    return html
      .replace(/<[^>]+>/g, ' ')
      .replace(/\s+/g, ' ')
      .trim();
  }
</script>

<section class="fedi">
  <h2>Encontrar pessoas no fediverso</h2>
  <p class="muted">
    Busque por <code>@usuario@instancia</code> (Mastodon, Pleroma, outras
    instâncias DemocraciaBR). Você precisa ter perfil público para seguir.
  </p>

  <form class="row" onsubmit={lookup} novalidate>
    <input
      class="input"
      type="text"
      bind:value={acct}
      placeholder="@m@pop.coop"
      autocomplete="off"
      spellcheck="false"
    />
    <button
      class="btn btn-primary"
      type="submit"
      disabled={!valid || looking}
    >
      {looking ? 'Buscando…' : 'Buscar'}
    </button>
  </form>

  {#if error}
    <p class="hint hint-error" role="alert">{error}</p>
  {/if}

  {#if result}
    <article class="card-result">
      {#if result.avatar_url}
        <img class="avatar" src={result.avatar_url} alt="" />
      {:else}
        <span class="avatar avatar-placeholder" aria-hidden="true">👤</span>
      {/if}
      <div class="meta">
        <strong>{result.name ?? result.preferred_username ?? 'Sem nome'}</strong>
        <span class="muted">{result.handle}</span>
        {#if result.summary}
          <div class="summary muted">{stripHtml(result.summary)}</div>
        {/if}
      </div>
      <div class="actions">
        {#if followState === 'sent'}
          <span class="hint hint-ok">Solicitação enviada ✓</span>
        {:else}
          <!-- Neste ramo `followState` nunca é 'sent' (o #if acima captura), então basta
               travar enquanto o pedido está em voo. -->
          <button
            class="btn btn-primary"
            type="button"
            onclick={follow}
            disabled={following}
          >
            {following ? 'Enviando…' : 'Seguir'}
          </button>
        {/if}
      </div>
    </article>
  {/if}
</section>

<style>
  .fedi {
    border-top: 1px solid var(--c-border);
    padding-top: 1.5rem;
    margin-top: 2rem;
  }
  .fedi h2 {
    margin: 0 0 0.4rem;
    font-size: 1.1rem;
  }
  .row {
    display: flex;
    gap: 0.5rem;
    margin: 1rem 0 0.5rem;
    flex-wrap: wrap;
  }
  .row .input {
    flex: 1;
    min-width: 12rem;
  }
  .card-result {
    display: flex;
    gap: 1rem;
    align-items: flex-start;
    margin-top: 1rem;
    padding: 1rem;
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 12px;
    flex-wrap: wrap;
  }
  .avatar {
    width: 56px;
    height: 56px;
    border-radius: 50%;
    object-fit: cover;
    flex-shrink: 0;
  }
  .avatar-placeholder {
    background: var(--c-bg);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 1.3rem;
  }
  .meta {
    flex: 1;
    min-width: 14rem;
    display: grid;
    gap: 0.25rem;
  }
  .summary {
    margin-top: 0.4rem;
    font-size: 0.9rem;
    line-height: 1.4;
  }
  .actions {
    align-self: center;
  }
  .hint-ok {
    color: var(--c-green-dark);
    font-weight: 600;
  }
  code {
    font-family: ui-monospace, SFMono-Regular, monospace;
    background: var(--c-bg);
    padding: 0.05rem 0.3rem;
    border-radius: 4px;
  }
</style>
