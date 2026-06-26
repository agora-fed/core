<script lang="ts">
  // Lista de sessões ativas + "encerrar" individual. A sessão deste dispositivo (current=true)
  // recebe um destaque e não pode ser revogada por aqui — pra encerrar a atual, use Sair.
  import { onMount } from 'svelte';
  import {
    getMySessions,
    revokeSession,
    type SessionInfoDto,
  } from '../../lib/api';

  let loading = $state(true);
  let sessions = $state<SessionInfoDto[]>([]);
  let loadError = $state<string | null>(null);
  let busyId = $state<string | null>(null);
  let status = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  async function reload() {
    const res = await getMySessions();
    if (res.success && res.data) {
      sessions = res.data;
    } else if (res.error?.code === 'http_401') {
      loadError = 'Entre na sua conta para ver suas sessões.';
    } else {
      loadError = res.error?.message ?? 'Não foi possível carregar suas sessões.';
    }
  }

  onMount(async () => {
    await reload();
    loading = false;
  });

  async function revoke(id: string) {
    busyId = id;
    status = null;
    const res = await revokeSession(id);
    busyId = null;
    if (res.success) {
      sessions = sessions.filter((s) => s.id !== id);
      status = { kind: 'ok', text: 'Sessão encerrada.' };
    } else {
      status = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível encerrar.',
      };
    }
  }

  function fmt(iso: string): string {
    try {
      const d = new Date(iso);
      return d.toLocaleString('pt-BR', {
        day: '2-digit',
        month: 'short',
        year: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      });
    } catch {
      return iso;
    }
  }
</script>

<section class="sessions">
  <h2>Sessões ativas</h2>
  <p class="muted">
    Cada vez que você entra em um navegador novo, uma sessão é criada. Encerre
    as que não reconhece.
  </p>

  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if loadError}
    <p class="hint hint-error" role="alert">{loadError}</p>
  {:else if sessions.length === 0}
    <p class="muted">Nenhuma sessão ativa.</p>
  {:else}
    <ul class="list">
      {#each sessions as s (s.id)}
        <li class="row" class:current={s.current}>
          <div class="meta">
            <strong>
              {s.current ? '🟢 Este dispositivo' : 'Outra sessão'}
            </strong>
            <span class="muted">
              Criada em {fmt(s.issued_at)} · expira em {fmt(s.expires_at)}
            </span>
            <span class="id muted" title={s.id}>
              id: {s.id.slice(0, 8)}…
            </span>
          </div>
          <div>
            {#if s.current}
              <a class="btn btn-ghost" href="/" onclick={() => { /* Sair pelo header */ }}>
                Use “Sair”
              </a>
            {:else}
              <button
                class="btn btn-ghost danger"
                type="button"
                onclick={() => revoke(s.id)}
                disabled={busyId === s.id}
              >
                {busyId === s.id ? 'Encerrando…' : 'Encerrar'}
              </button>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}

  {#if status}
    <p class={`hint ${status.kind === 'error' ? 'hint-error' : 'hint-ok'}`} role="status">
      {status.text}
    </p>
  {/if}
</section>

<style>
  .sessions {
    border-top: 1px solid var(--c-border);
    padding-top: 1.5rem;
    margin-top: 2rem;
  }
  .sessions h2 {
    margin: 0 0 0.5rem;
    font-size: 1.1rem;
  }
  .list {
    list-style: none;
    padding: 0;
    margin: 1rem 0 0;
    display: grid;
    gap: 0.6rem;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.85rem 1rem;
    border: 1px solid var(--c-border);
    border-radius: 10px;
    background: var(--c-paper);
    flex-wrap: wrap;
  }
  .row.current {
    border-color: var(--c-green-dark);
    background: var(--c-green-soft);
  }
  .meta {
    display: grid;
    gap: 0.15rem;
    flex: 1;
    min-width: 14rem;
  }
  .meta .muted {
    font-size: 0.88rem;
  }
  .id {
    font-family: ui-monospace, SFMono-Regular, monospace;
    font-size: 0.78rem;
  }
  .btn.danger {
    color: var(--c-ignored);
  }
  .hint-ok {
    color: var(--c-green-dark);
  }
</style>
