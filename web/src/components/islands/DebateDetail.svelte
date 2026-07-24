<script lang="ts">
  // Detalhe de um debate: enunciado + contribuições agrupadas por posição
  // (pró / contra / neutro) + formulário pra participar (gated no login).
  import { onMount } from 'svelte';
  import {
    getDebate,
    getDebateContributions,
    contributeToDebate,
    type DebateDto,
    type ContributionDto,
  } from '../../lib/api';

  // O id vem do querystring (?id=) — SSG não conhece debates novos, a ilha
  // resolve no client (mesmo padrão de /campanha, /grupo, /municipio).
  let debateId = $state('');

  let loading = $state(true);
  let error = $state<string | null>(null);
  let debate = $state<DebateDto | null>(null);
  let contributions = $state<ContributionDto[]>([]);

  // Form.
  let stance = $state<'pro' | 'con' | 'neutral'>('pro');
  let body = $state('');
  let busy = $state(false);
  let formMsg = $state<string | null>(null);
  let loggedIn = $state(false);

  const fmt = new Intl.DateTimeFormat('pt-BR', { day: '2-digit', month: 'short', year: 'numeric' });

  const STANCES = [
    { key: 'pro', label: 'A favor', cls: 'pro' },
    { key: 'con', label: 'Contra', cls: 'con' },
    { key: 'neutral', label: 'Ponderação', cls: 'neutral' },
  ] as const;

  function byStance(s: string) {
    return contributions.filter((c) => c.stance === s);
  }

  function stanceLabel(s: string) {
    return s === 'pro' ? 'A favor' : s === 'con' ? 'Contra' : 'Ponderação';
  }

  let bodyValid = $derived(body.trim().length >= 3);

  async function loadContributions() {
    const res = await getDebateContributions(debateId);
    if (res.ok && res.data) contributions = res.data;
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (!bodyValid || busy) return;
    if (!loggedIn) {
      window.location.href = `/entrar/?next=${encodeURIComponent(location.pathname + location.search)}`;
      return;
    }
    busy = true;
    formMsg = null;
    const res = await contributeToDebate(debateId, stance, body.trim());
    busy = false;
    if (res.success) {
      body = '';
      await loadContributions();
    } else {
      formMsg = res.error?.message ?? 'Não foi possível enviar sua contribuição.';
    }
  }

  onMount(async () => {
    try {
      loggedIn = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {
      loggedIn = false;
    }
    debateId = new URLSearchParams(window.location.search).get('id') ?? '';
    if (!debateId) {
      error = 'Debate não informado.';
      loading = false;
      return;
    }
    const [d, c] = await Promise.all([getDebate(debateId), getDebateContributions(debateId)]);
    loading = false;
    if (d.ok && d.data) debate = d.data;
    else {
      error = d.error ?? 'Debate não encontrado.';
      return;
    }
    if (c.ok && c.data) contributions = c.data;
  });
</script>

{#if loading}
  <p class="muted">Carregando debate…</p>
{:else if error || !debate}
  <div class="card center">
    <p class="hint-error" role="alert">{error ?? 'Debate não encontrado.'}</p>
    <p class="muted"><a href="/debates">Ver todos os debates</a></p>
  </div>
{:else}
  <header class="head">
    <p class="eyebrow muted"><a href="/debates">← Debates</a></p>
    <h1>{debate.title}</h1>
    <p class="framing">{debate.framing}</p>
    <p class="muted small">
      {contributions.length} contribuiç{contributions.length === 1 ? 'ão' : 'ões'}
    </p>
  </header>

  <!-- Colunas pró / contra / ponderação -->
  <div class="columns">
    {#each STANCES as s (s.key)}
      {@const list = byStance(s.key)}
      <section class={`col ${s.cls}`}>
        <h2>{s.label} <span class="count muted">{list.length}</span></h2>
        {#if list.length === 0}
          <p class="muted empty">Ninguém ainda.</p>
        {:else}
          <ul>
            {#each list as c (c.id)}
              <li class="contrib card">
                <p class="c-body">{c.body}</p>
                <time class="muted small">{fmt.format(new Date(c.created_at))}</time>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/each}
  </div>

  <!-- Participar -->
  <form class="participate card" onsubmit={submit}>
    <h2>Participar do debate</h2>
    <div class="stance-row">
      {#each STANCES as s (s.key)}
        <button
          type="button"
          class={`stance-btn ${s.cls}`}
          class:active={stance === s.key}
          onclick={() => (stance = s.key)}
        >{s.label}</button>
      {/each}
    </div>
    <textarea
      bind:value={body}
      rows="4"
      maxlength="20000"
      placeholder={loggedIn ? 'Seu argumento…' : 'Entre na sua conta para participar.'}
    ></textarea>
    {#if formMsg}<p class="hint-error" role="alert">{formMsg}</p>{/if}
    <button type="submit" class="btn-primary" disabled={busy || !bodyValid}>
      {busy ? 'Enviando…' : loggedIn ? `Contribuir — ${stanceLabel(stance)}` : 'Entrar para participar'}
    </button>
  </form>
{/if}

<style>
  .head { margin-bottom: 1.5rem; }
  .eyebrow a { color: var(--text-3, #888); text-decoration: none; }
  .head h1 { margin: 0.3rem 0 0.5rem; font-size: 1.7rem; }
  .framing { font-size: 1.08rem; color: var(--text-2, inherit); margin: 0 0 0.5rem; white-space: pre-wrap; }
  .small { font-size: 0.85rem; }

  .columns {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: 1rem;
    margin-bottom: 2rem;
  }
  .col h2 {
    font-size: 1.05rem;
    margin: 0 0 0.7rem;
    padding-bottom: 0.4rem;
    border-bottom: 3px solid var(--c);
  }
  .col.pro { --c: #15803d; }
  .col.con { --c: #dc2626; }
  .col.neutral { --c: #64748b; }
  .count { font-weight: 400; font-size: 0.85rem; }
  .col ul { list-style: none; padding: 0; margin: 0; display: grid; gap: 0.6rem; }
  .empty { font-size: 0.9rem; }
  .contrib { padding: 0.8rem 0.9rem; border-left: 3px solid var(--c); }
  .c-body { margin: 0 0 0.35rem; white-space: pre-wrap; font-size: 0.95rem; }

  .participate { padding: 1.2rem; display: grid; gap: 0.85rem; }
  .participate h2 { margin: 0; font-size: 1.15rem; }
  .stance-row { display: flex; gap: 0.5rem; flex-wrap: wrap; }
  .stance-btn {
    padding: 0.45rem 0.9rem;
    border-radius: 999px;
    border: 1px solid var(--border-subtle, #ccc);
    background: var(--surface-1, #fff);
    color: var(--text-1, inherit);
    font-weight: 600;
    cursor: pointer;
  }
  .stance-btn.active.pro { background: #15803d; color: #fff; border-color: #15803d; }
  .stance-btn.active.con { background: #dc2626; color: #fff; border-color: #dc2626; }
  .stance-btn.active.neutral { background: #64748b; color: #fff; border-color: #64748b; }
  textarea {
    padding: 0.6rem 0.7rem;
    border-radius: 8px;
    border: 1px solid var(--border-subtle, #ccc);
    background: var(--surface-1, #fff);
    color: var(--text-1, inherit);
    font: inherit;
    resize: vertical;
  }
  .btn-primary {
    justify-self: start;
    padding: 0.55rem 1.2rem;
    border-radius: 8px;
    border: none;
    background: var(--accent, #15803d);
    color: var(--accent-contrast, #fff);
    font-weight: 700;
    cursor: pointer;
  }
  .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }
  .card {
    background: var(--surface-1, #fff);
    border: 1px solid var(--border-subtle, rgba(0,0,0,0.1));
    border-radius: 12px;
  }
  .center { text-align: center; padding: 2.5rem 1.5rem; }
  .hint-error { color: #dc2626; margin: 0; }
</style>
